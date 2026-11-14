//! The native-window adapter for UiTree. Registration and direct entry share the same scope.
use matcha_window::event::{device_event::DeviceEvent, window_event::WindowEvent};
use matcha_window::window::WindowId;
use nalgebra::Matrix4;
use render_interface::Scene;

use super::{Window, WindowBinding};
use crate::arena::Id as UiId;
use crate::metrics::{Bounds, Position};
use crate::ui_tree::context::Context;
use crate::ui_tree::widget::{InputResult, UiError, UpdateResult, View};
use crate::ui_tree::{DeclContext, UiTree};

impl<'a> DeclContext<'a, '_, ()> {
    /// Mount with the current declaration, or apply its controlled settings to the live window.
    pub(super) fn declare_window<V: View>(&mut self, view: &'a Window<V>) {
        self.declaration.window = Some(WindowBinding::new::<V>());
        let node = self.declaration.current;
        let registry = self.context.window_registry();
        if let Some(window) = registry.window_for_node(node) {
            view.config.apply_updates(window);
            window.request_redraw();
        } else {
            let Some(window) = self.context.open_window(&view.config) else {
                return;
            };
            window.request_redraw();
            self.context.window_registry().register(node, window);
        }
        self.child(&view.view);
    }
}

impl UiTree {
    fn window_content_context<'a>(
        &self,
        id: WindowId,
        ctx: &'a mut Context<'_>,
    ) -> Option<(UiId, Context<'a>)> {
        let node_id = ctx.window_registry().node(id)?;
        let node = self.node_ref(node_id).expect("live window");
        let mut children = node.child_ids();
        let (Some(content), None) = (children.next(), children.next()) else {
            unreachable!("a Window declares exactly one content child");
        };
        Some((content, ctx.unobserved().with_window(id)))
    }

    /// Deliver a native window event. Unknown/deleted windows are ignored.
    /// Environment changes invalidate the target window's caches, excluding nested windows.
    pub fn window_event(
        &mut self,
        id: WindowId,
        event: &WindowEvent,
        ctx: &mut Context,
    ) -> Option<UpdateResult> {
        let node_id = ctx.window_registry().node(id)?;
        let node = self.node_ref(node_id).expect("live window");
        let binding = node.window.expect("registered window boundary");
        let widget = node.widget.as_deref().expect("live widget");
        let flags = (binding.window_event)(widget.as_widget(), event);
        if !flags.is_empty() {
            self.invalidate_caches_from(node_id, flags);
            ctx.window_registry()
                .window(id)
                .expect("live window is registered")
                .request_redraw();
        }
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

    /// Lay out one native window's content in its own viewport, independently of parent placement.
    pub fn layout_window(&mut self, id: WindowId, ctx: &mut Context) -> bool {
        let Some((child, mut scoped)) = self.window_content_context(id, ctx) else {
            return false;
        };
        let [x, y] = scoped.viewport_size();
        self.layout_node(
            child,
            Position {
                bounds: Bounds { x, y, z: 0.0 },
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
        let Some((child, mut scoped)) = self.window_content_context(id, ctx) else {
            return Ok(None);
        };
        self.render_from(child, &mut scoped).map(Some)
    }

    /// Dispatch input inside one native window; bubbling ends at its content root.
    pub fn input_window(
        &mut self,
        id: WindowId,
        event: &DeviceEvent,
        ctx: &mut Context,
    ) -> InputResult {
        let Some((child, mut scoped)) = self.window_content_context(id, ctx) else {
            return InputResult::default();
        };
        self.input_from(child, event, &mut scoped)
    }
}
