use std::time::Duration;

use matcha_window::adapter::ControlFlow;
use matcha_window::application::Application;
use web_time::Instant;

use super::*;
use crate::ui_tree::widget::PollResult;

struct State {
    next_poll: ControlFlow,
    deadline_update: UpdateResult,
    update: UpdateResult,
    input_request: Option<ControlFlow>,
    polls: Vec<Instant>,
    layouts: Vec<Instant>,
    layouts_at_input: Vec<usize>,
    renders: Vec<Instant>,
}

impl State {
    fn new(next_poll: ControlFlow) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            next_poll,
            deadline_update: UpdateResult::RENDER,
            update: UpdateResult::empty(),
            input_request: None,
            polls: Vec::new(),
            layouts: Vec::new(),
            layouts_at_input: Vec::new(),
            renders: Vec::new(),
        }))
    }
}

#[derive(Clone)]
struct Probe(Arc<Mutex<State>>);

impl View for Probe {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, _: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        self.clone()
    }
}

impl Widget for Probe {
    type V = Self;

    fn update<'a>(&mut self, view: &'a Self, _: &mut DeclContext<'a, '_>) -> UpdateResult {
        self.clone_from(view);
        UpdateResult::empty()
    }

    fn poll(&mut self, ctx: &mut Context) -> PollResult {
        let now = ctx.frame_time();
        let mut state = self.0.lock().unwrap();
        state.polls.push(now);
        let mut update = std::mem::take(&mut state.update);
        if matches!(state.next_poll, ControlFlow::WaitUntil(deadline) if deadline <= now) {
            state.next_poll = ControlFlow::Wait;
            update |= state.deadline_update;
        }
        PollResult {
            update,
            next_poll: state.next_poll,
        }
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        let mut state = self.0.lock().unwrap();
        let layouts = state.layouts.len();
        state.layouts_at_input.push(layouts);
        if let Some(next) = state.input_request.take() {
            state.next_poll = next;
        }
        InputResult::default()
    }

    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds::default()
    }

    fn layout(&self, _: Bounds, _: Children<'_>, ctx: &mut Context) {
        self.0.lock().unwrap().layouts.push(ctx.frame_time());
    }

    fn render(&self, _: Bounds, ctx: &mut Context) -> Draw {
        self.0.lock().unwrap().renders.push(ctx.frame_time());
        Draw::default()
    }
}

struct Probes(Vec<Window<Probe>>);

impl Probes {
    fn declare<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) {
        for window in &self.0 {
            ctx.child(window);
        }
    }
}

impl View for Probes {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        self.declare(ctx);
        ProbesWidget
    }
}

struct ProbesWidget;

impl Widget for ProbesWidget {
    type V = Probes;

    fn update<'a>(&mut self, view: &'a Probes, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        view.declare(ctx);
        UpdateResult::empty()
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }

    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds::default()
    }

    fn layout(&self, _: Bounds, _: Children<'_>, _: &mut Context) {}

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        Draw::default()
    }
}

struct ProbeModel(Vec<Arc<Mutex<State>>>);

impl Model for ProbeModel {
    fn view(&mut self, _: &ModelContext<'_>) -> impl View {
        Probes(
            self.0
                .iter()
                .enumerate()
                .map(|(index, state)| {
                    Window::new(Probe(Arc::clone(state)))
                        .with_key(index as u64)
                        .with_config(WindowConfig {
                            inner_size: Some(Apply::Init([64, 64].into())),
                            ..WindowConfig::default()
                        })
                })
                .collect(),
        )
    }
}

fn setup(states: &[Arc<Mutex<State>>]) -> (App<ProbeModel>, HeadlessEventLoop) {
    let event_loop = HeadlessEventLoop::new();
    let mut app = App::new(ProbeModel(states.to_vec()), noop_gpu());
    app.init(Box::new(HeadlessEventLoopProxy::new()), &event_loop);
    app.create_surface(&event_loop);
    (app, event_loop)
}

