use std::any::Any;

use renderer::render_node::RenderNode;
use utils::maybe_send_sync::MaybeSendSync;

use super::metrics;
use crate::ui_tree::context::UiContext;
use matcha_window::event::device_event::DeviceEvent;

// ----------------------------------------------------------------------------
// Types
// ----------------------------------------------------------------------------

/// Represents an error that can occur when updating a `Widget` tree.
pub enum WidgetUpdateError {
    /// Occurs when the type of the new `Dom` node does not match the existing `Widget`.
    TypeMismatch,
}

pub enum WidgetInteractionResult {
    // Need to rearrange layout (and also redraw).
    LayoutNeeded,
    // Need to redraw.
    RedrawNeeded,
    // No change.
    NoChange,
}

// ----------------------------------------------------------------------------
// Key Structs and Traits
// ----------------------------------------------------------------------------

pub trait View: MaybeSendSync + Any {
    fn build(&self, ctx: &UiContext) -> WidgetPod;
}

pub trait Widget: MaybeSendSync + Any {
    type View: View;

    fn update(&mut self, view: &Self::View, ctx: &UiContext) -> WidgetInteractionResult;

    fn device_input(
        &mut self,
        bounds: [f32; 2],
        event: &DeviceEvent,
        ctx: &UiContext,
    ) -> WidgetInteractionResult;

    fn is_inside(&self, bounds: [f32; 2], position: [f32; 2], ctx: &UiContext) -> bool {
        let _ = ctx;

        0.0 <= position[0]
            && position[0] <= bounds[0]
            && 0.0 <= position[1]
            && position[1] <= bounds[1]
    }

    fn measure(&self, constraints: &metrics::Constraints, ctx: &UiContext) -> [f32; 2];

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode;
}

/// Wrapper trait to erase the concrete Widget type.
pub(super) trait AnyWidget: MaybeSendSync + Any {
    fn try_update(
        &mut self,
        view: &dyn View,
        ctx: &UiContext,
    ) -> Result<WidgetInteractionResult, WidgetUpdateError>;

    fn device_input(
        &mut self,
        bounds: [f32; 2],
        event: &DeviceEvent,
        ctx: &UiContext,
    ) -> WidgetInteractionResult;

    fn is_inside(&self, bounds: [f32; 2], position: [f32; 2], ctx: &UiContext) -> bool;

    fn measure(&self, constraints: &metrics::Constraints, ctx: &UiContext) -> [f32; 2];

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode;
}

impl<W, V> AnyWidget for W
where
    W: Widget<View = V>,
    V: View,
{
    fn try_update(
        &mut self,
        view: &dyn View,
        ctx: &UiContext,
    ) -> Result<WidgetInteractionResult, WidgetUpdateError> {
        let Some(view) = (view as &dyn Any).downcast_ref::<V>() else {
            return Err(WidgetUpdateError::TypeMismatch);
        };

        Ok(self.update(view, ctx))
    }

    fn device_input(
        &mut self,
        bounds: [f32; 2],
        event: &DeviceEvent,
        ctx: &UiContext,
    ) -> WidgetInteractionResult {
        Widget::device_input(self, bounds, event, ctx)
    }

    fn is_inside(&self, bounds: [f32; 2], position: [f32; 2], ctx: &UiContext) -> bool {
        Widget::is_inside(self, bounds, position, ctx)
    }

    fn measure(&self, constraints: &metrics::Constraints, ctx: &UiContext) -> [f32; 2] {
        Widget::measure(self, constraints, ctx)
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        Widget::render(self, bounds, ctx)
    }
}

pub struct WidgetPod {
    label: Option<String>,
    id_hash: usize,

    widget: Box<dyn AnyWidget>,

    // cache
    // NOTE: Use cache existency as redraw flag.
    render_cache: Option<RenderNode>,
}

impl WidgetPod {
    pub fn new(id: impl std::hash::Hash, widget: impl Widget + 'static) -> Self {
        let id_hash = fxhash::hash(&id);

        Self {
            label: None,
            id_hash,
            widget: Box::new(widget),
            render_cache: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn id_hash(&self) -> usize {
        self.id_hash
    }
}

impl WidgetPod {
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub fn need_redraw(&self) -> bool {
        self.render_cache.is_none()
    }

    pub fn invalidate_render_cache(&mut self) {
        self.render_cache = None;
    }

    pub fn try_update(
        &mut self,
        view: &dyn View,
        ctx: &UiContext,
    ) -> Result<WidgetInteractionResult, WidgetUpdateError> {
        let result = self.widget.try_update(view, ctx)?;
        match result {
            WidgetInteractionResult::LayoutNeeded | WidgetInteractionResult::RedrawNeeded => {
                self.invalidate_render_cache();
            }
            WidgetInteractionResult::NoChange => {}
        }
        Ok(result)
    }

    pub fn device_input(
        &mut self,
        bounds: [f32; 2],
        event: &DeviceEvent,
        ctx: &UiContext,
    ) -> WidgetInteractionResult {
        let interaction_result = self.widget.device_input(bounds, event, ctx);

        match interaction_result {
            WidgetInteractionResult::LayoutNeeded => {
                self.invalidate_render_cache();
            }
            WidgetInteractionResult::RedrawNeeded => {
                self.invalidate_render_cache();
            }
            WidgetInteractionResult::NoChange => {}
        }

        interaction_result
    }

    pub fn is_inside(&self, bounds: [f32; 2], position: [f32; 2], ctx: &UiContext) -> bool {
        self.widget.is_inside(bounds, position, ctx)
    }

    pub fn measure(&self, constraints: &metrics::Constraints, ctx: &UiContext) -> [f32; 2] {
        self.widget.measure(constraints, ctx)
    }

    pub fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        if let Some(render_node) = &self.render_cache {
            return render_node.clone();
        }

        let render_node = self.widget.render(bounds, ctx);
        self.render_cache = Some(render_node.clone());
        render_node
    }
}
