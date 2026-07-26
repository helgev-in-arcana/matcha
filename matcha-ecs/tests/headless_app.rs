//! Full-pipeline headless tests (resolves `ECS_ARCHITECTURE.md` §13-9): the
//! real `UiEcs`, the real `Adapter` (with its `DeviceEventState` click state
//! machine), headless windows from `matcha-window`'s `headless` feature, and
//! wgpu's noop backend — no GPU, no OS window, no event loop. The test itself
//! plays the event loop, calling the `Adapter` methods in the order the winit
//! interface would.
//!
//! This is one level stronger than `tests/input.rs`, which exercises the
//! extracted pure functions (`RectZPicker::build`/`resolve_click_at`):
//! here a synthetic Moved→Pressed→Released sequence travels the production
//! dispatch path `Adapter::device_event` → `DeviceEventState::process` →
//! `UiEcs::device_event` → `on_pointer_press` → reducer → re-view.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

use gpu_utils::gpu::GpuDescriptor;
use matcha_ecs::{
    model::ModelHandle,
    ui_ecs::{UiCommand, UiEcs},
    view::Scope,
};
use matcha_ecs_widgets::Button;
use matcha_window::{
    adapter::{Adapter, EventLoop},
    event::device_event::{
        mouse_input::PhysicalMouseButton, DeviceEvent, DeviceEventData, ElementState, MouseInput,
    },
    headless_interface::{HeadlessEventLoop, HeadlessEventLoopProxy},
    window::WindowId,
};

// ---------------------------------------------------------------------------
// Test app: one 120x40 button at window origin, counting clicks and view runs
// ---------------------------------------------------------------------------

/// Interior-mutable counters so the test can observe the model after it has
/// been moved into the world as a resource.
#[derive(Clone)]
struct Model {
    count: Arc<AtomicI32>,
    view_runs: Arc<AtomicUsize>,
}

