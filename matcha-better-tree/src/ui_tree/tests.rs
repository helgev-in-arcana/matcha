use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use matcha_window::event::device_event::{
    DeviceEventData, DeviceEventState, Modifiers, MouseInput,
};
use nalgebra::Vector3;

use super::*;
use crate::metrics::Position;

mod cache;
mod input;
mod updates;
mod windows;

#[derive(Default)]
struct Observations {
    events: Vec<DeviceEvent>,
    measure_calls: usize,
    layout_calls: usize,
    layout_bounds: Vec<Bounds>,
}

struct ProbeView(Arc<Mutex<Observations>>);
struct ProbeWidget(Arc<Mutex<Observations>>);

impl View for ProbeView {
    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, _: &mut DeclContext<'a>) -> impl Widget<V = Self> {
        ProbeWidget(Arc::clone(&self.0))
    }
}

impl Widget for ProbeWidget {
    type V = ProbeView;

    fn update<'a>(&mut self, view: &'a Self::V, _: &mut DeclContext<'a>) -> UpdateResult {
        self.0 = Arc::clone(&view.0);
        UpdateResult::empty()
    }

    fn input(&mut self, _: Bounds, event: &DeviceEvent, _: Context) -> InputResult {
        self.0.lock().unwrap().events.push(event.clone());
        InputResult::default()
    }

    // Use Widget's default Bounds-based hit test.

    fn measure(&self, _: Constraints, _: Children<'_>, ctx: Context) -> Bounds {
        self.0.lock().unwrap().measure_calls += 1;
        Bounds {
            x: bounds().x * ctx.scale_factor(),
            ..bounds()
        }
    }

    fn layout(&self, bounds: Bounds, _: Children<'_>, _: Context) {
        let mut observations = self.0.lock().unwrap();
        observations.layout_calls += 1;
        observations.layout_bounds.push(bounds);
    }

    fn render(&self, _: Bounds, _: Context) -> Draw {
        Draw {
            color: SmallVec::new(),
            mask: None,
        }
    }
}

struct ContainerView<V> {
    child: V,
    child_position: Position,
}

struct ContainerWidget<V> {
    child: ChildId,
    child_position: Position,
    view_type: PhantomData<fn() -> V>,
}

impl<V: View> View for ContainerView<V> {
    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a>) -> impl Widget<V = Self> {
        ContainerWidget::<V> {
            child: ctx.child(&self.child),
            child_position: self.child_position,
            view_type: PhantomData,
        }
    }
}

impl<V: View> Widget for ContainerWidget<V> {
    type V = ContainerView<V>;

    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a>) -> UpdateResult {
        let changed = self.child_position != view.child_position;
        self.child = ctx.child(&view.child);
        self.child_position = view.child_position;
        if changed {
            UpdateResult::LAYOUT
        } else {
            UpdateResult::empty()
        }
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: Context) -> InputResult {
        InputResult::default()
    }

    fn measure(
        &self,
        constraints: Constraints,
        mut children: Children<'_>,
        ctx: Context,
    ) -> Bounds {
        children.measure(self.child, constraints, ctx).unwrap()
    }

    fn layout(&self, _: Bounds, mut children: Children<'_>, ctx: Context) {
        // This placement is relative to this container, independent of its own placement.
        children.layout(self.child, self.child_position, ctx);
    }

    fn render(&self, _: Bounds, _: Context) -> Draw {
        Draw {
            color: SmallVec::new(),
            mask: None,
        }
    }
}

fn bounds() -> Bounds {
    Bounds {
        x: 10.0,
        y: 10.0,
        z: 10.0,
    }
}

fn position(affine: Matrix4<f32>) -> Position {
    Position {
        bounds: bounds(),
        affine,
    }
}

fn translation(x: f32, y: f32, z: f32) -> Matrix4<f32> {
    Matrix4::new_translation(&Vector3::new(x, y, z))
}

fn quarter_turn() -> Matrix4<f32> {
    Matrix4::new(
        0.0, -1.0, 0.0, 0.0, // x' = -y
        1.0, 0.0, 0.0, 0.0, // y' = x
        0.0, 0.0, 1.0, 0.0, // z' = z
        0.0, 0.0, 0.0, 1.0,
    )
}

fn setup<V: View>(child: V) -> (UiTree, UiId, ChildId) {
    let mut tree = UiTree::new();
    tree.update(&ContainerView {
        child,
        child_position: position(Matrix4::identity()),
    });
    let root = tree.root.unwrap();
    // This fixture places the host only; each test controls when/how its child is laid out.
    tree.node_ref_mut(root).unwrap().placement = Some(Placement {
        bounds: bounds(),
        affine: Matrix4::identity(),
        affine_inverse: Some(Matrix4::identity()),
    });
    let child = tree.node_ref(root).unwrap().children[0].0;
    (tree, root, child)
}

fn mouse_event(x: f32, y: f32) -> DeviceEvent {
    DeviceEventState::default()
        .process(DeviceEvent::stateless(DeviceEventData::MouseInput {
            dragging_from_primary: None,
            dragging_from_secondary: None,
            dragging_from_middle: None,
            event: Some(MouseInput::Moved { position: [x, y] }),
        }))
        .unwrap()
}