#[test]
fn early_resize_and_redraw_keep_the_absolute_deadline() {
    let deadline = Instant::now() + Duration::from_secs(3_600);
    let state = State::new(ControlFlow::WaitUntil(deadline));
    let (mut app, event_loop) = setup(&[Arc::clone(&state)]);
    let window = event_loop.created_window_ids()[0];
    app.window_event(
        &event_loop,
        window,
        WindowEvent::Resized {
            inner_size: [64.0; 2],
            outer_size: [64.0; 2],
        },
    );
    app.render(window);
    app.render(window);
    app.about_to_wait(&event_loop);
    let state = state.lock().unwrap();
    assert_eq!(state.polls.len(), 3);
    assert!(state.polls.iter().all(|now| *now < deadline));
    assert_eq!(state.layouts.len(), 1);
    assert_eq!(state.renders.len(), 1);
    assert_eq!(event_loop.control_flow(), ControlFlow::WaitUntil(deadline));
}

#[test]
fn deadlines_are_replaced_and_only_due_windows_are_polled() {
    let now = Instant::now();
    let first_deadline = now + Duration::from_secs(3_600);
    let second_deadline = now + Duration::from_secs(7_200);
    let later = now + Duration::from_secs(10_800);
    let first = State::new(ControlFlow::WaitUntil(first_deadline));
    let second = State::new(ControlFlow::WaitUntil(second_deadline));
    let (mut app, event_loop) = setup(&[Arc::clone(&first), Arc::clone(&second)]);
    let windows = event_loop.created_window_ids();
    app.about_to_wait(&event_loop);
    assert_eq!(
        event_loop.control_flow(),
        ControlFlow::WaitUntil(first_deadline)
    );

    first.lock().unwrap().next_poll = ControlFlow::WaitUntil(later);
    assert!(app.poll_window(windows[0], now).is_empty());
    app.about_to_wait(&event_loop);
    assert_eq!(
        event_loop.control_flow(),
        ControlFlow::WaitUntil(second_deadline)
    );
    let first_count = first.lock().unwrap().polls.len();
    app.poll_ready(&event_loop, second_deadline);
    assert_eq!(first.lock().unwrap().polls.len(), first_count);
    assert_eq!(second.lock().unwrap().polls.last(), Some(&second_deadline));
    assert!(second.lock().unwrap().renders.is_empty());
    app.about_to_wait(&event_loop);
    assert_eq!(event_loop.control_flow(), ControlFlow::WaitUntil(later));

    first.lock().unwrap().next_poll = ControlFlow::Wait;
    assert!(app.poll_window(windows[0], now).is_empty());
    app.about_to_wait(&event_loop);
    assert_eq!(event_loop.control_flow(), ControlFlow::Wait);
}

#[test]
fn shared_deadline_only_requests_redraw_for_the_changed_window() {
    let deadline = Instant::now() + Duration::from_secs(3_600);
    let changed = State::new(ControlFlow::WaitUntil(deadline));
    let unchanged = State::new(ControlFlow::WaitUntil(deadline));
    unchanged.lock().unwrap().deadline_update = UpdateResult::empty();
    let (mut app, event_loop) = setup(&[changed.clone(), unchanged.clone()]);
    let windows: [_; 2] = event_loop.created_window_ids().try_into().unwrap();
    let redraw_counts = || windows.map(|id| event_loop.redraw_request_count(id).unwrap());

    // Service initial redraws so both windows start with a cached frame.
    for id in windows {
        app.render(id);
    }
    app.about_to_wait(&event_loop);
    assert_eq!(event_loop.control_flow(), ControlFlow::WaitUntil(deadline));
    let [first, second] = redraw_counts();

    app.poll_ready(&event_loop, deadline);
    for state in [&changed, &unchanged] {
        let state = state.lock().unwrap();
        assert_eq!(state.polls.last(), Some(&deadline));
        assert_eq!(state.renders.len(), 1, "waking must not render inline");
    }
    assert_eq!(redraw_counts(), [first + 1, second]);
    assert_eq!(event_loop.control_flow(), ControlFlow::Wait);

    // Deliver the one redraw requested by the wake.
    app.render(windows[0]);
    app.about_to_wait(&event_loop);
    assert_eq!(changed.lock().unwrap().renders.len(), 2);
    assert_eq!(unchanged.lock().unwrap().renders.len(), 1);
    assert_eq!(redraw_counts(), [first + 1, second]);
    assert_eq!(event_loop.control_flow(), ControlFlow::Wait);
}

