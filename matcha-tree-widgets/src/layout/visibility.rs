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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityState {
    Visible,
    Hidden,
    Gone,
}

pub struct Visibility {
    pub label: Option<String>,
    pub visibility: VisibilityState,
    pub content: Option<Box<dyn View>>,
}

impl Visibility {
    pub fn new() -> Self {
        Self {
            label: None,
            visibility: VisibilityState::Visible,
            content: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn visible(mut self) -> Self {
        self.visibility = VisibilityState::Visible;
        self
    }

    pub fn hidden(mut self) -> Self {
        self.visibility = VisibilityState::Hidden;
        self
    }

    pub fn gone(mut self) -> Self {
        self.visibility = VisibilityState::Gone;
        self
    }

    pub fn visibility(mut self, v: VisibilityState) -> Self {
        self.visibility = v;
        self
    }

    pub fn content(mut self, content: impl View + 'static) -> Self {
        self.content = Some(Box::new(content));
        self
    }
}

impl Default for Visibility {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Visibility {
    fn build(&self, ctx: &UiContext) -> WidgetPod {
        let child = self.content.as_ref().map(|c| c.build(ctx));
        let mut pod = WidgetPod::new(
            0usize,
            VisibilityWidget {
                visibility: self.visibility,
                child,
            },
        );
        if let Some(label) = &self.label {
            pod = pod.with_label(label.clone());
        }
        pod
    }
}

pub struct VisibilityWidget {
    visibility: VisibilityState,
    child: Option<WidgetPod>,
}

impl Widget for VisibilityWidget {
    type View = Visibility;

    fn update(&mut self, view: &Visibility, ctx: &UiContext) -> WidgetInteractionResult {
        let vis_changed = self.visibility != view.visibility;
        self.visibility = view.visibility;
        let child_changed = reconcile_single_child(&mut self.child, view.content.as_deref(), ctx);
        if vis_changed || child_changed {
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
        if self.visibility == VisibilityState::Visible {
            if let Some(child) = &mut self.child {
                return child.device_input(bounds, event, ctx);
            }
        }
        WidgetInteractionResult::NoChange
    }

    fn measure(&self, constraints: &Constraints, ctx: &UiContext) -> [f32; 2] {
        match self.visibility {
            VisibilityState::Gone => [0.0, 0.0],
            VisibilityState::Visible | VisibilityState::Hidden => self
                .child
                .as_ref()
                .map(|c| c.measure(constraints, ctx))
                .unwrap_or([0.0, 0.0]),
        }
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        if self.visibility == VisibilityState::Visible {
            if let Some(child) = &mut self.child {
                let child_node = child.render(bounds, ctx);
                return RenderNode::new().add_child(child_node, nalgebra::Matrix4::identity());
            }
        }
        RenderNode::new()
    }
}
