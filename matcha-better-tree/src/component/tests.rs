use std::num::NonZeroUsize;
use std::sync::mpsc;

use futures::FutureExt;
use futures::executor::LocalPool;
use futures::future::RemoteHandle;
use futures::task::LocalSpawnExt;
use matcha_window::adapter::Adapter;
use matcha_window::event::{device_event::DeviceEvent, window_event::WindowEvent};
use matcha_window::headless_interface::{HeadlessEventLoop, HeadlessEventLoopProxy};

use super::{Component, ComponentPod, EmitError, EventSender, TryEmitError};
use crate::app::{App, Gpu, GpuDescriptor, Model, UpdateView};
use crate::metrics::{Bounds, Constraints};
use crate::ui_tree::context::Context;
use crate::ui_tree::widget::{Draw, InputResult, UpdateResult, View, Widget};
use crate::ui_tree::window::Window;
use crate::ui_tree::{Children, DeclContext};

struct Empty;

impl View for Empty {
    type ChildData = ();
    fn label(&self) -> Option<&str> {
        None
    }
    fn build<'a>(&'a self, _: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        Empty
    }
}

impl Widget for Empty {
    type V = Self;
    fn update<'a>(&mut self, _: &'a Self, _: &mut DeclContext<'a, '_>) -> UpdateResult {
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

struct Host<M: Model + utils::MaybeSendSync + 'static> {
    adapter: Adapter<App<M>>,
    event_loop: HeadlessEventLoop,
    proxy: HeadlessEventLoopProxy<App<M>>,
}

impl<M: Model + utils::MaybeSendSync + 'static> Host<M> {
    fn mount(model: M) -> Self {
        let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor::noop())).unwrap();
        let mut adapter = Adapter::new(App::new(model, gpu));
        let event_loop = HeadlessEventLoop::new();
        let proxy = HeadlessEventLoopProxy::new();
        adapter.init(Box::new(proxy.clone()), &event_loop);
        adapter.create_surface(&event_loop);
        Self {
            adapter,
            event_loop,
            proxy,
        }
    }

    fn dispatch(&mut self) {
        assert_eq!(self.proxy.drain_commands(), [()]);
        self.adapter.ui_command(&self.event_loop, ());
    }
}

#[derive(Debug)]
enum Edit {
    Append(u32),
    AppendLater(u32),
    Hold(mpsc::Sender<()>),
}

struct Tab {
    value: u32,
    initialized: mpsc::Sender<EventSender<Edit>>,
    values: mpsc::Sender<u32>,
}

struct TabProbe {
    initialized: mpsc::Receiver<EventSender<Edit>>,
    values: mpsc::Receiver<u32>,
}

fn tab() -> (Tab, TabProbe) {
    let (init_tx, initialized) = mpsc::channel();
    let (value_tx, values) = mpsc::channel();
    let component = Tab {
        value: 0,
        initialized: init_tx,
        values: value_tx,
    };
    (
        component,
        TabProbe {
            initialized,
            values,
        },
    )
}

impl Component for Tab {
    type Message = Edit;

    fn init(&mut self, events: &EventSender<Edit>) {
        events.try_emit(Edit::Append(5)).unwrap();
        self.initialized.send(events.clone()).unwrap();
    }

    fn update(&mut self, edit: Edit, events: &EventSender<Edit>) {
        match edit {
            Edit::Append(digit) => self.value = self.value * 10 + digit,
            Edit::AppendLater(digit) => events.try_emit(Edit::Append(digit)).unwrap(),
            Edit::Hold(sender) => drop(sender),
        }
    }

    fn view(&mut self, _: &UpdateView, events: EventSender<Edit>) -> impl View {
        self.values.send(self.value).unwrap();
        Window::new(Empty).on_close_requested(move || events.try_emit(Edit::Append(1)).unwrap())
    }
}

#[test]
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn widget_and_task_messages_are_batched_in_order_and_self_sends_wait() {
    let (tab, probe) = tab();
    let mut host = Host::mount(ComponentPod::new(tab));
    let events = probe.initialized.try_recv().unwrap();
    assert_eq!(probe.values.try_recv().unwrap(), 0);
    host.dispatch();
    assert_eq!(probe.values.try_recv().unwrap(), 5);
    let [window] = host.event_loop.created_window_ids().try_into().unwrap();
    host.adapter
        .window_event(&host.event_loop, window, WindowEvent::CloseRequested);
    std::thread::spawn(move || {
        events.try_emit(Edit::Append(2)).unwrap();
        events.try_emit(Edit::AppendLater(4)).unwrap();
        events.try_emit(Edit::Append(3)).unwrap();
    })
    .join()
    .unwrap();
    assert!(probe.values.try_recv().is_err());

    host.dispatch();
    assert_eq!(probe.values.try_iter().collect::<Vec<_>>(), [5123]);
    host.dispatch();
    assert_eq!(probe.values.try_iter().collect::<Vec<_>>(), [51234]);
    assert!(host.proxy.drain_commands().is_empty());
    assert!(probe.initialized.try_recv().is_err());
}

