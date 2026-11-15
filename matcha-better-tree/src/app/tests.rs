use std::sync::{Arc, Mutex, OnceLock, mpsc};

use gpu_utils::gpu::{Gpu, GpuDescriptor};
use matcha_window::adapter::{Adapter, EventLoop};
use matcha_window::event::device_event::{DeviceEvent, DeviceEventData, MouseInput};
use matcha_window::event::window_event::WindowEvent;
use matcha_window::headless_interface::{HeadlessEventLoop, HeadlessEventLoopProxy};

use super::{App, Model, ModelContext, UpdateView};
use crate::metrics::{Bounds, Constraints};
use crate::ui_tree::context::Context;
use crate::ui_tree::widget::{Draw, InputResult, UpdateResult, View, Widget};
use crate::ui_tree::window::{Apply, Window, WindowConfig};
use crate::ui_tree::{Children, DeclContext};

fn noop_gpu() -> Gpu {
    futures::executor::block_on(Gpu::new(GpuDescriptor::noop())).unwrap()
}

#[derive(Debug, PartialEq, Eq)]
enum Observed {
    Evaluated(usize),
    Built(usize),
    Updated(usize),
    Rendered(usize),
    CloseRequested,
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
type ButtonInput = dyn Fn() + Send + Sync;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
type ButtonInput = dyn Fn();

#[derive(Clone)]
struct Button {
    value: usize,
    on_input: Arc<ButtonInput>,
    observed: mpsc::Sender<Observed>,
}

impl View for Button {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        ctx.context()
            .insert_resource(self.observed.clone())
            .send(Observed::Built(self.value))
            .unwrap();
        self.clone()
    }
}

impl Widget for Button {
    type V = Self;

    fn update<'a>(&mut self, view: &'a Self, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        self.clone_from(view);
        ctx.context()
            .resource::<mpsc::Sender<Observed>>()
            .expect("app preserves resources across passes")
            .send(Observed::Updated(self.value))
            .unwrap();
        UpdateResult::RENDER
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        (self.on_input)();
        InputResult::default()
    }

    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds::default()
    }

    fn layout(&self, _: Bounds, _: Children<'_>, _: &mut Context) {}

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        self.observed.send(Observed::Rendered(self.value)).unwrap();
        Draw::default()
    }
}

struct WindowRoot(Option<Window<Button>>);

impl WindowRoot {
    fn declare<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) {
        if let Some(window) = &self.0 {
            ctx.child(window);
        }
    }
}

impl View for WindowRoot {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        self.declare(ctx);
        WindowsWidget
    }
}

struct WindowsWidget;

impl Widget for WindowsWidget {
    type V = WindowRoot;

