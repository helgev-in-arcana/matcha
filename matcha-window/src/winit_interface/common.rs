use crate::{
    adapter::{Adapter, ControlFlow, EventLoop, EventLoopCommand, EventLoopProxy},
    application::Application,
    event::device_event::{ElementState, KeyInput, KeyboardState},
    window::{WindowConfig, WindowError, WindowSurface, WindowId},
};

// ---------------------------------------------------------------------------
// WinitInterface
// ---------------------------------------------------------------------------

pub(crate) struct WinitInterface<App: Application> {
    pub(super) adapter: Adapter<App>,
    pub(super) event_loop_proxy: winit::event_loop::EventLoopProxy<WinitUserMessage<App>>,
}

// ---------------------------------------------------------------------------
// winit::application::ApplicationHandler impl
// ---------------------------------------------------------------------------

impl<App: Application> winit::application::ApplicationHandler<WinitUserMessage<App>>
    for WinitInterface<App>
{
    fn new_events(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        cause: winit::event::StartCause,
    ) {
        match cause {
            winit::event::StartCause::Init => {
                self.adapter
                    .init(self.event_loop_proxy.clone_box(), event_loop);
            }
            winit::event::StartCause::Poll => {
                self.adapter.poll(event_loop);
            }
            winit::event::StartCause::ResumeTimeReached {
                start,
                requested_resume,
            } => {
                self.adapter
                    .resume_time_reached(event_loop, start, requested_resume);
            }
            winit::event::StartCause::WaitCancelled {
                start,
                requested_resume,
            } => {
                self.adapter
                    .wait_cancelled(event_loop, start, requested_resume);
            }
        }
    }

    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.adapter.resumed(event_loop);
        self.adapter.create_surface(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        winit_window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let window_id = WindowId::from(winit_window_id);

        match event {
            // --------------
            // Redraw request
            // --------------
            winit::event::WindowEvent::RedrawRequested => {
                self.adapter.render(window_id);
            }

            // --------------
            // Window lifecycle
            // --------------
            winit::event::WindowEvent::CloseRequested => {
                let e = crate::event::window_event::WindowEvent::CloseRequested;
                self.adapter.window_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::Destroyed => {
                self.adapter.window_destroyed(event_loop, window_id);
            }

            winit::event::WindowEvent::Occluded(occluded) => {
                let e = crate::event::window_event::WindowEvent::Occluded(occluded);
                self.adapter.window_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::ActivationTokenDone { .. } => {
                // Not mapped to a custom event yet.
            }

            // --------------
            // Window state
            // --------------
            winit::event::WindowEvent::Resized(physical_size) => {
                let inner = [physical_size.width as f32, physical_size.height as f32];
                let e = crate::event::window_event::WindowEvent::Resized {
                    inner_size: inner,
                    outer_size: inner,
                };
                self.adapter.window_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::Moved(physical_position) => {
                let pos = [physical_position.x as f32, physical_position.y as f32];
                let e = crate::event::window_event::WindowEvent::Moved {
                    inner_position: pos,
                    outer_position: pos,
                };
                self.adapter.window_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::Focused(focused) => {
                let e = crate::event::window_event::WindowEvent::Focus(focused);
                self.adapter.window_event(event_loop, window_id, e);
            }

            // --------------
            // Mouse events
            // --------------
            winit::event::WindowEvent::CursorMoved { position, .. } => {
                let pos = [position.x as f32, position.y as f32];
                let e = crate::event::device_event::DeviceEvent::stateless(
                    crate::event::device_event::DeviceEventData::MouseInput {
                        dragging_from_primary: None,
                        dragging_from_secondary: None,
                        dragging_from_middle: None,
                        event: Some(crate::event::device_event::MouseInput::Moved {
                            position: pos,
                        }),
                    },
                );
                self.adapter.device_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::CursorEntered { .. } => {
                let e = crate::event::device_event::DeviceEvent::stateless(
                    crate::event::device_event::DeviceEventData::MouseInput {
                        dragging_from_primary: None,
                        dragging_from_secondary: None,
                        dragging_from_middle: None,
                        event: Some(crate::event::device_event::MouseInput::Entered),
                    },
                );
                self.adapter.device_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::CursorLeft { .. } => {
                let e = crate::event::device_event::DeviceEvent::stateless(
                    crate::event::device_event::DeviceEventData::MouseInput {
                        dragging_from_primary: None,
                        dragging_from_secondary: None,
                        dragging_from_middle: None,
                        event: Some(crate::event::device_event::MouseInput::Left),
                    },
                );
                self.adapter.device_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::MouseWheel { delta, .. } => {
                let converted_delta = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => {
                        crate::event::device_event::mouse_input::ScrollDelta::LineDelta(x, y)
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        crate::event::device_event::mouse_input::ScrollDelta::PixelDelta([
                            pos.x as f32,
                            pos.y as f32,
                        ])
                    }
                };
                let e = crate::event::device_event::DeviceEvent::stateless(
                    crate::event::device_event::DeviceEventData::MouseInput {
                        dragging_from_primary: None,
                        dragging_from_secondary: None,
                        dragging_from_middle: None,
                        event: Some(crate::event::device_event::MouseInput::ScrollRaw {
                            delta: converted_delta,
                        }),
                    },
                );
                self.adapter.device_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::MouseInput { button, state, .. } => {
                if let Some(custom_button) = map_mouse_button(button) {
                    let custom_state = match state {
                        winit::event::ElementState::Pressed => ElementState::Pressed(0),
                        winit::event::ElementState::Released => ElementState::Released(0),
                    };
                    let e = crate::event::device_event::DeviceEvent::stateless(
                        crate::event::device_event::DeviceEventData::MouseInput {
                            dragging_from_primary: None,
                            dragging_from_secondary: None,
                            dragging_from_middle: None,
                            event: Some(crate::event::device_event::MouseInput::ButtonInput {
                                state: custom_state,
                                button: custom_button,
                            }),
                        },
                    );
                    self.adapter.device_event(event_loop, window_id, e);
                }
            }

            // --------------
            // File drag and drop
            // --------------
            winit::event::WindowEvent::DroppedFile(path_buf) => {
                let e = crate::event::device_event::DeviceEvent::stateless(
                    crate::event::device_event::DeviceEventData::FileDrop { path_buf },
                );
                self.adapter.device_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::HoveredFile(path_buf) => {
                let e = crate::event::device_event::DeviceEvent::stateless(
                    crate::event::device_event::DeviceEventData::FileHover { path_buf },
                );
                self.adapter.device_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::HoveredFileCancelled => {
                let e = crate::event::device_event::DeviceEvent::stateless(
                    crate::event::device_event::DeviceEventData::FileHoverCancelled,
                );
                self.adapter.device_event(event_loop, window_id, e);
            }

            // --------------
            // Keyboard events
            // --------------
            winit::event::WindowEvent::KeyboardInput { event, .. } => {
                let key_input = map_key_input(event);
                let e = crate::event::device_event::DeviceEvent::stateless(
                    crate::event::device_event::DeviceEventData::Keyboard(key_input),
                );
                self.adapter.device_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::ModifiersChanged(modifiers) => {
                let e = crate::event::device_event::DeviceEvent::stateless(
                    crate::event::device_event::DeviceEventData::ModifiersChanged(
                        super::keyboard::map_modifiers(modifiers.state()),
                    ),
                );
                self.adapter.device_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::Ime(ime) => {
                use crate::event::device_event::ImeEvent;
                let ime_event = match ime {
                    winit::event::Ime::Enabled => ImeEvent::Enabled,
                    winit::event::Ime::Preedit(text, cursor) => ImeEvent::Preedit { text, cursor },
                    winit::event::Ime::Commit(text) => ImeEvent::Commit { text },
                    winit::event::Ime::Disabled => ImeEvent::Disabled,
                };
                let e = crate::event::device_event::DeviceEvent::stateless(
                    crate::event::device_event::DeviceEventData::Ime(ime_event),
                );
                self.adapter.device_event(event_loop, window_id, e);
            }

            // --------------
            // UI events
            // --------------
            winit::event::WindowEvent::ThemeChanged(theme) => {
                let custom_theme = crate::window::window_config::Theme::from(theme);
                let e = crate::event::window_event::WindowEvent::Theme(custom_theme);
                self.adapter.window_event(event_loop, window_id, e);
            }

            winit::event::WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let e =
                    crate::event::window_event::WindowEvent::ScaleFactorChanged { scale_factor };
                self.adapter.window_event(event_loop, window_id, e);
            }

            // --------------
            // Touch / gesture / touchpad / axis — not mapped yet
            // --------------
            winit::event::WindowEvent::Touch(_)
            | winit::event::WindowEvent::PinchGesture { .. }
            | winit::event::WindowEvent::PanGesture { .. }
            | winit::event::WindowEvent::DoubleTapGesture { .. }
            | winit::event::WindowEvent::RotationGesture { .. }
            | winit::event::WindowEvent::TouchpadPressure { .. }
            | winit::event::WindowEvent::AxisMotion { .. } => {
                // Not mapped yet.
            }
        }
    }

    fn user_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        event: WinitUserMessage<App>,
    ) {
        match event {
            WinitUserMessage::AppCommand { command } => {
                self.adapter.ui_command(event_loop, command);
            }
            WinitUserMessage::EventLoopCommand { cmd } => match cmd {
                EventLoopCommand::Exit => event_loop.exit(),
                EventLoopCommand::SetControlFlow(cf) => {
                    event_loop.set_control_flow(cf.into());
                }
            },
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        _event: winit::event::DeviceEvent,
    ) {
        // Raw device events are not yet mapped to custom types.
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.adapter.about_to_wait(event_loop);
    }

    fn suspended(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.adapter.destroy_surface(event_loop);
        self.adapter.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.adapter.exiting(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.adapter.memory_warning(event_loop);
    }
}

// ---------------------------------------------------------------------------
// User message type
// ---------------------------------------------------------------------------

pub(crate) enum WinitUserMessage<App: Application> {
    AppCommand { command: App::Command },
    EventLoopCommand { cmd: EventLoopCommand },
}

// ---------------------------------------------------------------------------
// Trait impls for winit types
// ---------------------------------------------------------------------------

impl EventLoop for winit::event_loop::ActiveEventLoop {
    fn create_window(&self, config: &WindowConfig) -> Result<WindowSurface, WindowError> {
        WindowSurface::new(self, config).map_err(|e| WindowError::BackendError(e.to_string()))
    }

    fn set_control_flow(&self, control_flow: ControlFlow) {
        self.set_control_flow(control_flow.into());
    }

    fn control_flow(&self) -> ControlFlow {
        self.control_flow().into()
    }

    fn exit(&self) {
        self.exit()
    }

    fn exiting(&self) -> bool {
        self.exiting()
    }
}

impl<App: Application> EventLoopProxy<App>
    for winit::event_loop::EventLoopProxy<WinitUserMessage<App>>
{
    fn clone_box(&self) -> Box<dyn EventLoopProxy<App>> {
        Box::new(Clone::clone(self))
    }

    fn send_command(&self, command: App::Command) {
        let _ = self.send_event(WinitUserMessage::AppCommand { command });
    }

    fn request_exit(&self) {
        let _ = self.send_event(WinitUserMessage::EventLoopCommand {
            cmd: EventLoopCommand::Exit,
        });
    }

    fn request_control_flow(&self, control_flow: ControlFlow) {
        let _ = self.send_event(WinitUserMessage::EventLoopCommand {
            cmd: EventLoopCommand::SetControlFlow(control_flow),
        });
    }
}

// ---------------------------------------------------------------------------
// Mapping helpers (winit → custom types)
// ---------------------------------------------------------------------------

fn map_mouse_button(
    button: winit::event::MouseButton,
) -> Option<crate::event::device_event::mouse_input::PhysicalMouseButton> {
    use crate::event::device_event::mouse_input::PhysicalMouseButton as P;
    use winit::event::MouseButton as W;
    match button {
        W::Left => Some(P::Left),
        W::Right => Some(P::Right),
        W::Middle => Some(P::Middle),
        W::Back => Some(P::Back),
        W::Forward => Some(P::Forward),
        W::Other(b) => Some(P::Other(b)),
    }
}

fn map_key_input(key_event: winit::event::KeyEvent) -> KeyInput {
    let state = match key_event.state {
        winit::event::ElementState::Pressed => ElementState::Pressed(0),
        winit::event::ElementState::Released => ElementState::Released(0),
    };
    KeyInput {
        physical_key: super::keyboard::map_code(key_event.physical_key),
        logical_key: super::keyboard::map_key(key_event.logical_key),
        text: key_event.text.map(|s| s.to_string()),
        location: super::keyboard::map_location(key_event.location),
        state,
        repeat: key_event.repeat,
        snapshot: KeyboardState::default(),
    }
}

// ---------------------------------------------------------------------------
// Type mapping
// ---------------------------------------------------------------------------

impl From<crate::adapter::ControlFlow> for winit::event_loop::ControlFlow {
    fn from(control_flow: crate::adapter::ControlFlow) -> Self {
        match control_flow {
            crate::adapter::ControlFlow::Wait => winit::event_loop::ControlFlow::Wait,
            crate::adapter::ControlFlow::Poll => winit::event_loop::ControlFlow::Poll,
            crate::adapter::ControlFlow::WaitUntil(instant) => {
                winit::event_loop::ControlFlow::WaitUntil(instant)
            }
        }
    }
}

impl From<winit::event_loop::ControlFlow> for crate::adapter::ControlFlow {
    fn from(control_flow: winit::event_loop::ControlFlow) -> Self {
        match control_flow {
            winit::event_loop::ControlFlow::Wait => crate::adapter::ControlFlow::Wait,
            winit::event_loop::ControlFlow::Poll => crate::adapter::ControlFlow::Poll,
            winit::event_loop::ControlFlow::WaitUntil(instant) => {
                crate::adapter::ControlFlow::WaitUntil(instant)
            }
        }
    }
}
