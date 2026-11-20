use super::*;
use crate::ui_tree::window::{Apply, Window, WindowConfig, WindowRegistry};
use matcha_window::event::window_event::WindowEvent;
use matcha_window::headless_interface::HeadlessEventLoop;

const FIRST: Constraints = Constraints {
    x: [0.0, 1.0],
    y: [0.0, 0.0],
    z: [0.0, 0.0],
};

#[derive(Clone)]
enum TwoPass {
    Parent(Box<Self>),
    Leaf(Arc<Mutex<Vec<Constraints>>>),
}

impl View for TwoPass {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        if let Self::Parent(child) = self {
            ctx.child(child.as_ref());
        }
        self.clone()
    }
}

impl Widget for TwoPass {
    type V = Self;

    fn update<'a>(&mut self, view: &'a Self, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        if let Self::Parent(child) = view {
            ctx.child(child.as_ref());
        }
        self.clone_from(view);
        UpdateResult::LAYOUT
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }

    fn measure(&self, constraints: Constraints, _: Children<'_>, ctx: &mut Context) -> Bounds {
        let Self::Leaf(calls) = self else {
            return Bounds::default();
        };
        calls.lock().unwrap().push(constraints);
        if constraints == FIRST {
            Bounds {
                x: ctx.viewport_size()[0],
                ..Bounds::default()
            }
        } else {
            Bounds::default()
        }
    }

    fn layout(&self, _: Bounds, mut children: Children<'_>, ctx: &mut Context) {
        children.for_each(|_, mut child| {
            let first = child.measure(FIRST, ctx);
            child.measure(Constraints::default(), ctx);
            child.layout(
                Position {
                    bounds: first,
                    ..Position::default()
                },
                ctx,
            );
        });
    }

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        Draw::default()
    }
}

#[test]
fn resize_refreshes_layout_using_an_earlier_measurement_including_a_cache_hit() {
    for preheat_first in [false, true] {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let window = Window::new(ContainerView {
            child: TwoPass::Parent(Box::new(TwoPass::Leaf(Arc::clone(&calls)))),
            // Keep the two-pass parent's own Bounds unchanged across the resize.
            child_position: position(Matrix4::identity()),
        })
        .with_config(WindowConfig {
            inner_size: Some(Apply::Init([100, 80].into())),
            ..WindowConfig::default()
        });
        let mut registry = WindowRegistry::default();
        let event_loop = HeadlessEventLoop::new();
        let mut open = super::windows::headless_factory(&event_loop);
        let mut root_ctx = Context::new(&mut registry);
        let mut ctx = root_ctx.with_window_factory(&mut open);
        let mut tree = UiTree::new();
        tree.update(&window, &mut ctx);
        let root = tree.root.unwrap();
        let id = super::windows::native_id(&tree, root, &mut ctx);
        let content = child_at(&tree, root, 0);
        let parent = child_at(&tree, content, 0);
        let leaf = child_at(&tree, parent, 0);
        let _ = tree.poll_window(id, &mut ctx);
        if preheat_first {
            tree.measure_node(leaf, FIRST, &mut ctx.unobserved().with_window(id));
        }
        tree.layout_window(id, &mut ctx);
        assert_eq!(*calls.lock().unwrap(), [FIRST, Constraints::default()]);
        assert_eq!(
            tree.node_ref(leaf).unwrap().placement.unwrap().bounds.x,
            100.0
        );
        assert_eq!(
            tree.node_ref(leaf)
                .unwrap()
                .env_reads
                .get(ReadPhase::Measure),
            Some(EnvReads::empty())
        );

        ctx.window_registry()
            .window(id)
            .unwrap()
            .request_inner_size(160, 80);
        let resize = WindowEvent::Resized {
            inner_size: [160.0, 80.0],
            outer_size: [160.0, 80.0],
        };
        tree.window_event(id, &resize, &mut ctx);
        let update = tree.poll_window(id, &mut ctx).unwrap().update;
        assert!(update.contains(UpdateResult::LAYOUT));
        tree.layout_window(id, &mut ctx);
        assert_eq!(
            tree.node_ref(leaf).unwrap().placement.unwrap().bounds.x,
            160.0
        );
        assert_eq!(
            *calls.lock().unwrap(),
            [FIRST, Constraints::default(), FIRST, Constraints::default()]
        );
    }
}

