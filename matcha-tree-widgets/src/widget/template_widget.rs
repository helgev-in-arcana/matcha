use matcha_tree::event::device_event::DeviceEvent;
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::Constraints,
    widget::{View, Widget, WidgetInteractionResult, WidgetPod},
};
use renderer::render_node::RenderNode;

// MARK: View

pub struct Template {
    pub label: Option<String>,
}

impl Template {
    pub fn new() -> Self {
        Self { label: None }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl View for Template {
    fn build(&self, _ctx: &UiContext) -> WidgetPod {
        let mut pod = WidgetPod::new(0usize, TemplateWidget {});
        if let Some(label) = &self.label {
            pod = pod.with_label(label.clone());
        }
        pod
    }
}

// MARK: Widget

pub struct TemplateWidget {}

impl Widget for TemplateWidget {
    type View = Template;

    fn update(&mut self, _view: &Template, _ctx: &UiContext) -> WidgetInteractionResult {
        WidgetInteractionResult::NoChange
    }

    fn device_input(
        &mut self,
        _bounds: [f32; 2],
        _event: &DeviceEvent,
        _ctx: &UiContext,
    ) -> WidgetInteractionResult {
        WidgetInteractionResult::NoChange
    }

    fn measure(&self, _constraints: &Constraints, _ctx: &UiContext) -> [f32; 2] {
        [0.0, 0.0]
    }

    fn render(&mut self, _bounds: [f32; 2], _ctx: &UiContext) -> RenderNode {
        RenderNode::new()
    }
}
