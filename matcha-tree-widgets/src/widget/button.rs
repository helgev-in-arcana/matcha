use std::sync::Arc;

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
use renderer::render_node::RenderNode;

use crate::style::Style as _;

// MARK: View

pub struct Button {
    pub label: Option<String>,
    pub content: Box<dyn View>,
    #[cfg(not(target_arch = "wasm32"))]
    pub on_click: Option<Arc<dyn Fn(&UiContext) + Send + Sync>>,
    #[cfg(target_arch = "wasm32")]
    pub on_click: Option<Arc<dyn Fn(&UiContext)>>,
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

    #[cfg(not(target_arch = "wasm32"))]
    pub fn on_click<F>(mut self, f: F) -> Self
    where
        F: Fn(&UiContext) + Send + Sync + 'static,
    {
        self.on_click = Some(Arc::new(f));
        self
    }

    #[cfg(target_arch = "wasm32")]
    pub fn on_click<F>(mut self, f: F) -> Self
    where
        F: Fn(&UiContext) + 'static,
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
    #[cfg(not(target_arch = "wasm32"))]
    on_click: Option<Arc<dyn Fn(&UiContext) + Send + Sync>>,
    #[cfg(target_arch = "wasm32")]
    on_click: Option<Arc<dyn Fn(&UiContext)>>,
    state: ButtonState,
    child: Option<WidgetPod>,
}

impl Widget for ButtonWidget {
    type View = Button;

    fn update(&mut self, view: &Button, ctx: &UiContext) -> WidgetInteractionResult {
        self.on_click = view.on_click.clone();
        reconcile_single_child(&mut self.child, Some(view.content.as_ref()), ctx);
        WidgetInteractionResult::NoChange
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

        if let Some(child) = &mut self.child {
            child.device_input(bounds, event, ctx);
        }

        if state_changed {
            WidgetInteractionResult::RedrawNeeded
        } else {
            WidgetInteractionResult::NoChange
        }
    }

    fn measure(&self, constraints: &Constraints, ctx: &UiContext) -> [f32; 2] {
        self.child
            .as_ref()
            .map(|c| c.measure(constraints, ctx))
            .unwrap_or([0.0, 0.0])
    }

    fn render(&mut self, bounds: [f32; 2], ctx: &UiContext) -> RenderNode {
        let bg_color = match self.state {
            ButtonState::Normal => Color::RgbaF32 {
                r: 0.8,
                g: 0.8,
                b: 0.8,
                a: 1.0,
            },
            ButtonState::Hovered => Color::RgbaF32 {
                r: 0.9,
                g: 0.9,
                b: 0.9,
                a: 1.0,
            },
            ButtonState::Pressed => Color::RgbaF32 {
                r: 0.7,
                g: 0.7,
                b: 0.7,
                a: 1.0,
            },
        };

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
                            label: Some("Button BG Render Encoder"),
                        });
                let bg_style = SolidBox::new(bg_color);
                bg_style.draw(&mut encoder, &style_region, bounds, [0.0, 0.0], ctx);
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