impl Model {
    fn new() -> Self {
        Self {
            count: Arc::new(AtomicI32::new(0)),
            view_runs: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Msg {
    Inc,
}

fn view(model: &Model, s: &mut Scope) {
    model.view_runs.fetch_add(1, Ordering::Relaxed);
    // Default Button geometry: 120x40, arranged at window origin [0, 0].
    s.leaf(Button::new("+").on(Msg::Inc));
}

fn reduce(model: &mut Model, msg: Msg) {
    match msg {
        Msg::Inc => {
            model.count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

type TestApp = UiEcs<Model, Msg, fn(&Model, &mut Scope), fn(&mut Model, Msg)>;

struct Harness {
    adapter: Adapter<TestApp>,
    event_loop: HeadlessEventLoop,
    proxy: HeadlessEventLoopProxy<TestApp>,
    handle: ModelHandle<Model>,
    window_id: WindowId,
    count: Arc<AtomicI32>,
    view_runs: Arc<AtomicUsize>,
}

/// Boots the app the way `winit_interface` would: init → resumed →
/// create_surface → one render. The render is required before any click: the
/// picker's acceleration structure is built by `update_picker` in
/// `MatchaSet::PreExtract`, which only runs inside the render schedule.
fn boot() -> Harness {
    let model = Model::new();
    let count = model.count.clone();
    let view_runs = model.view_runs.clone();

    let app: TestApp = UiEcs::new_with_gpu(model, view, reduce, GpuDescriptor::noop());
    let handle = app.model_handle();
    let mut adapter = Adapter::new(app);
    let event_loop = HeadlessEventLoop::new();
    let proxy = HeadlessEventLoopProxy::new();

    adapter.init(Box::new(proxy.clone()), &event_loop);
    adapter.resumed(&event_loop);
    adapter.create_surface(&event_loop);

    let window_id = *event_loop
        .created_window_ids()
        .first()
        .expect("UiEcs::resumed creates exactly one window through the event loop");
    adapter.render(window_id);

    Harness {
        adapter,
        event_loop,
        proxy,
        handle,
        window_id,
        count,
        view_runs,
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

impl Harness {
    /// Move → press → release through `Adapter::device_event`, running the
    /// real click state machine (`on_click` fires on the press edge).
    fn click(&mut self, position: [f32; 2]) {
        self.adapter.device_event(
            &self.event_loop,
            self.window_id,
            mouse_input(MouseInput::Moved { position }),
        );
        self.adapter.device_event(
            &self.event_loop,
            self.window_id,
            mouse_input(MouseInput::ButtonInput {
                state: ElementState::Pressed(0),
                button: PhysicalMouseButton::Left,
            }),
        );
        self.adapter.device_event(
            &self.event_loop,
            self.window_id,
            mouse_input(MouseInput::ButtonInput {
                state: ElementState::Released(0),
                button: PhysicalMouseButton::Left,
            }),
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The full production roundtrip: synthetic click → hit-test → reducer →
/// re-view. `view_runs` starts at 1 (`resumed`) and the hit bumps it to 2.
#[test]
fn click_on_button_reaches_reducer_and_reruns_view() {
    let mut h = boot();
    assert_eq!(h.view_runs.load(Ordering::Relaxed), 1);

    h.click([10.0, 10.0]);

    assert_eq!(h.count.load(Ordering::Relaxed), 1);
    assert_eq!(h.view_runs.load(Ordering::Relaxed), 2);
}

/// A click far from the button resolves to no target: no reducer call, no
/// re-view.
#[test]
fn click_outside_button_is_a_no_op() {
    let mut h = boot();

    h.click([600.0, 400.0]);

    assert_eq!(h.count.load(Ordering::Relaxed), 0);
    assert_eq!(h.view_runs.load(Ordering::Relaxed), 1);
}

/// A move alone must not trigger a click (the state machine only emits
/// `Click` on a button press).
#[test]
fn mouse_move_alone_does_not_click() {
    let mut h = boot();

    let position = [10.0, 10.0];
    h.adapter.device_event(
        &h.event_loop,
        h.window_id,
        mouse_input(MouseInput::Moved { position }),
    );

    assert_eq!(h.count.load(Ordering::Relaxed), 0);
    assert_eq!(h.view_runs.load(Ordering::Relaxed), 1);
}

/// `ModelHandle::update` wakes the (headless) proxy exactly once; feeding the
/// drained command back into `Adapter::ui_command` applies the mutation and
/// re-runs the view — the same delivery a real event loop performs.
#[test]
fn model_handle_roundtrips_through_proxy_and_ui_command() {
    let mut h = boot();

    h.handle.update(|m| {
        m.count.fetch_add(10, Ordering::Relaxed);
    });
    // The mutation is queued, not yet applied.
    assert_eq!(h.count.load(Ordering::Relaxed), 0);

    let commands = h.proxy.drain_commands();
    assert_eq!(commands, vec![UiCommand::ModelUpdated]);
    for command in commands {
        h.adapter.ui_command(&h.event_loop, command);
    }

    assert_eq!(h.count.load(Ordering::Relaxed), 10);
    assert_eq!(h.view_runs.load(Ordering::Relaxed), 2);
}

/// Surfaceless rendering is safe: repeated renders (including for a bogus
/// window id) skip the frame without panicking, and input keeps working
/// afterwards.
#[test]
fn surfaceless_render_skips_frames_safely() {
    let mut h = boot();

    for _ in 0..3 {
        h.adapter.render(h.window_id);
    }
    // A window id the app never created: build_snapshot rejects it.
    let bogus = h
        .event_loop
        .create_window(&matcha_window::window::WindowConfig::default())
        .expect("headless window creation cannot fail")
        .id();
    h.adapter.render(bogus);

    h.click([10.0, 10.0]);
    assert_eq!(h.count.load(Ordering::Relaxed), 1);
}

/// Focus travels the same production path as the click, from the same single
/// pick: `Adapter::device_event` → `UiEcs::on_pointer_press` →
/// `resolve_pointer_press`. `Button` opts into `FocusPolicy::Normal`, so the
/// press both fires the reducer and moves focus onto the button.
#[test]
fn click_on_button_also_focuses_it() {
    let mut h = boot();
    assert_eq!(h.adapter.app().focus().top(), None, "nothing focused at boot");

    h.click([10.0, 10.0]);

    let focus = h.adapter.app().focus();
    let focused = focus.top().expect("the button took focus");
    assert!(focus.is_focused(focused));
    // The window root is the path's first element and is focus-within.
    assert_eq!(
        focus.path().last().copied(),
        Some(focused),
        "the vertex is the deepest entity on the path"
    );
    assert!(focus.path().len() >= 2, "the path includes the window root");
    assert!(focus.is_focus_within(focus.path()[0]));
}

/// Pressing on empty background clears focus (the default `clear_on_miss`),
/// and does so without running the reducer or re-running the view: focus is
/// ECS state, not model state.
#[test]
fn click_on_background_clears_focus_without_touching_the_model() {
    let mut h = boot();
    h.click([10.0, 10.0]);
    assert!(h.adapter.app().focus().top().is_some());

    let count_before = h.count.load(Ordering::Relaxed);
    let views_before = h.view_runs.load(Ordering::Relaxed);

    // Well outside the 120x40 button.
    h.click([600.0, 400.0]);

    assert_eq!(h.adapter.app().focus().top(), None, "focus cleared");
    assert_eq!(h.count.load(Ordering::Relaxed), count_before, "model untouched");
    assert_eq!(
        h.view_runs.load(Ordering::Relaxed),
        views_before,
        "a focus-only change must not re-run the view"
    );
}
