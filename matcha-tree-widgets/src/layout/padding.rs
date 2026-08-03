use nalgebra::Matrix4;

use matcha_tree::{
    event::device_event::DeviceEvent,
    ui_tree::{
        context::UiContext,
        metrics::Constraints,
        widget::{View, Widget, WidgetInteractionResult, WidgetPod},
    },
};
use renderer::render_node::RenderNode;

use super::reconcile_single_child;

pub struct Padding {
    pub label: Option<String>,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    pub content: Option<Box<dyn View>>,
}

impl Padding {
    pub fn new() -> Self {
        Self {
            label: None,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
            content: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn all(v: f32) -> Self {
        Self {
            label: None,
            top: v,
            right: v,
            bottom: v,
            left: v,
            content: None,
        }
    }

    pub fn top(mut self, v: f32) -> Self {
        self.top = v;
        self
    }

    pub fn right(mut self, v: f32) -> Self {
        self.right = v;
        self
    }

    pub fn bottom(mut self, v: f32) -> Self {
        self.bottom = v;
        self
    }

    pub fn left(mut self, v: f32) -> Self {
        self.left = v;
        self
    }

    pub fn content(mut self, content: impl View + 'static) -> Self {
        self.content = Some(Box::new(content));
        self
    }
}

impl Default for Padding {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Padding {
    fn build(&self, ctx: &UiContext) -> WidgetPod {
        let child = self.content.as_ref().map(|c| c.build(ctx));
        let mut pod = WidgetPod::new(
            0usize,
            PaddingWidget {
                top: self.top,
                right: self.right,
                bottom: self.bottom,
                left: self.left,
                child,
            },
        );
        if let Some(label) = &self.label {
            pod = pod.with_label(label.clone());
        }
        pod
    }
}

pub struct PaddingWidget {
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
    child: Option<WidgetPod>,
}

impl PaddingWidget {
    fn inner_bounds(&self, bounds: [f32; 2]) -> [f32; 2] {
        [
            (bounds[0] - self.left - self.right).max(0.0),
            (bounds[1] - self.top - self.bottom).max(0.0),
        ]
    }

    fn child_affine(&self) -> Matrix4<f32> {
        Matrix4::new_translation(&nalgebra::Vector3::new(self.left, self.top, 0.0))
    }
}

impl Widget for PaddingWidget {
    type View = Padding;

    fn update(&mut self, view: &Padding, ctx: &UiContext) -> WidgetInteractionResult {
        let dims_changed = self.top != view.top
            || self.right != view.right
            || self.bottom != view.bottom
            || self.left != view.left;
        self.top = view.top;
        self.right = view.right;
        self.bottom = view.bottom;
        self.left = view.left;
        let child_changed = reconcile_single_child(&mut self.child, view.content.as_deref(), ctx);
        if dims_changed || child_changed {
            WidgetInteractionResult::LayoutNeeded
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
        let affine = self.child_affine();
        let inner = self.inner_bounds(bounds);
        let child_event = event.transform(affine);
        if let Some(child) = &mut self.child {
            return child.device_input(inner, &child_event, ctx);
        }
        WidgetInteractionResult::NoChange
    }

    fn measure(&self, constraints: &Constraints, ctx: &UiContext) -> [f32; 2] {
        let h_pad = self.left + self.right;
        let v_pad = self.top + self.bottom;
        let inner = Constraints::new(
            [
                (constraints.min_width() - h_pad).max(0.0),
                (constraints.max_width() - h_pad).max(0.0),
            ],
            [
                (constraints.min_height() - v_pad).max(0.0),
                (constraints.max_height() - v_pad).max(0.0),
            ],
        );
        let content_size = self
            .child
            .as_ref()
            .map(|c| c.measure(&inner, ctx))
            .unwrap_or([0.0, 0.0]);
        [content_size[0] + h_pad, content_size[1] + v_pad]
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        let inner = self.inner_bounds(bounds);
        let affine = self.child_affine();
        if let Some(child) = &mut self.child {
            let child_node = child.render(inner, ctx);
            RenderNode::new().add_child(child_node, affine)
        } else {
            RenderNode::new()
        }
    }
}
