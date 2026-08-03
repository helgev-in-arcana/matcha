//! Headless-backend tests: in-memory windows, the test-driven event loop, and
//! the real `DeviceEventState` click state machine — no OS window, no GPU.
//!
//! Run with `cargo test -p matcha-window --features headless`.

use matcha_window::{
    adapter::{Adapter, EventLoop, EventLoopCommand, EventLoopProxy},
    application::Application,
    event::{
        device_event::{DeviceEvent, DeviceEventData, ElementState, ImeEvent, MouseInput},
        raw_device_event::{RawDeviceEvent, RawDeviceId},
        window_event::WindowEvent,
    },
    headless_interface::{HeadlessEventLoop, HeadlessEventLoopProxy},
    window::{Window, WindowConfig, WindowId},
};

// ---------------------------------------------------------------------------
// Recorder application
// ---------------------------------------------------------------------------

/// Records what reaches the `Application` so tests can assert on it.
#[derive(Default)]
struct Recorder {
    clicks: Vec<[f32; 2]>,
    ime: Vec<ImeEvent>,
    device_events: usize,
    commands: Vec<u32>,
}

impl Application for Recorder {
    type Command = u32;

    fn init(&mut self, _proxy: Box<dyn EventLoopProxy<Self>>, _event_loop: &impl EventLoop) {}
    fn resumed(&mut self, _event_loop: &impl EventLoop) {}
    fn create_surface(&mut self, _event_loop: &impl EventLoop) {}
    fn destroy_surface(&mut self, _event_loop: &impl EventLoop) {}
    fn suspended(&mut self, _event_loop: &impl EventLoop) {}
    fn exiting(&mut self, _event_loop: &impl EventLoop) {}
    fn render(&mut self, _window_id: WindowId) {}
    fn window_event(
        &mut self,
        _event_loop: &impl EventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }
    fn window_destroyed(&mut self, _event_loop: &impl EventLoop, _window_id: WindowId) {}

    fn device_event(
        &mut self,
        _event_loop: &impl EventLoop,
        _window_id: WindowId,
        event: DeviceEvent,
    ) {
        self.device_events += 1;
        if event.on_click(|_| ()).is_some() {
            self.clicks.push(event.mouse_viewport_position());
        }
        if let Some(ime) = event.on_ime(|ime| ime.clone()) {
            self.ime.push(ime);
        }
    }

    fn ui_command(&mut self, _event_loop: &impl EventLoop, command: Self::Command) {
        self.commands.push(command);
    }

    fn raw_device_event(
        &mut self,
        _event_loop: &impl EventLoop,
        _raw_device_id: RawDeviceId,
        _raw_event: RawDeviceEvent,
    ) {
    }
}

// ---------------------------------------------------------------------------
// Synthetic events (same shape as winit_interface/common.rs produces)
// ---------------------------------------------------------------------------

fn mouse_input(event: MouseInput) -> DeviceEvent {
    DeviceEvent::stateless(DeviceEventData::MouseInput {
        dragging_from_primary: None,
        dragging_from_secondary: None,
        dragging_from_middle: None,
        event: Some(event),
    })
}

fn mouse_moved(position: [f32; 2]) -> DeviceEvent {
    mouse_input(MouseInput::Moved { position })
}

