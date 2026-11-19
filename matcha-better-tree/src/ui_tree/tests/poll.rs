use super::*;
use crate::ui_tree::widget::{ControlFlow, PollInfo, Lifecycle};
use crate::ui_tree::window::{Apply, Window, WindowConfig, WindowRegistry};
use matcha_window::headless_interface::HeadlessEventLoop;
use std::time::Duration;
use web_time::Instant;

#[derive(Default)]
struct PollState {
    response: PollSummary,
    width: f32,
    scopes: Vec<(Instant, f64, [f32; 2])>,
    measures: usize,
    layouts: usize,
    renders: usize,
}

struct PollView {
    name: &'static str,
    children: Vec<PollView>,
    state: Arc<Mutex<PollState>>,
    order: Arc<Mutex<Vec<&'static str>>>,
}

impl PollView {
    fn new(name: &'static str, children: Vec<Self>, order: &Arc<Mutex<Vec<&'static str>>>) -> Self {
        Self {
            name,
            children,
            state: Arc::new(Mutex::new(PollState {
                width: 10.0,
                ..PollState::default()
            })),
            order: Arc::clone(order),
        }
    }
}

struct PollWidget {
    name: &'static str,
    width: f32,
    state: Arc<Mutex<PollState>>,
    order: Arc<Mutex<Vec<&'static str>>>,
}

impl View for PollView {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        Some(self.name)
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        for child in &self.children {
            ctx.child(child);
        }
        PollWidget {
            name: self.name,
            width: self.state.lock().unwrap().width,
            state: Arc::clone(&self.state),
            order: Arc::clone(&self.order),
        }
    }
}

impl Widget for PollWidget {
    type V = PollView;

    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        for child in &view.children {
            ctx.child(child);
        }
        UpdateResult::empty()
    }

    fn poll(&mut self, _: PollInfo, ctx: &mut Context) -> Lifecycle {
        self.order.lock().unwrap().push(self.name);
        let mut state = self.state.lock().unwrap();
        self.width = state.width;
        state
            .scopes
            .push((ctx.frame_time(), ctx.dpi(), ctx.viewport_size()));
        Lifecycle::Keep {
            update: state.response.update,
            next_poll: state.response.next_poll,
        }
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }

    fn measure(
        &self,
        constraints: Constraints,
        mut children: Children<'_>,
        ctx: &mut Context,
    ) -> Bounds {
        self.state.lock().unwrap().measures += 1;
        let mut width = self.width;
        children.for_each(|_, mut child| width += child.measure(constraints, ctx).x);
        Bounds {
            x: width,
            ..bounds()
        }
    }

    fn layout(&self, _: Bounds, mut children: Children<'_>, ctx: &mut Context) {
        self.state.lock().unwrap().layouts += 1;
        let mut x = 0.0;
        children.for_each(|_, mut child| {
            let measured = child.measure(Constraints::default(), ctx);
            child.layout(
                Position {
                    bounds: measured,
                    affine: translation(x, 0.0, 0.0),
                },
                ctx,
            );
            x += measured.x;
        });
    }

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        self.state.lock().unwrap().renders += 1;
        Draw::default()
    }
}

fn tree_view(order: &Arc<Mutex<Vec<&'static str>>>) -> PollView {
    PollView::new(
        "parent",
        vec![
            PollView::new("leaf", vec![], order),
            PollView::new("sibling", vec![], order),
        ],
        order,
    )
}

#[test]
fn full_poll_visits_cached_widgets_in_postorder_without_invalidating_them() {
    let order = Arc::default();
    let view = tree_view(&order);
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    let time = Instant::now();
    let mut ctx = Context::new(&mut registry).with_frame_time(time);
    tree.update(&view, &mut ctx);
    let root = tree.root.unwrap();
    tree.measure_node(root, Constraints::default(), &mut ctx);
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();

    let (result, reads) = ctx.observe_env_reads(|ctx| tree.poll(ctx));
    assert_eq!(result, PollSummary::default());
    assert_eq!(*order.lock().unwrap(), ["leaf", "sibling", "parent"]);
    assert!(reads.is_empty());
    tree.measure_node(root, Constraints::default(), &mut ctx);
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    for view in [&view, &view.children[0], &view.children[1]] {
        let state = view.state.lock().unwrap();
        assert_eq!((state.measures, state.layouts, state.renders), (1, 1, 1));
        assert_eq!(state.scopes.len(), 1);
        assert_eq!(state.scopes[0].0, time);
    }
}

