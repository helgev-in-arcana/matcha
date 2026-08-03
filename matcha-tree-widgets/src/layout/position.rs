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

pub struct Position {
    pub label: Option<String>,
    pub left: Option<f32>,
    pub top: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
    pub content: Option<Box<dyn View>>,
}

impl Position {
    pub fn new() -> Self {
        Self {
            label: None,
            left: None,
            top: None,
            right: None,
            bottom: None,
            content: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn left(mut self, v: f32) -> Self {
        self.left = Some(v);
        self
    }

    pub fn top(mut self, v: f32) -> Self {
        self.top = Some(v);
        self
    }

    pub fn right(mut self, v: f32) -> Self {
        self.right = Some(v);
        self
    }

    pub fn bottom(mut self, v: f32) -> Self {
        self.bottom = Some(v);
        self
    }

    pub fn content(mut self, content: impl View + 'static) -> Self {
        self.content = Some(Box::new(content));
        self
    }
}

impl Default for Position {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Position {
    fn build(&self, ctx: &UiContext) -> WidgetPod {
        let child = self.content.as_ref().map(|c| c.build(ctx));
        let mut pod = WidgetPod::new(
            0usize,
            PositionWidget {
                left: self.left,
                top: self.top,
                right: self.right,
                bottom: self.bottom,
                child,
            },
        );
        if let Some(label) = &self.label {
            pod = pod.with_label(label.clone());
        }
        pod
    }
}

pub struct PositionWidget {
    left: Option<f32>,
    top: Option<f32>,
    right: Option<f32>,
    bottom: Option<f32>,
    child: Option<WidgetPod>,
}

impl PositionWidget {
    fn child_layout(
        &self,
        bounds: [f32; 2],
        child: &WidgetPod,
        ctx: &UiContext,
    ) -> ([f32; 2], Matrix4<f32>) {
        let h_margin = self.left.unwrap_or(0.0) + self.right.unwrap_or(0.0);
        let v_margin = self.top.unwrap_or(0.0) + self.bottom.unwrap_or(0.0);
        let available = [
            (bounds[0] - h_margin).max(0.0),
            (bounds[1] - v_margin).max(0.0),
        ];
        let child_constraints = Constraints::new([0.0, available[0]], [0.0, available[1]]);
        let child_size = child.measure(&child_constraints, ctx);
        let final_size = [
            child_size[0].clamp(0.0, available[0]),
            child_size[1].clamp(0.0, available[1]),
        ];

        let x = match (self.left, self.right) {
            (Some(l), _) => l,
            (None, Some(r)) => bounds[0] - r - final_size[0],
            (None, None) => 0.0,
        };
        let y = match (self.top, self.bottom) {
            (Some(t), _) => t,
            (None, Some(b)) => bounds[1] - b - final_size[1],
            (None, None) => 0.0,
        };

        (
            final_size,
            Matrix4::new_translation(&nalgebra::Vector3::new(x, y, 0.0)),
        )
    }
}

impl Widget for PositionWidget {
    type View = Position;

    fn update(&mut self, view: &Position, ctx: &UiContext) -> WidgetInteractionResult {
        let changed = self.left != view.left
            || self.top != view.top
            || self.right != view.right
            || self.bottom != view.bottom;
        self.left = view.left;
        self.top = view.top;
        self.right = view.right;
        self.bottom = view.bottom;
        let child_changed = reconcile_single_child(&mut self.child, view.content.as_deref(), ctx);
        if changed || child_changed {
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
        if self.child.is_none() {
            return WidgetInteractionResult::NoChange;
        }
        let (child_size, affine) = {
            let child = self.child.as_ref().unwrap();
            self.child_layout(bounds, child, ctx)
        };
        let child_event = event.transform(affine);
        self.child
            .as_mut()
            .unwrap()
            .device_input(child_size, &child_event, ctx)
    }

    fn measure(&self, constraints: &Constraints, ctx: &UiContext) -> [f32; 2] {
        let h_margin = self.left.unwrap_or(0.0) + self.right.unwrap_or(0.0);
        let v_margin = self.top.unwrap_or(0.0) + self.bottom.unwrap_or(0.0);
        let inner = Constraints::new(
            [
                (constraints.min_width() - h_margin).max(0.0),
                (constraints.max_width() - h_margin).max(0.0),
            ],
            [
                (constraints.min_height() - v_margin).max(0.0),
                (constraints.max_height() - v_margin).max(0.0),
            ],
        );
        let child_size = self
            .child
            .as_ref()
            .map(|c| c.measure(&inner, ctx))
            .unwrap_or([0.0, 0.0]);
        [child_size[0] + h_margin, child_size[1] + v_margin]
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        if self.child.is_none() {
            return RenderNode::new();
        }
        let (child_size, affine) = {
            let child = self.child.as_ref().unwrap();
            self.child_layout(bounds, child, ctx)
        };
        let child_node = self.child.as_mut().unwrap().render(child_size, ctx);
        RenderNode::new().add_child(child_node, affine)
    }
}
