use std::sync::Arc;

use crate::buffer::Buffer;
use crate::layout::reconcile_single_child;
use crate::style::solid_box::SolidBox;
use matcha_tree::color::Color;
use matcha_tree::event::device_event::ElementState;
use matcha_tree::event::device_event::MouseLogicalButton;
use matcha_tree::event::device_event::mouse_input::MouseInput;
use matcha_tree::event::device_event::{DeviceEvent, DeviceEventData};
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::Constraints,
    widget::{View, Widget, WidgetInteractionResult, WidgetPod},
};
use render_interface::Draw;

// MARK: View

/// Closure trait invoked when a button is clicked.
/// `MaybeSendSync` supplies the platform-conditional `Send + Sync` bound.
pub trait ClickFn: for<'a> Fn(&'a UiContext) + utils::MaybeSendSync {}
impl<F> ClickFn for F where F: for<'a> Fn(&'a UiContext) + utils::MaybeSendSync {}

pub struct Button {
    pub label: Option<String>,
    pub content: Box<dyn View>,
    pub on_click: Option<Arc<dyn ClickFn>>,
}

impl Button {
    pub fn new(content: impl View + 'static) -> Self {
        Self {
            label: None,
            content: Box::new(content),
            on_click: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn on_click<F>(mut self, f: F) -> Self
    where
        F: Fn(&UiContext) + utils::MaybeSendSync + 'static,
    {
        self.on_click = Some(Arc::new(f));
        self
    }
}

impl View for Button {
    fn build(&self, ctx: &UiContext) -> WidgetPod {
        let child = self.content.build(ctx);
        let mut pod = WidgetPod::new(
            0usize,
            ButtonWidget {
                on_click: self.on_click.clone(),
                state: ButtonState::Normal,
                decoration: None,
                child: Some(child),
            },
        );
        if let Some(label) = &self.label {
            pod = pod.with_label(label.clone());
        }
        pod
    }
}

// MARK: Widget

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ButtonState {
    Normal,
    Hovered,
    Pressed,
}

pub struct ButtonWidget {
    decoration: Option<(ButtonState, Buffer)>,
    on_click: Option<Arc<dyn ClickFn>>,
    state: ButtonState,
    child: Option<WidgetPod>,
}

impl Widget for ButtonWidget {
    type View = Button;

    fn update(&mut self, view: &Button, ctx: &UiContext) -> WidgetInteractionResult {
        self.on_click = view.on_click.clone();
        reconcile_single_child(&mut self.child, Some(view.content.as_ref()), ctx)
    }

    fn device_input(
        &mut self,
        bounds: [f32; 2],
        event: &DeviceEvent,
        ctx: &UiContext,
    ) -> WidgetInteractionResult {
        let position = event.mouse_position().unwrap_or([-1.0, -1.0]);
        let is_inside = position[0] >= 0.0
            && position[0] <= bounds[0]
            && position[1] >= 0.0
            && position[1] <= bounds[1];

        let mut new_state = self.state;
        let mut clicked = false;

        match event.event() {
            DeviceEventData::MouseInput {
                event: Some(mouse_event),
                ..
            } => match mouse_event {
                MouseInput::Click {
                    click_state,
                    button,
                } => {
                    if *button == MouseLogicalButton::Primary {
                        if is_inside {
                            if matches!(click_state, ElementState::Pressed(_)) {
                                new_state = ButtonState::Pressed;
                            } else if matches!(click_state, ElementState::Released(_))
                                && self.state == ButtonState::Pressed
                            {
                                new_state = ButtonState::Hovered;
                                clicked = true;
                            }
                        } else {
                            new_state = ButtonState::Normal;
                        }
                    }
                }
                _ => {
                    if is_inside {
                        if self.state == ButtonState::Normal {
                            new_state = ButtonState::Hovered;
                        }
                    } else {
                        new_state = ButtonState::Normal;
                    }
                }
            },
            DeviceEventData::MouseInput { event: None, .. } => {
                if is_inside {
                    if self.state == ButtonState::Normal {
                        new_state = ButtonState::Hovered;
                    }
                } else {
                    new_state = ButtonState::Normal;
                }
            }
            _ => {}
        }

        let state_changed = new_state != self.state;
        self.state = new_state;

        if clicked {
            if let Some(f) = &self.on_click {
                f(ctx);
            }
        }

        let child_result = self
            .child
            .as_mut()
            .map(|child| child.device_input(bounds, event, ctx))
            .unwrap_or(WidgetInteractionResult::NoChange);
        if matches!(child_result, WidgetInteractionResult::LayoutNeeded) {
            child_result
        } else if state_changed {
            WidgetInteractionResult::RedrawNeeded
        } else {
            child_result
        }
    }

    fn measure(&self, constraints: &Constraints, ctx: &UiContext) -> [f32; 2] {
        self.child
            .as_ref()
            .map(|c| c.measure(constraints, ctx))
            .unwrap_or([0.0, 0.0])
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext, draw: &mut Draw<'_>) {
        if self
            .decoration
            .as_ref()
            .is_none_or(|(state, _)| *state != self.state)
        {
            let v = match self.state {
                ButtonState::Normal => 0.8,
                ButtonState::Hovered => 0.9,
                ButtonState::Pressed => 0.7,
            };
            let style = SolidBox::new(Color::RgbaF32 {
                r: v,
                g: v,
                b: v,
                a: 1.0,
            });
            self.decoration = Some((self.state, Buffer::clipped(vec![Arc::new(style)])));
        }
        self.decoration.as_mut().unwrap().1.paint(bounds, ctx, draw);
        if let Some(child) = &mut self.child {
            child.render(bounds, ctx, draw);
        }
    }
}
