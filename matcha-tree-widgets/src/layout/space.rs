use matcha_tree::{
    event::device_event::DeviceEvent,
    ui_tree::{
        context::UiContext,
        metrics::Constraints,
        widget::{View, Widget, WidgetInteractionResult, WidgetPod},
    },
};
use renderer::render_node::RenderNode;

use crate::types::size::{ChildSize, Size};

pub struct Space {
    pub label: Option<String>,
    pub width: Size,
    pub height: Size,
}

impl Space {
    pub fn new() -> Self {
        Self {
            label: None,
            width: Size::px(0.0),
            height: Size::px(0.0),
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn width(mut self, width: Size) -> Self {
        self.width = width;
        self
    }

    pub fn height(mut self, height: Size) -> Self {
        self.height = height;
        self
    }
}

impl Default for Space {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Space {
    fn build(&self, _ctx: &UiContext) -> WidgetPod {
        let mut pod = WidgetPod::new(
            0usize,
            SpaceWidget {
                width: self.width.clone(),
                height: self.height.clone(),
            },
        );
        if let Some(label) = &self.label {
            pod = pod.with_label(label.clone());
        }
        pod
    }
}

pub struct SpaceWidget {
    width: Size,
    height: Size,
}

impl Widget for SpaceWidget {
    type View = Space;

    fn update(&mut self, view: &Space, _ctx: &UiContext) -> WidgetInteractionResult {
        let changed = self.width != view.width || self.height != view.height;
        self.width = view.width.clone();
        self.height = view.height.clone();
        if changed {
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

    fn measure(&self, constraints: &Constraints, ctx: &UiContext) -> [f32; 2] {
        let parent_size = [constraints.max_width(), constraints.max_height()];
        let mut child_size = ChildSize::with_size([0.0, 0.0]);
        let w = self.width.size(parent_size, &mut child_size, ctx);
        let h = self.height.size(parent_size, &mut child_size, ctx);
        [
            w.clamp(constraints.min_width(), constraints.max_width()),
            h.clamp(constraints.min_height(), constraints.max_height()),
        ]
    }

    fn render(&mut self, _bounds: [f32; 2], _ctx: &UiContext) -> RenderNode {
        RenderNode::new()
    }
}