#[test]
fn event_loop_wake_callbacks_poll_without_a_render_callback() {
    let state = State::new(ControlFlow::Poll);
    let (mut app, event_loop) = setup(&[Arc::clone(&state)]);
    app.about_to_wait(&event_loop);
    assert_eq!(event_loop.control_flow(), ControlFlow::Poll);
    app.poll(&event_loop);
    let now = Instant::now();
    app.resume_time_reached(&event_loop, now, now);
    app.wait_cancelled(&event_loop, now, None);
    assert_eq!(state.lock().unwrap().polls.len(), 4);
    assert!(state.lock().unwrap().renders.is_empty());
}

#[test]
fn poll_invalidation_is_applied_before_layout_and_render_with_one_timestamp() {
    let state = State::new(ControlFlow::Wait);
    let (mut app, event_loop) = setup(&[Arc::clone(&state)]);
    let window = event_loop.created_window_ids()[0];
    app.render(window);
    state.lock().unwrap().update = UpdateResult::LAYOUT | UpdateResult::RENDER;
    app.render(window);
    let state = state.lock().unwrap();
    assert_eq!(state.layouts.len(), 2);
    assert_eq!(state.renders.len(), 2);
    assert_eq!(state.polls.last(), state.layouts.last());
    assert_eq!(state.polls.last(), state.renders.last());
}

#[test]
fn input_uses_polled_layout_and_discovers_its_new_deadline() {
    let state = State::new(ControlFlow::Wait);
    let (mut app, event_loop) = setup(&[Arc::clone(&state)]);
    let window = event_loop.created_window_ids()[0];
    app.render(window);
    let deadline = Instant::now() + Duration::from_secs(3_600);
    {
        let mut state = state.lock().unwrap();
        assert_eq!(state.layouts.len(), 1);
        state.update = UpdateResult::LAYOUT;
        state.input_request = Some(ControlFlow::WaitUntil(deadline));
    }
    app.device_event(
        &event_loop,
        window,
        DeviceEvent::stateless(DeviceEventData::MouseInput {
            dragging_from_primary: None,
            dragging_from_secondary: None,
            dragging_from_middle: None,
            event: Some(MouseInput::Moved { position: [1.0; 2] }),
        }),
    );
    app.about_to_wait(&event_loop);
    assert_eq!(event_loop.control_flow(), ControlFlow::WaitUntil(deadline));
    let state = state.lock().unwrap();
    assert!(state.input_request.is_none());
    assert_eq!(state.layouts_at_input, [2]);
    assert_eq!(state.layouts.len(), 2);
    assert_eq!(state.polls.len(), 4);
    assert_eq!(state.polls[2], state.polls[3]);
    assert_eq!(state.layouts.last(), state.polls.last());
    assert_eq!(state.renders.len(), 1);
}

#[test]
fn suspension_and_window_removal_discard_requests_and_resume_rediscovers_them() {
    let deadline = Instant::now() + Duration::from_secs(3_600);
    let state = State::new(ControlFlow::WaitUntil(deadline));
    let (mut app, event_loop) = setup(&[Arc::clone(&state)]);
    app.destroy_surface(&event_loop);
    app.poll_ready(&event_loop, deadline);
    app.about_to_wait(&event_loop);
    assert_eq!(state.lock().unwrap().polls.len(), 1);
    assert!(app.poll_requests.is_empty());
    assert_eq!(event_loop.control_flow(), ControlFlow::Wait);

    app.create_surface(&event_loop);
    app.about_to_wait(&event_loop);
    assert_eq!(state.lock().unwrap().polls.len(), 2);
    assert_eq!(event_loop.control_flow(), ControlFlow::WaitUntil(deadline));
    app.model.0.clear();
    app.request.as_ref().unwrap().request();
    app.ui_command(&event_loop, ());
    app.about_to_wait(&event_loop);
    assert!(app.windows.ids().is_empty());
    assert!(app.poll_requests.is_empty());
    assert_eq!(event_loop.control_flow(), ControlFlow::Wait);
}
