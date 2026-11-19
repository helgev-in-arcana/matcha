use super::*;
use crate::metrics::Position;
use crate::ui_tree::widget::{ControlFlow, Lifecycle, PollInfo, WidgetPhase};
use matcha_window::application::Application;
use std::{marker::PhantomData, time::Duration};
use web_time::Instant;

// Reuse the same container inside and outside a Window: each one would request a
// delayed exit if asked. Only the branch inside the Window has a polling driver.
struct Branch<V> {
    child: Option<V>,
    enter_at: Option<Instant>,
    exit_at: Instant,
    layout_counts: Option<Arc<Mutex<Vec<usize>>>>,
}

impl<V: View> Branch<V> {
    fn declare<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) {
        if let Some(child) = &self.child {
            ctx.child(child);
        }
    }
}

impl<V: View> View for Branch<V> {
    type ChildData = ();
    fn label(&self) -> Option<&str> {
        None
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        if self.enter_at.is_some() {
            ctx.start_inactive();
        }
        self.declare(ctx);
        BranchWidget::<V> {
            enter_at: self.enter_at,
            exit_at: self.exit_at,
            layout_counts: self.layout_counts.clone(),
            child_type: PhantomData,
        }
    }
}

struct BranchWidget<V> {
    enter_at: Option<Instant>,
    exit_at: Instant,
    layout_counts: Option<Arc<Mutex<Vec<usize>>>>,
    child_type: PhantomData<fn() -> V>,
}

impl<V: View> Widget for BranchWidget<V> {
    type V = Branch<V>;

    fn update<'a>(&mut self, view: &'a Branch<V>, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        view.declare(ctx);
        UpdateResult::empty()
    }
    fn poll(&mut self, info: PollInfo, ctx: &mut Context) -> Lifecycle {
        let next_poll = match info.phase {
            WidgetPhase::PendingEnter => {
                let t = self.enter_at.unwrap();
                if ctx.frame_time() >= t {
                    return info.phase.default_lifecycle();
                }
                ControlFlow::WaitUntil(t)
            }
            WidgetPhase::Exiting if ctx.frame_time() >= self.exit_at => {
                return info.phase.default_lifecycle();
            }
            WidgetPhase::Exiting => ControlFlow::WaitUntil(self.exit_at),
            WidgetPhase::Active => return info.phase.default_lifecycle(),
        };
        Lifecycle::Keep {
            next_poll,
            update: UpdateResult::empty(),
        }
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }
    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds::default()
    }
    fn layout(&self, bounds: Bounds, mut children: Children<'_>, ctx: &mut Context) {
        if let Some(counts) = &self.layout_counts {
            counts.lock().unwrap().push(children.len());
        }
        children.for_each(|_, mut child| {
            child.layout(
                Position {
                    bounds,
                    ..Default::default()
                },
                ctx,
            )
        });
    }
    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        Draw::default()
    }
}

struct Demo {
    enter: Instant,
    exit: Instant,
    show_item: bool,
    show_host: bool,
    layout_counts: Arc<Mutex<Vec<usize>>>,
}

impl Model for Demo {
    fn view(&mut self, _: &ModelContext<'_>) -> impl View {
        let leaf = Branch::<Button> {
            child: None,
            enter_at: Some(self.enter),
            exit_at: self.exit,
            layout_counts: None,
        };
        let window = Window::new(Branch {
            child: self.show_item.then_some(leaf),
            enter_at: None,
            exit_at: self.exit,
            layout_counts: Some(self.layout_counts.clone()),
        });
        let host = Branch {
            child: Some(window),
            enter_at: None,
            exit_at: self.exit,
            layout_counts: None,
        };
        Branch {
            child: self.show_host.then_some(host),
            enter_at: None,
            exit_at: self.exit,
            layout_counts: None,
        }
    }
}