#[test]
fn hit_test_maps_affine_points_to_default_local_bounds() {
    let (mut tree, root, child) = setup(ProbeView(Arc::default()));
    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    for affine in [
        Matrix4::identity(),
        translation(100.0, 20.0, 7.0),
        quarter_turn(),
        Matrix4::new_nonuniform_scaling(&Vector3::new(-2.0, 3.0, 4.0)),
        translation(100.0, 20.0, 7.0)
            * quarter_turn()
            * Matrix4::new_nonuniform_scaling(&Vector3::new(2.0, 3.0, 4.0)),
    ] {
        children.layout(child, position(affine), Context::new());
        for (local, expected) in [
            (Vector4::new(2.0, 3.0, 4.0, 1.0), true),
            (Vector4::new(11.0, 3.0, 4.0, 1.0), false),
            (Vector4::new(2.0, 3.0, 11.0, 1.0), false),
        ] {
            let parent = affine * local;
            assert_eq!(
                children.is_inside(
                    child,
                    Point {
                        x: parent.x,
                        y: parent.y,
                        z: parent.z,
                    },
                    Context::new(),
                ),
                Some(expected),
                "affine={affine:?}, local={local:?}",
            );
        }
    }
}

#[test]
fn translated_hit_test_includes_local_bounds_edges() {
    let (mut tree, root, child) = setup(ProbeView(Arc::default()));
    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    // Exact inverses avoid rounding at the boundary; arbitrary affine transforms above use
    // interior/exterior points so they don't impose a floating-point tolerance policy.
    children.layout(
        child,
        position(translation(100.0, 20.0, 7.0)),
        Context::new(),
    );
    for point in [
        Point {
            x: 100.0,
            y: 20.0,
            z: 7.0,
        },
        Point {
            x: 110.0,
            y: 30.0,
            z: 17.0,
        },
    ] {
        assert_eq!(children.is_inside(child, point, Context::new()), Some(true));
    }
}

#[test]
fn hit_test_uses_updated_affine_and_bounds() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ProbeView(Arc::clone(&observations)));
    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    let point = |x| Point { x, y: 5.0, z: 5.0 };
    children.layout(
        child,
        position(translation(100.0, 0.0, 0.0)),
        Context::new(),
    );
    assert_eq!(
        children.is_inside(child, point(105.0), Context::new()),
        Some(true)
    );
    let mut updated = position(translation(200.0, 0.0, 0.0));
    children.layout(child, updated, Context::new());
    assert_eq!(observations.lock().unwrap().layout_calls, 1);
    let _ = children
        .tree
        .input(&mouse_event(205.0, 5.0), Context::new());
    assert_eq!(
        observations.lock().unwrap().events[0].mouse_position(),
        Some([5.0, 5.0])
    );
    assert_eq!(
        children.is_inside(child, point(105.0), Context::new()),
        Some(false)
    );
    assert_eq!(
        children.is_inside(child, point(205.0), Context::new()),
        Some(true)
    );
    assert_eq!(
        children.is_inside(child, point(215.0), Context::new()),
        Some(false)
    );
    updated.bounds.x = 20.0;
    children.layout(child, updated, Context::new());
    children.layout(child, updated, Context::new());
    assert_eq!(
        children.is_inside(child, point(215.0), Context::new()),
        Some(true)
    );
    let observations = observations.lock().unwrap();
    assert_eq!(observations.layout_calls, 2);
    assert_eq!(observations.layout_bounds, vec![bounds(), updated.bounds]);
}

#[test]
fn invalidating_layout_preserves_placement_and_recomputes_same_bounds() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ProbeView(Arc::clone(&observations)));
    let placed = position(translation(100.0, 0.0, 0.0));
    Children {
        tree: &mut tree,
        current: root,
    }
    .layout(child, placed, Context::new());
    let child_ui_id = tree.node_ref(root).unwrap().child(child).unwrap();
    tree.node_ref_mut(child_ui_id).unwrap().layout_cache = None;

    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    assert_eq!(
        children.is_inside(
            child,
            Point {
                x: 105.0,
                y: 5.0,
                z: 0.0
            },
            Context::new()
        ),
        Some(true)
    );
    let _ = children
        .tree
        .input(&mouse_event(105.0, 5.0), Context::new());
    assert_eq!(
        observations.lock().unwrap().events[0].mouse_position(),
        Some([5.0, 5.0])
    );
    children.layout(child, placed, Context::new());
    children.layout(child, placed, Context::new());
    assert_eq!(observations.lock().unwrap().layout_calls, 2);
}