#[test]
fn poll_invalidates_only_local_drawing_and_applies_layout_before_the_same_frame() {
    let order = Arc::default();
    let view = tree_view(&order);
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    tree.update(&view, &mut ctx);
    let root = tree.root.unwrap();
    let leaf = child_at(&tree, root, 0);
    tree.measure_node(root, Constraints::default(), &mut ctx);
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();

    view.children[0].state.lock().unwrap().response.update = UpdateResult::RENDER;
    assert_eq!(tree.poll(&mut ctx).update, UpdateResult::RENDER);
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    assert_eq!(view.state.lock().unwrap().renders, 1);
    assert_eq!(view.children[0].state.lock().unwrap().renders, 2);
    assert_eq!(view.children[1].state.lock().unwrap().renders, 1);
    assert_eq!(view.children[0].state.lock().unwrap().layouts, 1);

    {
        let mut state = view.children[0].state.lock().unwrap();
        state.width = 25.0;
        state.response.update = UpdateResult::LAYOUT;
    }
    assert_eq!(tree.poll(&mut ctx).update, UpdateResult::LAYOUT);
    assert!(tree.node_ref(root).unwrap().measure_cache.value().is_none());
    assert!(tree.node_ref(leaf).unwrap().render_cache.get(&()).is_some());
    assert_eq!(
        tree.measure_node(root, Constraints::default(), &mut ctx).x,
        45.0
    );
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    assert_eq!(
        tree.node_ref(leaf).unwrap().placement.unwrap().bounds.x,
        25.0
    );
    assert_eq!(view.state.lock().unwrap().layouts, 2);
    assert_eq!(view.children[0].state.lock().unwrap().layouts, 2);
    assert_eq!(view.children[1].state.lock().unwrap().layouts, 1);
}

#[test]
fn poll_aggregates_requests_and_each_pass_replaces_the_previous_deadline() {
    let order = Arc::default();
    let view = tree_view(&order);
    let now = Instant::now();
    let early = now + Duration::from_secs(1);
    let late = now + Duration::from_secs(2);
    view.children[0].state.lock().unwrap().response.next_poll = ControlFlow::WaitUntil(late);
    view.children[1].state.lock().unwrap().response.next_poll = ControlFlow::WaitUntil(early);
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry).with_frame_time(now);
    tree.update(&view, &mut ctx);
    assert_eq!(tree.poll(&mut ctx).next_poll, ControlFlow::WaitUntil(early));
    view.state.lock().unwrap().response.next_poll = ControlFlow::Poll;
    assert_eq!(tree.poll(&mut ctx).next_poll, ControlFlow::Poll);
    for view in [&view, &view.children[0], &view.children[1]] {
        view.state.lock().unwrap().response.next_poll = ControlFlow::Wait;
    }
    assert_eq!(tree.poll(&mut ctx).next_poll, ControlFlow::Wait);
    for request in [
        ControlFlow::Wait,
        ControlFlow::Poll,
        ControlFlow::WaitUntil(early),
    ] {
        assert_eq!(ControlFlow::Wait.merge(request), request);
        assert_eq!(request.merge(ControlFlow::Wait), request);
        assert_eq!(ControlFlow::Poll.merge(request), ControlFlow::Poll);
        assert_eq!(request.merge(ControlFlow::Poll), ControlFlow::Poll);
    }
    assert_eq!(
        ControlFlow::WaitUntil(early).merge(ControlFlow::WaitUntil(late)),
        ControlFlow::WaitUntil(early)
    );
}

#[test]
fn native_poll_uses_window_scope_and_skips_nested_windows() {
    let order = Arc::default();
    let probe = PollView::new("inner", vec![], &order);
    let state = Arc::clone(&probe.state);
    let view = ContainerView {
        child: Window::new(ContainerView {
            child: Window::new(probe).with_config(WindowConfig {
                inner_size: Some(Apply::Init([240, 120].into())),
                ..WindowConfig::default()
            }),
            child_position: position(Matrix4::identity()),
        }),
        child_position: position(Matrix4::identity()),
    };
    let event_loop = HeadlessEventLoop::new();
    let mut open = windows::headless_factory(&event_loop);
    let mut registry = WindowRegistry::default();
    let now = Instant::now();
    let mut ctx = Context::new(&mut registry)
        .with_dpi(3.0)
        .with_frame_time(now);
    let mut tree = UiTree::new();
    tree.try_update(&view, &mut ctx.with_window_factory(&mut open))
        .unwrap();
    let outside = tree.root.unwrap();
    let outer = child_at(&tree, outside, 0);
    let outer_content = child_at(&tree, outer, 0);
    let inner = child_at(&tree, outer_content, 0);
    let outer_id = windows::native_id(&tree, outer, &mut ctx);
    let inner_id = windows::native_id(&tree, inner, &mut ctx);
    tree.measure_node(outside, Constraints::default(), &mut ctx);
    tree.layout(bounds(), &mut ctx);
    tree.measure_node(outer_content, Constraints::default(), &mut ctx);
    tree.layout_window(outer_id, &mut ctx);
    tree.layout_window(inner_id, &mut ctx);
    assert_eq!(tree.poll(&mut ctx), PollSummary::default());
    assert_eq!(
        tree.poll_window(outer_id, &mut ctx),
        Some(PollSummary::default())
    );
    assert!(order.lock().unwrap().is_empty());

    state.lock().unwrap().response.update = UpdateResult::LAYOUT;
    assert_eq!(
        tree.poll_window(inner_id, &mut ctx).unwrap().update,
        UpdateResult::LAYOUT
    );
    assert_eq!(*order.lock().unwrap(), ["inner"]);
    assert_eq!(state.lock().unwrap().scopes, [(now, 1.0, [240.0, 120.0])]);
    assert!(tree.node_ref(inner).unwrap().layout_cache.value().is_none());
    for id in [outside, outer, outer_content] {
        assert!(tree.node_ref(id).unwrap().layout_cache.value().is_some());
    }
    tree.clear(&mut ctx);
    assert_eq!(tree.poll_window(inner_id, &mut ctx), None);
    assert_eq!(tree.poll(&mut ctx), PollSummary::default());
}
