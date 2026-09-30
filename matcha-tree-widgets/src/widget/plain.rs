use std::sync::Arc;

use crate::buffer::Buffer;
use crate::layout::reconcile_single_child;
use crate::style::Style;
use matcha_tree::event::device_event::DeviceEvent;
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::Constraints,
    widget::{View, Widget, WidgetInteractionResult, WidgetPod},
};
use render_interface::Draw;

use crate::types::size::{ChildSize, Size};

// MARK: View

pub struct Plain {
    pub label: Option<String>,
    pub style: Vec<Arc<dyn Style>>,
    pub content: Option<Box<dyn View>>,
    pub size: [Size; 2],
}

impl Default for Plain {
    fn default() -> Self {
        Self::new()
    }
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
                buffer: Buffer::clipped(self.style.clone()),
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
    buffer: Buffer,
    style: Vec<Arc<dyn Style>>,
    size: [Size; 2],
    child: Option<WidgetPod>,
}

impl Widget for PlainWidget {
    type View = Plain;

    fn update(&mut self, view: &Plain, ctx: &UiContext) -> WidgetInteractionResult {
        let size_changed = self.size != view.size;
        // Stable shared styles keep their content identities across view updates.
        let style_changed = self.style.len() != view.style.len()
            || self
                .style
                .iter()
                .zip(&view.style)
                .any(|(a, b)| !Arc::ptr_eq(a, b));
        if style_changed {
            self.buffer = Buffer::clipped(view.style.clone());
        }
        self.style = view.style.clone();
        self.size = view.size.clone();
        let child_result = reconcile_single_child(&mut self.child, view.content.as_deref(), ctx);
        if size_changed || matches!(child_result, WidgetInteractionResult::LayoutNeeded) {
            WidgetInteractionResult::LayoutNeeded
        } else if style_changed {
            WidgetInteractionResult::RedrawNeeded
        } else {
            child_result
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

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext, draw: &mut Draw<'_>) {
        self.buffer.paint(bounds, ctx, draw);
        if let Some(child) = &mut self.child {
            child.render(bounds, ctx, draw);
        }
    }
}
