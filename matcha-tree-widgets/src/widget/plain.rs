use std::sync::Arc;

use crate::layout::reconcile_single_child;
use crate::style::Style;
use matcha_tree::event::device_event::DeviceEvent;
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::Constraints,
    widget::{View, Widget, WidgetInteractionResult, WidgetPod},
};
use renderer::render_node::RenderNode;

use crate::types::size::{ChildSize, Size};

// MARK: View

pub struct Plain {
    pub label: Option<String>,
    pub style: Vec<Arc<dyn Style>>,
    pub content: Option<Box<dyn View>>,
    pub size: [Size; 2],
}

impl Plain {
    pub fn new() -> Self {
        Self {
            label: None,
            style: Vec::new(),
            content: None,
            size: [Size::child_w(1.0), Size::child_h(1.0)],
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn style(mut self, style: impl Style + 'static) -> Self {
        self.style.push(Arc::new(style));
        self
    }

    pub fn content(mut self, content: impl View + 'static) -> Self {
        self.content = Some(Box::new(content));
        self
    }

    pub fn size(mut self, size: [Size; 2]) -> Self {
        self.size = size;
        self
    }
}

impl View for Plain {
    fn build(&self, ctx: &UiContext) -> WidgetPod {
        let child = self.content.as_ref().map(|c| c.build(ctx));
        let mut pod = WidgetPod::new(
            0usize,
            PlainWidget {
                style: self.style.clone(),
                size: self.size.clone(),
                child,
            },
        );
        if let Some(label) = &self.label {
            pod = pod.with_label(label.clone());
        }
        pod
    }
}

// MARK: Widget

pub struct PlainWidget {
    style: Vec<Arc<dyn Style>>,
    size: [Size; 2],
    child: Option<WidgetPod>,
}

impl Widget for PlainWidget {
    type View = Plain;

    fn update(&mut self, view: &Plain, ctx: &UiContext) -> WidgetInteractionResult {
        let size_changed = self.size != view.size;
        // Style is a dyn trait so we cannot do value equality; treat any non-empty
        // style as potentially changed (new Arc is created every frame in view()).
        let style_changed = !view.style.is_empty();
        self.style = view.style.clone();
        self.size = view.size.clone();
        let child_changed = reconcile_single_child(&mut self.child, view.content.as_deref(), ctx);
        if size_changed || child_changed {
            WidgetInteractionResult::LayoutNeeded
        } else if style_changed {
            WidgetInteractionResult::RedrawNeeded
        } else {
            WidgetInteractionResult::NoChange
        }
    }

    fn device_input(
        &mut self,
        bounds: [f32; 2],
        event: &DeviceEvent,
        ctx: &UiContext,
    ) -> WidgetInteractionResult {
        if let Some(child) = &mut self.child {
            return child.device_input(bounds, event, ctx);
        }
        WidgetInteractionResult::NoChange
    }

    fn measure(&self, constraints: &Constraints, ctx: &UiContext) -> [f32; 2] {
        let child_size = self
            .child
            .as_ref()
            .map(|c| c.measure(constraints, ctx))
            .unwrap_or([0.0, 0.0]);

        let parent_size = [constraints.max_width(), constraints.max_height()];
        let mut child_size_provider = ChildSize::new(|| child_size);

        let w = self.size[0].size(parent_size, &mut child_size_provider, ctx);
        let h = self.size[1].size(parent_size, &mut child_size_provider, ctx);
        [w, h]
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        let mut render_node = RenderNode::new();

        if bounds[0] > 0.0 && bounds[1] > 0.0 {
            let texture_size = [bounds[0].ceil() as u32, bounds[1].ceil() as u32];
            if let Ok(style_region) =
                ctx.texture_atlas()
                    .allocate(ctx.gpu_device(), ctx.gpu_queue(), texture_size)
            {
                let mut encoder =
                    ctx.gpu_device()
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("Plain Render Encoder"),
                        });
                for style in &self.style {
                    style.draw(&mut encoder, &style_region, bounds, [0.0, 0.0], ctx);
                }
                ctx.gpu_queue().submit(Some(encoder.finish()));
                render_node =
                    render_node.with_texture(style_region, bounds, nalgebra::Matrix4::identity());
            }
        }

        if let Some(child) = &mut self.child {
            let child_node = child.render(bounds, ctx);
            render_node.push_child(child_node, nalgebra::Matrix4::identity());
        }

        render_node
    }
}