fn start_waiting_send(
    pool: &mut LocalPool,
    events: &EventSender<Edit>,
    edit: Edit,
) -> RemoteHandle<Result<(), EmitError<Edit>>> {
    let events = events.clone();
    let mut result = pool
        .spawner()
        .spawn_local_with_handle(async move { events.emit_async(edit).await })
        .unwrap();
    pool.run_until_stalled();
    assert!((&mut result).now_or_never().is_none());
    result
}

#[test]
fn receiving_a_message_frees_a_slot_and_wakes_a_waiting_sender() {
    let (tab, probe) = tab();
    let mut host = Host::mount(ComponentPod::with_capacity(tab, NonZeroUsize::MIN));
    let events = probe.initialized.try_recv().unwrap();
    assert_eq!(probe.values.try_recv().unwrap(), 0);
    let Err(TryEmitError::Full(edit)) = events.try_emit(Edit::Append(8)) else {
        panic!("the init message occupies the only slot");
    };
    let mut pool = LocalPool::new();
    let result = start_waiting_send(&mut pool, &events, edit);

    host.dispatch();
    assert_eq!(probe.values.try_recv().unwrap(), 5);
    assert!(host.proxy.drain_commands().is_empty());
    pool.run_until_stalled();
    assert!(matches!(result.now_or_never(), Some(Ok(()))));
    host.dispatch();
    assert_eq!(probe.values.try_recv().unwrap(), 58);
    assert!(host.proxy.drain_commands().is_empty());
}

#[derive(Debug)]
enum TabAction {
    Select(usize),
    RemoveFirst,
}

struct Tabs {
    children: Vec<ComponentPod<Tab>>,
    selected: usize,
    initialized: mpsc::Sender<EventSender<TabAction>>,
}

impl Component for Tabs {
    type Message = TabAction;

    fn init(&mut self, events: &EventSender<TabAction>) {
        self.initialized.send(events.clone()).unwrap();
    }

    fn update(&mut self, action: TabAction, _: &EventSender<TabAction>) {
        match action {
            TabAction::Select(index) => self.selected = index,
            TabAction::RemoveFirst => {
                self.children.remove(0);
                self.selected = 0;
            }
        }
    }

    fn view(&mut self, update: &UpdateView, _: EventSender<TabAction>) -> impl View {
        self.children[self.selected].view(update)
    }
}

#[test]
fn hidden_child_retains_messages_and_a_removed_child_stops_waking_the_app() {
    let (left, left_probe) = tab();
    let (right, right_probe) = tab();
    let (initialized, control) = mpsc::channel();
    let mut host = Host::mount(ComponentPod::new(Tabs {
        children: vec![
            ComponentPod::with_capacity(left, NonZeroUsize::MIN),
            ComponentPod::new(right),
        ],
        selected: 0,
        initialized,
    }));
    let tabs = control.try_recv().unwrap();
    let left = left_probe.initialized.try_recv().unwrap();
    assert_eq!(left_probe.values.try_recv().unwrap(), 0);
    assert!(right_probe.initialized.try_recv().is_err());
    host.dispatch();
    assert_eq!(left_probe.values.try_recv().unwrap(), 5);

    tabs.try_emit(TabAction::Select(1)).unwrap();
    host.dispatch();
    let right = right_probe.initialized.try_recv().unwrap();
    assert_eq!(right_probe.values.try_recv().unwrap(), 0);
    left.try_emit(Edit::Append(7)).unwrap();
    right.try_emit(Edit::Append(9)).unwrap();
    host.dispatch();
    assert!(left_probe.values.try_recv().is_err());
    assert_eq!(right_probe.values.try_recv().unwrap(), 59);

    tabs.try_emit(TabAction::Select(0)).unwrap();
    host.dispatch();
    assert_eq!(left_probe.values.try_recv().unwrap(), 57);
    assert!(left_probe.initialized.try_recv().is_err());
    tabs.try_emit(TabAction::Select(1)).unwrap();
    host.dispatch();
    assert_eq!(right_probe.values.try_recv().unwrap(), 59);
    let (queued_payload, payload_lifetime) = mpsc::channel();
    left.try_emit(Edit::Hold(queued_payload)).unwrap();
    host.dispatch();
    assert_eq!(right_probe.values.try_recv().unwrap(), 59);
    assert!(matches!(
        payload_lifetime.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        left.try_emit(Edit::Append(6)),
        Err(TryEmitError::Full(Edit::Append(6)))
    ));
    let mut pool = LocalPool::new();
    let result = start_waiting_send(&mut pool, &left, Edit::Append(3));
    assert!(host.proxy.drain_commands().is_empty());

    tabs.try_emit(TabAction::RemoveFirst).unwrap();
    host.dispatch();
    assert_eq!(right_probe.values.try_recv().unwrap(), 59);
    assert!(matches!(
        payload_lifetime.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    pool.run_until_stalled();
    assert!(matches!(
        result.now_or_never(),
        Some(Err(EmitError::Closed(Edit::Append(3))))
    ));
    assert!(matches!(
        left.try_emit(Edit::Append(4)),
        Err(TryEmitError::Closed(Edit::Append(4)))
    ));
    assert!(host.proxy.drain_commands().is_empty());
}
