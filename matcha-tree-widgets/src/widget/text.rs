use matcha_tree::event::device_event::DeviceEvent;
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::Constraints,
    widget::{View, Widget, WidgetInteractionResult, WidgetPod},
};
use renderer::render_node::RenderNode;

use crate::style::Style as _;

pub use crate::style::text::TextSpan;
pub use crate::style::text::{Family, Stretch, Style, Weight};

// MARK: View

pub struct Text {
    pub label: Option<String>,
    pub text: crate::style::text::TextRenderer,
}

impl Text {
    pub fn new() -> Self {
        Self {
            label: None,
            text: crate::style::text::TextRenderer::new(),
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn push_span(mut self, span: TextSpan) -> Self {
        self.text.push_span(span);
        self
    }
}

impl View for Text {
    fn build(&self, _ctx: &UiContext) -> WidgetPod {
        let mut pod = WidgetPod::new(
            0usize,
            TextWidget {
                clear: crate::style::viewport_clear::ViewportClear::new(
                    matcha_tree::color::Color::TRANSPARENT,
                ),
                text: self.text.clone(),
            },
        );
        if let Some(label) = &self.label {
            pod = pod.with_label(label.clone());
        }
        pod
    }
}

// MARK: Widget

pub struct TextWidget {
    clear: crate::style::viewport_clear::ViewportClear,
    text: crate::style::text::TextRenderer,
}

impl Widget for TextWidget {
    type View = Text;

    fn update(&mut self, view: &Text, _ctx: &UiContext) -> WidgetInteractionResult {
        if self.text != view.text {
            // Update spans in-place so the initialized FontSystem (text_shared)
            // is reused across frames.  A full clone would drop and re-create the
            // FontSystem every frame, causing system-font reloading each time.
            self.text.set_spans_from(&view.text);
            WidgetInteractionResult::LayoutNeeded
        } else {
            WidgetInteractionResult::NoChange
        }
    }

    fn device_input(
        &mut self,
        _bounds: [f32; 2],
        _event: &DeviceEvent,
        _ctx: &UiContext,
    ) -> WidgetInteractionResult {
        WidgetInteractionResult::NoChange
    }

    fn is_inside(&self, bounds: [f32; 2], position: [f32; 2], ctx: &UiContext) -> bool {
        self.text.is_inside(position, bounds, ctx)
    }

    fn measure(&self, constraints: &Constraints, ctx: &UiContext) -> [f32; 2] {
        match self.text.required_region(constraints, ctx) {
            Some(r) => [r.width(), r.height()],
            None => [0.0, 0.0],
        }
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        let constraints = Constraints::from_boundary(bounds);
        let size = match self.text.required_region(&constraints, ctx) {
            Some(r) => [r.width(), r.height()],
            None => return RenderNode::new(),
        };

        if size[0] <= 0.0 || size[1] <= 0.0 {
            return RenderNode::new();
        }

        let texture_size = [size[0].ceil() as u32, size[1].ceil() as u32];
        let Ok(style_region) =
            ctx.texture_atlas()
                .allocate(ctx.gpu_device(), ctx.gpu_queue(), texture_size)
        else {
            return RenderNode::new();
        };

        let mut encoder =
            ctx.gpu_device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Text Render Encoder"),
                });

        self.clear
            .draw(&mut encoder, &style_region, size, [0.0, 0.0], ctx);
        self.text
            .draw(&mut encoder, &style_region, size, [0.0, 0.0], ctx);

        ctx.gpu_queue().submit(Some(encoder.finish()));

        RenderNode::new().with_texture(style_region, size, nalgebra::Matrix4::identity())
    }
}