    fn update<'a>(&mut self, view: &'a WindowRoot, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
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

enum Action {
    Increment,
    Close,
}

struct Counter {
    value: usize,
    open: bool,
    sender: mpsc::Sender<Action>,
    pending: Mutex<mpsc::Receiver<Action>>,
    observed: mpsc::Sender<Observed>,
}

impl Counter {
    fn new() -> (Self, mpsc::Receiver<Observed>) {
        let (sender, pending) = mpsc::channel();
        let (observed, events) = mpsc::channel();
        (
            Self {
                value: 0,
                open: true,
                sender,
                pending: Mutex::new(pending),
                observed,
            },
            events,
        )
    }
}

impl Model for Counter {
    fn view(&mut self, ctx: &ModelContext<'_>) -> impl View {
        for action in self.pending.get_mut().unwrap().try_iter() {
            match action {
                Action::Increment => self.value += 1,
                Action::Close => self.open = false,
            }
        }
        self.observed.send(Observed::Evaluated(self.value)).unwrap();
        if !self.open {
            return WindowRoot(None);
        }
        let sender = self.sender.clone();
        let button_update = ctx.update_view().clone();
        let window = Window::new(Button {
            value: self.value,
            on_input: Arc::new(move || {
                sender.send(Action::Increment).unwrap();
                button_update.request();
            }),
            observed: self.observed.clone(),
        })
        .with_config(WindowConfig {
            inner_size: Some(Apply::Init([64, 64].into())),
            ..WindowConfig::default()
        });
        let sender = self.sender.clone();
        let update_view = ctx.update_view().clone();
        let observed = self.observed.clone();
        WindowRoot(Some(window.on_close_requested(move || {
            observed.send(Observed::CloseRequested).unwrap();
            sender.send(Action::Close).unwrap();
            update_view.request();
        })))
    }
}

#[test]
fn close_requests_reach_the_model_until_it_removes_the_window() {
    let (model, observed) = Counter::new();
    let event_loop = HeadlessEventLoop::new();
    let proxy = HeadlessEventLoopProxy::new();
    let mut adapter = Adapter::new(App::new(model, noop_gpu()));
    adapter.init(Box::new(proxy.clone()), &event_loop);
    adapter.resumed(&event_loop);
    adapter.create_surface(&event_loop);
    let [window]: [_; 1] = event_loop.created_window_ids().try_into().unwrap();
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        [Observed::Evaluated(0), Observed::Built(0)]
    );
    for _ in 0..2 {
        adapter.window_event(&event_loop, window, WindowEvent::CloseRequested);
    }
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        [Observed::CloseRequested, Observed::CloseRequested]
    );
    assert!(!event_loop.exiting());
    assert_eq!(proxy.drain_commands(), [()]);

    adapter.ui_command(&event_loop, ());
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        [Observed::Evaluated(0)]
    );
    adapter.window_event(&event_loop, window, WindowEvent::CloseRequested);
    assert!(proxy.drain_commands().is_empty());
    assert_eq!(observed.try_iter().count(), 0);
}

#[test]
fn widget_input_batches_messages_and_updates_the_mounted_widget() {
    let (counter, observed) = Counter::new();
    let event_loop = HeadlessEventLoop::new();
    let proxy = HeadlessEventLoopProxy::new();
    let mut adapter = Adapter::new(App::new(counter, noop_gpu()));
    adapter.init(Box::new(proxy.clone()), &event_loop);
    assert_eq!(observed.try_iter().count(), 0);
    assert!(proxy.drain_commands().is_empty());
    assert!(event_loop.created_window_ids().is_empty());

    adapter.resumed(&event_loop);
    adapter.create_surface(&event_loop);
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        [Observed::Evaluated(0), Observed::Built(0)]
    );
    let [window]: [_; 1] = event_loop.created_window_ids().try_into().unwrap();
    adapter.render(window);
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        [Observed::Rendered(0)]
    );
    for x in [1.0, 2.0, 3.0] {
        let event = DeviceEvent::stateless(DeviceEventData::MouseInput {
            dragging_from_primary: None,
            dragging_from_secondary: None,
            dragging_from_middle: None,
            event: Some(MouseInput::Moved { position: [x, 1.0] }),
        });
        adapter.device_event(&event_loop, window, event);
    }
    assert_eq!(observed.try_iter().count(), 0);
    assert_eq!(proxy.drain_commands(), [()]);
    adapter.ui_command(&event_loop, ());
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        [Observed::Evaluated(3), Observed::Updated(3)]
    );
    adapter.render(window);
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        [Observed::Rendered(3)]
    );
    assert!(proxy.drain_commands().is_empty());
    adapter.ui_command(&event_loop, ());
    assert_eq!(observed.try_iter().count(), 0);
    adapter.destroy_surface(&event_loop);
    adapter.suspended(&event_loop);
    adapter.resumed(&event_loop);
    adapter.create_surface(&event_loop);
    adapter.create_surface(&event_loop);
    assert_eq!(event_loop.created_window_ids(), [window]);
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        [Observed::Evaluated(3), Observed::Updated(3)]
    );
}

