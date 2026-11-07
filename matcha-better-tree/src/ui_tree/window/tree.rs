//! The native-window adapter for UiTree. Registration and direct entry share the same scope.
use matcha_window::event::{device_event::DeviceEvent, window_event::WindowEvent};
use matcha_window::window::WindowId;
use nalgebra::Matrix4;
use render_interface::Scene;

use super::{WindowBinding, WindowConfig, WindowInstanceId};
use crate::arena::Id as UiId;
use crate::metrics::{Bounds, Position};
use crate::ui_tree::context::Context;
use crate::ui_tree::widget::{InputResult, UiError, UpdateResult, View};
use crate::ui_tree::{DeclContext, UiTree};

impl DeclContext<'_, '_> {
    /// Register identity and concrete dispatch together; callers cannot install mismatched hooks.
    pub(crate) fn register_window<V: View>(&mut self, config: WindowConfig) -> WindowInstanceId {
        assert!(
            self.declaration.window.is_none(),
            "one native window registration per declaration node"
        );
        let id = self
            .context
            .window_registry()
            .register(self.declaration.current, config);
        let binding = WindowBinding::new::<V>(id);
        self.declaration.window = Some(binding);
        id
    }
}

impl UiTree {
    fn window_route(&self, id: WindowId, ctx: &mut Context) -> Option<(UiId, WindowBinding)> {
        let (node_id, instance_id) = ctx.window_registry().route(id)?;
        let node = self.node_ref(node_id)?;
        let binding = node.window.expect("registered window node has a binding");
        assert_eq!(binding.id(), instance_id, "registered window node matches");
        Some((node_id, binding))
    }

    fn window_content_context<'a>(
        &self,
        id: WindowId,
        ctx: &'a mut Context<'_>,
    ) -> Option<(UiId, Context<'a>)> {
        let (node_id, binding) = self.window_route(id, ctx)?;
        let node = self.node_ref(node_id).expect("live window");
        let (child, scoped) =
            binding.content_context(node.widget.as_deref().expect("live widget"), ctx);
        let child = node.child(child).expect("declared window content");
        Some((child, scoped))
    }

    /// Deliver a native window event. Unknown/deleted windows are ignored.
    /// Environment changes invalidate the target window's caches, excluding nested windows.
    pub fn window_event(
        &mut self,
        id: WindowId,
        event: &WindowEvent,
        ctx: &mut Context,
    ) -> Option<UpdateResult> {
        let (node_id, binding) = self.window_route(id, ctx)?;
        let widget = self
            .node_ref_mut(node_id)?
            .widget
            .as_deref_mut()
            .expect("live widget");
        let flags = binding.dispatch(widget, event);
        if !flags.is_empty() {
            self.invalidate_caches_from(node_id, flags);
            ctx.window_registry()
                .get_mut(binding.id())
                .expect("live window is registered")
                .request_redraw();
        }
        Some(flags)
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
        let Some((node_id, binding)) = self.window_route(id, ctx) else {
            return false;
        };
        self.invalidate_caches_from(node_id, flags);
        ctx.window_registry()
            .get_mut(binding.id())
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
