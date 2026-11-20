//! The native-window adapter for UiTree. Registration and direct entry share the same scope.
use matcha_window::event::{device_event::DeviceEvent, window_event::WindowEvent};
use matcha_window::window::WindowId;
use nalgebra::Matrix4;
use render_interface::Scene;

use super::Window;
use crate::arena::Id as UiId;
use crate::metrics::{Bounds, Position};
use crate::ui_tree::context::Context;
use crate::ui_tree::env_reads::EnvReads;
use crate::ui_tree::widget::{InputResult, UiError, UpdateResult, View, WidgetPhase};
use crate::ui_tree::{ChildScope, DeclContext, InvalidatedCaches, NodeBody, PollSummary, UiTree};

impl<'a> DeclContext<'a, '_, ()> {
    /// Mount with the current declaration, or apply its controlled settings to the live window.
    pub(super) fn declare_window<V: View>(&mut self, view: &'a Window<V>) {
        let node = self.declaration.current;
        let registry = self.context.window_registry();
        let id = if let Some(window) = registry.window_for_node(node) {
            view.config.apply_updates(window);
            window.request_redraw();
            window.id()
        } else {
            let Some(window) = self.context.open_window(&view.config) else {
                self.declaration.child_scope = ChildScope::UnmountedWindow;
                return;
            };
            window.request_redraw();
            self.context.window_registry().register(node, window)
        };
        self.declaration.child_scope = ChildScope::Native(id);
        self.child(&view.view);
    }
}