#[test]
fn moving_container_reuses_descendant_layout() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ContainerView {
        child: ProbeView(Arc::clone(&observations)),
        child_position: position(translation(10.0, 0.0, 0.0)),
    });
    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    children.layout(
        child,
        position(translation(100.0, 0.0, 0.0)),
        Context::new(),
    );
    children.layout(
        child,
        position(translation(200.0, 0.0, 0.0)),
        Context::new(),
    );
    assert_eq!(observations.lock().unwrap().layout_calls, 1);
    assert_eq!(
        children
            .tree
            .hit_test(
                root,
                Point {
                    x: 215.0,
                    y: 5.0,
                    z: 0.0
                },
                &Context::new()
            )
            .is_some(),
        true
    );
    assert_eq!(
        children
            .tree
            .hit_test(
                root,
                Point {
                    x: 115.0,
                    y: 5.0,
                    z: 0.0
                },
                &Context::new()
            )
            .is_some(),
        false
    );
    let _ = children
        .tree
        .input(&mouse_event(215.0, 5.0), Context::new());
    assert_eq!(
        observations.lock().unwrap().events[0].mouse_position(),
        Some([5.0, 5.0])
    );
}

#[test]
fn singular_hit_test_returns_none_and_recovers_after_layout() {
    let (mut tree, root, child) = setup(ProbeView(Arc::default()));
    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    let point = Point {
        x: 5.0,
        y: 5.0,
        z: 5.0,
    };
    children.layout(child, position(Matrix4::new_scaling(0.0)), Context::new());
    assert_eq!(children.is_inside(child, point, Context::new()), None);
    children.layout(child, position(Matrix4::identity()), Context::new());
    assert_eq!(children.is_inside(child, point, Context::new()), Some(true));
}

#[test]
fn missing_layout_and_stale_child_are_not_dispatched() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ProbeView(Arc::clone(&observations)));
    let event = mouse_event(5.0, 5.0);
    let point = Point {
        x: 5.0,
        y: 5.0,
        z: 0.0,
    };
    {
        let mut children = Children {
            tree: &mut tree,
            current: root,
        };
        assert_eq!(children.is_inside(child, point, Context::new()), None);
        let _ = children.tree.input(&event, Context::new());
        children.layout(child, position(Matrix4::identity()), Context::new());
    }
    // Replacing the root removes the old child and invalidates its handle.
    tree.update(&ProbeView(Arc::default()));
    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    assert_eq!(children.is_inside(child, point, Context::new()), None);
    let _ = children.tree.input(&event, Context::new());
    assert!(observations.lock().unwrap().events.is_empty());
}

#[test]
fn nested_hit_test_applies_each_relative_transform_once() {
    let (mut tree, root, child) = setup(ContainerView {
        child: ProbeView(Arc::default()),
        child_position: position(
            translation(10.0, 5.0, 0.0)
                * Matrix4::new_nonuniform_scaling(&Vector3::new(2.0, 3.0, 1.0)),
        ),
    });
    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    children.layout(
        child,
        position(translation(100.0, 50.0, 0.0) * quarter_turn()),
        Context::new(),
    );
    // Leaf-local (2, 3) -> container-local (14, 14) -> root-local (86, 64).
    assert_eq!(
        children
            .tree
            .hit_test(
                root,
                Point {
                    x: 86.0,
                    y: 64.0,
                    z: 0.0
                },
                &Context::new()
            )
            .is_some(),
        true
    );
    // Leaf-local (12, 3) is outside the leaf's bounds.
    assert_eq!(
        children
            .tree
            .hit_test(
                root,
                Point {
                    x: 86.0,
                    y: 84.0,
                    z: 0.0
                },
                &Context::new()
            )
            .is_some(),
        false
    );
}

#[test]
fn nested_input_preserves_raw_event_and_delivers_local_mouse_position() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ContainerView {
        child: ProbeView(Arc::clone(&observations)),
        child_position: position(
            translation(10.0, 5.0, 0.0)
                * Matrix4::new_nonuniform_scaling(&Vector3::new(2.0, 3.0, 1.0)),
        ),
    });
    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    children.layout(
        child,
        position(translation(100.0, 50.0, 0.0) * quarter_turn()),
        Context::new(),
    );
    let event = mouse_event(86.0, 64.0);
    let _ = children.tree.input(&event, Context::new());
    let observations = observations.lock().unwrap();
    assert_eq!(observations.events.len(), 1);
    let received = &observations.events[0];
    let local = received.mouse_position().unwrap();
    assert!((local[0] - 2.0).abs() < 1e-5);
    assert!((local[1] - 3.0).abs() < 1e-5);
    assert_eq!(received.raw_event(), event.raw_event());
    assert_eq!(received.mouse_viewport_position(), [86.0, 64.0]);
    assert_eq!(event.mouse_position(), Some([86.0, 64.0]));
}

#[test]
fn non_pointer_input_waits_for_a_separate_delivery_route() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ProbeView(Arc::clone(&observations)));
    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    children.layout(child, position(Matrix4::identity()), Context::new());
    let event = DeviceEvent::stateless(DeviceEventData::ModifiersChanged(Modifiers::default()));
    assert_eq!(
        children.tree.input(&event, Context::new()),
        InputResult::default()
    );
    let observations = observations.lock().unwrap();
    assert!(observations.events.is_empty());
}