#[test]
fn presence_wakes_and_reflows_its_window_and_app_removes_undriven_hosts() {
    let hour = Duration::from_secs(3_600);
    let enter = Instant::now() + hour;
    let exit = enter + hour;
    let counts: Arc<Mutex<Vec<usize>>> = Arc::default();
    let model = Demo {
        enter,
        exit,
        layout_counts: counts.clone(),
        show_host: true,
        show_item: true,
    };
    let mut app = App::new(model, noop_gpu());
    let event_loop = HeadlessEventLoop::new();
    app.init(Box::new(HeadlessEventLoopProxy::new()), &event_loop);
    app.create_surface(&event_loop);
    let [id]: [_; 1] = event_loop.created_window_ids().try_into().unwrap();
    let redraws = || event_loop.redraw_request_count(id).unwrap();
    app.render(id);
    app.about_to_wait(&event_loop);
    assert_eq!(event_loop.control_flow(), ControlFlow::WaitUntil(enter));
    assert_eq!(*counts.lock().unwrap(), [0]);

    let before = redraws();
    app.poll_ready(&event_loop, enter);
    assert_eq!(redraws(), before + 1);
    assert_eq!(event_loop.control_flow(), ControlFlow::Wait);
    app.render(id);
    assert_eq!(counts.lock().unwrap().last(), Some(&1));

    app.model.show_item = false;
    app.request.as_ref().unwrap().request();
    app.ui_command(&event_loop, ());
    app.about_to_wait(&event_loop);
    assert_eq!(event_loop.control_flow(), ControlFlow::WaitUntil(exit));
    let before = redraws();
    app.poll_ready(&event_loop, exit);
    assert_eq!(redraws(), before + 1);
    assert_eq!(event_loop.control_flow(), ControlFlow::Wait);
    app.render(id);
    assert_eq!(counts.lock().unwrap().last(), Some(&0));

    app.model.show_host = false;
    app.request.as_ref().unwrap().request();
    app.ui_command(&event_loop, ());
    assert!(
        app.windows.ids().is_empty(),
        "App only polls native window contents; an outside host cannot wait for an exit poll"
    );
}

#[test]
fn a_view_update_finishes_a_default_exit_before_any_redraw() {
    struct Buttons(Option<Button>);
    impl Model for Buttons {
        fn view(&mut self, _: &ModelContext<'_>) -> impl View {
            Window::new(Branch {
                child: self.0.clone(),
                enter_at: None,
                exit_at: Instant::now(),
                layout_counts: None,
            })
        }
    }

    let (observed, _events) = mpsc::channel();
    let on_input: Arc<ButtonInput> = Arc::new(|| {});
    let mounted = Arc::downgrade(&on_input);
    let button = Button {
        value: 0,
        on_input,
        observed,
    };
    let mut app = App::new(Buttons(Some(button)), noop_gpu());
    let event_loop = HeadlessEventLoop::new();
    app.init(Box::new(HeadlessEventLoopProxy::new()), &event_loop);
    app.create_surface(&event_loop);

    app.model.0 = None;
    assert!(
        mounted.upgrade().is_some(),
        "the mounted widget still owns the callback"
    );
    app.request.as_ref().unwrap().request();
    app.ui_command(&event_loop, ());
    assert!(
        mounted.upgrade().is_none(),
        "the update's poll finished the exit"
    );
}

#[test]
fn retiring_an_ordinary_branch_closes_its_nested_os_window_on_omission() {
    struct Nested {
        show_branch: bool,
        inner_wake: Instant,
        exit: Instant,
        counts: Arc<Mutex<Vec<usize>>>,
    }
    impl Model for Nested {
        fn view(&mut self, _: &ModelContext<'_>) -> impl View {
            let inner = Window::new(Branch::<Button> {
                child: None,
                enter_at: Some(self.inner_wake),
                exit_at: self.exit,
                layout_counts: None,
            });
            let retiring = Branch {
                child: Some(inner),
                enter_at: None,
                exit_at: self.exit,
                layout_counts: Some(self.counts.clone()),
            };
            Window::new(Branch {
                child: self.show_branch.then_some(retiring),
                enter_at: None,
                exit_at: self.exit,
                layout_counts: None,
            })
        }
    }
    let inner_wake = Instant::now() + Duration::from_secs(3_600);
    let exit = inner_wake + Duration::from_secs(3_600);
    let counts = Arc::new(Mutex::new(Vec::new()));
    let mut app = App::new(
        Nested {
            show_branch: true,
            inner_wake,
            exit,
            counts: counts.clone(),
        },
        noop_gpu(),
    );
    let event_loop = HeadlessEventLoop::new();
    app.init(Box::new(HeadlessEventLoopProxy::new()), &event_loop);
    app.create_surface(&event_loop);
    let [outer, inner]: [_; 2] = event_loop.created_window_ids().try_into().unwrap();
    app.render(outer);
    app.about_to_wait(&event_loop);
    assert_eq!(
        event_loop.control_flow(),
        ControlFlow::WaitUntil(inner_wake)
    );
    assert_eq!(*counts.lock().unwrap(), [1]);

    app.model.show_branch = false;
    app.request.as_ref().unwrap().request();
    app.ui_command(&event_loop, ());
    app.about_to_wait(&event_loop);
    assert!(app.windows.window(inner).is_none());
    assert_eq!(app.windows.ids(), [outer]);
    assert_eq!(event_loop.control_flow(), ControlFlow::WaitUntil(exit));
    app.render(outer);
    assert_eq!(
        *counts.lock().unwrap(),
        [1, 0],
        "the retiring parent remains and relays out without the closed window"
    );
}