#[derive(Clone)]
enum PlacementOrder {
    Host {
        child: Box<Self>,
        measure_last: bool,
    },
    PlacesDuringMeasure(Box<Self>),
    Leaf,
}

impl PlacementOrder {
    fn declare<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) {
        if let Self::Host { child, .. } | Self::PlacesDuringMeasure(child) = self {
            ctx.child(child.as_ref());
        }
    }
}

impl View for PlacementOrder {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        self.declare(ctx);
        self.clone()
    }
}

impl Widget for PlacementOrder {
    type V = Self;

    fn update<'a>(&mut self, view: &'a Self, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        view.declare(ctx);
        self.clone_from(view);
        UpdateResult::LAYOUT
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }

    fn measure(&self, _: Constraints, mut children: Children<'_>, ctx: &mut Context) -> Bounds {
        children.for_each(|_, mut child| {
            let mut position = Position::default();
            position.bounds.x = 20.0;
            child.layout(position, ctx);
        });
        Bounds::default()
    }

    fn layout(&self, bounds: Bounds, mut children: Children<'_>, ctx: &mut Context) {
        children.for_each(|_, mut child| {
            let mut position = Position::default();
            position.bounds.x = 10.0;
            match self {
                Self::Host {
                    measure_last: true, ..
                } => {
                    child.layout(position, ctx);
                    if bounds.x == 100.0 {
                        child.measure(FIRST, ctx);
                    }
                }
                Self::Host {
                    measure_last: false,
                    ..
                } => {
                    child.measure(FIRST, ctx);
                    if bounds.x == 100.0 {
                        child.layout(position, ctx);
                    }
                }
                Self::PlacesDuringMeasure(_) => child.layout(position, ctx),
                Self::Leaf => unreachable!(),
            }
        });
    }

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        Draw::default()
    }
}

#[test]
fn resize_replays_placement_when_measure_and_layout_previously_overwrote_each_other() {
    for (measure_last, before_resize, after_resize) in [(true, 20.0, 10.0), (false, 10.0, 20.0)] {
        let window = Window::new(PlacementOrder::Host {
            child: Box::new(PlacementOrder::PlacesDuringMeasure(Box::new(
                PlacementOrder::Leaf,
            ))),
            measure_last,
        })
        .with_config(WindowConfig {
            inner_size: Some(Apply::Init([100, 80].into())),
            ..WindowConfig::default()
        });
        let mut registry = WindowRegistry::default();
        let event_loop = HeadlessEventLoop::new();
        let mut open = super::windows::headless_factory(&event_loop);
        let mut root_ctx = Context::new(&mut registry);
        let mut ctx = root_ctx.with_window_factory(&mut open);
        let mut tree = UiTree::new();
        tree.update(&window, &mut ctx);
        let root = tree.root.unwrap();
        let id = super::windows::native_id(&tree, root, &mut ctx);
        let host = child_at(&tree, root, 0);
        let mixed = child_at(&tree, host, 0);
        let leaf = child_at(&tree, mixed, 0);
        let _ = tree.poll_window(id, &mut ctx);
        tree.layout_window(id, &mut ctx);
        tree.render_window(id, &mut ctx).unwrap();
        assert_eq!(
            tree.node_ref(leaf).unwrap().placement.unwrap().bounds.x,
            before_resize
        );

        ctx.window_registry()
            .window(id)
            .unwrap()
            .request_inner_size(160, 80);
        tree.window_event(
            id,
            &WindowEvent::Resized {
                inner_size: [160.0, 80.0],
                outer_size: [160.0, 80.0],
            },
            &mut ctx,
        );
        let _ = tree.poll_window(id, &mut ctx);
        tree.layout_window(id, &mut ctx);
        tree.render_window(id, &mut ctx).unwrap();
        assert_eq!(
            tree.node_ref(leaf).unwrap().placement.unwrap().bounds.x,
            after_resize
        );
    }
}