impl UiTree {
    fn window_content_context<'a>(
        &self,
        id: WindowId,
        ctx: &'a mut Context<'_>,
    ) -> Option<(UiId, Context<'a>, EnvReads)> {
        let mut scoped = ctx.unobserved();
        let node_id = scoped.window_registry().node(id)?;
        let node = self.node_ref(node_id).expect("live window");
        let mut children = node.child_ids();
        let (Some(content), None) = (children.next(), children.next()) else {
            unreachable!("a Window declares exactly one content child");
        };
        let mut scoped = scoped.with_window(id);
        let dpi = scoped.dpi();
        let viewport = scoped.viewport_size();
        let changes = scoped.window_registry().take_env_changes(id, dpi, viewport);
        Some((content, scoped, changes))
    }

    /// A direct pass may precede the next poll. Apply pending changes without invoking widgets.
    fn prepare_window_content<'a>(
        &mut self,
        id: WindowId,
        ctx: &'a mut Context<'_>,
    ) -> Option<(UiId, Context<'a>)> {
        let (child, scoped, changes) = self.window_content_context(id, ctx)?;
        if !changes.is_empty()
            && !self
                .node_ref(child)
                .expect("live content")
                .body()
                .is_window()
        {
            self.invalidate_env_from(child, changes);
        }
        Some((child, scoped))
    }

    fn invalidate_env_from(&mut self, id: UiId, changed: EnvReads) -> InvalidatedCaches {
        let mut affected = InvalidatedCaches::empty();
        self.for_each_child_in_scope(id, |tree, child| {
            affected |= tree.invalidate_env_from(child, changed);
        });
        let node = self.node_ref_mut(id).expect("live window content");
        node.invalidate_env(changed, &mut affected);
        if node.phase == WidgetPhase::PendingEnter {
            return InvalidatedCaches::empty();
        }
        affected
    }

    /// Deliver a native window event. Unknown/deleted windows are ignored. Metric notifications
    /// request a redraw immediately and are coalesced until the next poll or direct window pass.
    pub fn window_event(
        &mut self,
        id: WindowId,
        event: &WindowEvent,
        ctx: &mut Context,
    ) -> Option<UpdateResult> {
        let node_id = ctx.window_registry().node(id)?;
        let node = self.node_ref(node_id).expect("live window");
        let NodeBody::Window(window) = node.body() else {
            unreachable!("a registered OS window belongs to a mounted window node");
        };
        let flags = match event {
            WindowEvent::CloseRequested => {
                window.close_requested();
                return Some(UpdateResult::empty());
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                ctx.window_registry().note_env_change(id, EnvReads::DPI);
                UpdateResult::LAYOUT | UpdateResult::RENDER
            }
            WindowEvent::Resized { .. } => {
                ctx.window_registry()
                    .note_env_change(id, EnvReads::VIEWPORT_SIZE);
                UpdateResult::LAYOUT | UpdateResult::RENDER
            }
            WindowEvent::Theme(_) => {
                self.invalidate_caches_from(node_id, UpdateResult::RENDER);
                UpdateResult::RENDER
            }
            _ => return Some(UpdateResult::empty()),
        };
        ctx.window_registry()
            .window(id)
            .expect("live window is registered")
            .request_redraw();
        Some(flags)
    }

    /// Release the destroyed OS window. The next declaration supplies fresh creation settings.
    pub(crate) fn window_destroyed(&mut self, id: WindowId, ctx: &mut Context) -> bool {
        let Some(node_id) = ctx.window_registry().node(id) else {
            return false;
        };
        ctx.window_registry().unregister(id);
        self.invalidate_caches_from(node_id, UpdateResult::LAYOUT | UpdateResult::RENDER);
        true
    }

    /// Explicit environment invalidation, e.g. when a target/device change requires new drawing.
    /// Surface recreation alone does not imply CPU Draw definitions became invalid.
    /// Releases selected CPU cache values in this window, excluding nested windows.
    pub fn invalidate_window_caches(
        &mut self,
        id: WindowId,
        flags: UpdateResult,
        ctx: &mut Context,
    ) -> bool {
        let Some(node_id) = ctx.window_registry().node(id) else {
            return false;
        };
        self.invalidate_caches_from(node_id, flags);
        ctx.window_registry()
            .window(id)
            .expect("live window is registered")
            .request_redraw();
        true
    }

    /// Poll one window's content before its measurement/layout/rendering passes.
    /// Nested native windows are polled separately. All callbacks share this Context's
    /// frame time and receive the target window's environment.
    pub fn poll_window(&mut self, id: WindowId, ctx: &mut Context) -> Option<PollSummary> {
        let (child, mut scoped, changes) = self.window_content_context(id, ctx)?;
        let mut summary = self.poll_from(child, &mut scoped, changes);
        if changes.contains(EnvReads::VIEWPORT_SIZE) && self.window_root_resized(child, &mut scoped)
        {
            summary.update |= UpdateResult::LAYOUT;
        }
        Some(summary)
    }

    /// Lay out one native window's content in its own viewport, independently of parent placement.
    pub fn layout_window(&mut self, id: WindowId, ctx: &mut Context) -> bool {
        let Some((child, mut scoped)) = self.prepare_window_content(id, ctx) else {
            return false;
        };
        self.layout_node(
            child,
            Position {
                bounds: viewport_bounds(&mut scoped),
                affine: Matrix4::identity(),
            },
            &mut scoped,
        );
        true
    }

    /// Build only this window's Scene. Nested native windows have separate passes.
    pub fn render_window(
        &mut self,
        id: WindowId,
        ctx: &mut Context,
    ) -> Result<Option<Scene>, UiError> {
        let Some((child, mut scoped)) = self.prepare_window_content(id, ctx) else {
            return Ok(None);
        };
        if self.window_root_resized(child, &mut scoped) {
            return Err(UiError::LayoutRequired);
        }
        self.render_from(child, &mut scoped).map(Some)
    }

    fn window_root_resized(&self, child: UiId, ctx: &mut Context) -> bool {
        let node = self.node_ref(child).expect("live window content");
        node.phase != WidgetPhase::PendingEnter
            && !node.body().is_window()
            && node
                .placement
                .is_some_and(|placed| placed.bounds != viewport_bounds(ctx))
    }

    /// Dispatch input inside one native window; bubbling ends at its content root.
    /// As with standalone input, call layout first to update the hit geometry.
    pub fn input_window(
        &mut self,
        id: WindowId,
        event: &DeviceEvent,
        ctx: &mut Context,
    ) -> InputResult {
        let Some((child, mut scoped)) = self.prepare_window_content(id, ctx) else {
            return InputResult::default();
        };
        self.input_from(child, event, &mut scoped)
    }
}

fn viewport_bounds(ctx: &mut Context) -> Bounds {
    let [x, y] = ctx.viewport_size();
    Bounds { x, y, z: 0.0 }
}