#[test]
fn a_recreated_native_window_has_a_fresh_input_route_and_viewport() {
    let (counter, _observed) = Counter::new();
    let event_loop = HeadlessEventLoop::new();
    let proxy = HeadlessEventLoopProxy::new();
    let mut adapter = Adapter::new(App::new(counter, noop_gpu()));
    adapter.init(Box::new(proxy.clone()), &event_loop);
    adapter.create_surface(&event_loop);
    let old = event_loop.created_window_ids()[0];
    adapter
        .app()
        .windows
        .window(old)
        .unwrap()
        .request_inner_size(8, 8);
    adapter.window_event(
        &event_loop,
        old,
        WindowEvent::Resized {
            inner_size: [16.0; 2],
            outer_size: [16.0; 2],
        },
    );
    let config = adapter
        .app()
        .windows
        .window(old)
        .unwrap()
        .surface()
        .surface_config();
    assert_eq!([config.width, config.height], [8, 8]);
    adapter.render(old);
    adapter.window_destroyed(&event_loop, old);
    assert_eq!(proxy.drain_commands(), [()]);
    adapter.ui_command(&event_loop, ());
    let [_, new]: [_; 2] = event_loop.created_window_ids().try_into().unwrap();
    let mouse = DeviceEvent::stateless(DeviceEventData::MouseInput {
        dragging_from_primary: None,
        dragging_from_secondary: None,
        dragging_from_middle: None,
        event: Some(MouseInput::Moved {
            position: [40.0; 2],
        }),
    });
    adapter.device_event(&event_loop, old, mouse.clone());
    assert!(proxy.drain_commands().is_empty());
    adapter.device_event(&event_loop, new, mouse);
    assert_eq!(proxy.drain_commands(), [()]);
}

#[test]
fn requests_during_view_are_delivered_on_the_next_dispatch() {
    struct RequestTwice {
        counter: Counter,
        requests_left: usize,
        handle: Arc<OnceLock<UpdateView>>,
    }

    impl Model for RequestTwice {
        fn view(&mut self, ctx: &ModelContext<'_>) -> impl View {
            self.handle.get_or_init(|| ctx.update_view().clone());
            if self.requests_left > 0 {
                self.requests_left -= 1;
                ctx.update_view().request();
            }
            self.counter.view(ctx)
        }
    }

    let (counter, observed) = Counter::new();
    let handle = Arc::new(OnceLock::new());
    let model = RequestTwice {
        counter,
        requests_left: 2,
        handle: Arc::clone(&handle),
    };
    let event_loop = HeadlessEventLoop::new();
    let proxy = HeadlessEventLoopProxy::new();
    let mut adapter = Adapter::new(App::new(model, noop_gpu()));
    adapter.init(Box::new(proxy.clone()), &event_loop);
    adapter.create_surface(&event_loop);
    for _ in 0..2 {
        assert_eq!(proxy.drain_commands(), [()]);
        adapter.ui_command(&event_loop, ());
    }
    assert!(proxy.drain_commands().is_empty());
    adapter.destroy_surface(&event_loop);
    adapter.suspended(&event_loop);
    for _ in 0..3 {
        handle.get().unwrap().request();
    }
    assert_eq!(proxy.drain_commands(), [()]);
    adapter.ui_command(&event_loop, ());
    assert_eq!(
        observed
            .try_iter()
            .filter(|o| matches!(o, Observed::Evaluated(_)))
            .count(),
        3
    );
    adapter.resumed(&event_loop);
    adapter.create_surface(&event_loop);
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        [Observed::Evaluated(0), Observed::Updated(0)]
    );
    assert_eq!(event_loop.created_window_ids().len(), 1);
    assert!(proxy.drain_commands().is_empty());
    adapter.exiting(&event_loop);
    handle.get().unwrap().request();
    assert!(proxy.drain_commands().is_empty());
    drop(adapter);
    handle.get().unwrap().request();
    assert!(proxy.drain_commands().is_empty());
}