fn left_button(state: ElementState) -> DeviceEvent {
    mouse_input(MouseInput::ButtonInput {
        state,
        button: matcha_window::event::device_event::mouse_input::PhysicalMouseButton::Left,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn headless_window_reports_in_memory_state() {
    let event_loop = HeadlessEventLoop::new();
    let config = WindowConfig::default()
        .with_title("headless test")
        .with_inner_size([640u32, 480u32]);

    let mut window = Window::new(&config, &event_loop).expect(
        "headless window creation is in-memory only and cannot fail like an OS window would",
    );

    assert_eq!(event_loop.created_window_ids(), vec![window.id()]);
    assert_eq!(window.title(), "headless test");
    assert_eq!(window.inner_size(), [640, 480]);
    assert_eq!(window.outer_size(), [640, 480]);
    assert!((window.dpi() - 1.0).abs() < f64::EPSILON);
    assert!(!window.has_surface());

    // create_surface is a no-op for a headless window: Ok, still surfaceless.
    let gpu = futures::executor::block_on(gpu_utils::gpu::Gpu::new(
        gpu_utils::gpu::GpuDescriptor::noop(),
    ))
    .expect("noop backend adapter/device request should succeed without any GPU");
    let (device, _queue) = gpu
        .context()
        .expect("device/queue are available immediately after Gpu::new");
    window
        .create_surface(gpu.instance(), &device)
        .expect("headless create_surface never fails: it attaches nothing");
    assert!(!window.has_surface());
    assert!(
        window
            .surface()
            .get_surface_texture(&device)
            .expect("surfaceless acquire is the Ok(None) frame-skip path")
            .is_none()
    );
}

#[test]
fn click_state_machine_runs_through_adapter() {
    let event_loop = HeadlessEventLoop::new();
    let mut adapter = Adapter::new(Recorder::default());
    let window_id = event_loop
        .create_window(&WindowConfig::default())
        .expect("headless window creation cannot fail")
        .id();

    adapter.device_event(&event_loop, window_id, mouse_moved([42.0, 24.0]));
    adapter.device_event(&event_loop, window_id, left_button(ElementState::Pressed(0)));
    adapter.device_event(&event_loop, window_id, left_button(ElementState::Released(0)));

    let recorder = adapter.app();
    // `on_click` fires on the press edge only: exactly one click, at the
    // position tracked from the preceding Moved event.
    assert_eq!(recorder.clicks, vec![[42.0, 24.0]]);
}

#[test]
fn move_only_does_not_click() {
    let event_loop = HeadlessEventLoop::new();
    let mut adapter = Adapter::new(Recorder::default());
    let window_id = event_loop
        .create_window(&WindowConfig::default())
        .expect("headless window creation cannot fail")
        .id();

    adapter.device_event(&event_loop, window_id, mouse_moved([10.0, 10.0]));

    let recorder = adapter.app();
    assert!(recorder.clicks.is_empty());
    assert_eq!(recorder.device_events, 1);
}

#[test]
fn proxy_queues_commands_for_the_test_to_drain() {
    let proxy = HeadlessEventLoopProxy::<Recorder>::new();
    let cloned = proxy.clone_box();

    cloned.send_command(7);
    proxy.send_command(8);
    assert_eq!(proxy.drain_commands(), vec![7, 8]);
    assert!(proxy.drain_commands().is_empty());

    cloned.request_exit();
    assert!(matches!(
        proxy.drain_loop_commands().as_slice(),
        [EventLoopCommand::Exit]
    ));
}

// ---------------------------------------------------------------------------
// IME
// ---------------------------------------------------------------------------

fn ime(event: ImeEvent) -> DeviceEvent {
    DeviceEvent::stateless(DeviceEventData::Ime(event))
}

/// A full composition session survives `DeviceEventState::process` intact.
///
/// IME has no state machine of its own — each event is self-contained and the
/// platform IME already tracks the composition — so the state machine must pass
/// them straight through rather than swallowing them the way it does for
/// unmapped event kinds.
#[test]
fn ime_composition_reaches_the_application_through_the_state_machine() {
    let event_loop = HeadlessEventLoop::new();
    let mut adapter = Adapter::new(Recorder::default());
    let window_id = event_loop
        .create_window(&WindowConfig::default())
        .expect("headless window creation cannot fail")
        .id();

    // Typing "にほん" in a Japanese IME: enable, a run of preedits, then commit.
    let session = [
        ImeEvent::Enabled,
        ImeEvent::Preedit {
            text: "に".into(),
            cursor: Some((0, 3)),
        },
        ImeEvent::Preedit {
            text: "にほん".into(),
            cursor: Some((0, 9)),
        },
        ImeEvent::Commit {
            text: "日本".into(),
        },
        ImeEvent::Disabled,
    ];
    for event in &session {
        adapter.device_event(&event_loop, window_id, ime(event.clone()));
    }

    assert_eq!(adapter.app().ime, session, "every step arrives, in order");
}

/// The headless backend has no platform IME, but the control API must still be
/// callable so the same UI code runs headlessly.
#[test]
fn headless_ime_control_api_is_a_no_op() {
    let event_loop = HeadlessEventLoop::new();
    let window = Window::new(&WindowConfig::default(), &event_loop).expect("headless window");

    window.set_ime_allowed(true);
    window.set_ime_cursor_area([10.0, 20.0], [2.0, 16.0]);
    window.set_ime_allowed(false);
}
