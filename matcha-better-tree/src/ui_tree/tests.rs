use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use matcha_window::event::device_event::{
    DeviceEventData, DeviceEventState, Modifiers, MouseInput,
};
use nalgebra::Vector3;
use smallvec::SmallVec;

use super::*;
use crate::metrics::Position;

mod cache;
mod child_properties;
mod env_reads;
mod input;
mod updates;
mod windows;

#[derive(Default)]
struct Observations {
    events: Vec<DeviceEvent>,
    measure_calls: usize,
    layout_calls: usize,
    layout_bounds: Vec<Bounds>,
    render_calls: usize,
    scopes: Vec<(&'static str, f64, [f32; 2])>,
}

fn record_scope(observations: &Arc<Mutex<Observations>>, stage: &'static str, ctx: &mut Context) {
    observations
        .lock()
        .unwrap()
        .scopes
        .push((stage, ctx.dpi(), ctx.viewport_size()));
}

struct ProbeView(Arc<Mutex<Observations>>);
struct ProbeWidget(Arc<Mutex<Observations>>);

impl View for ProbeView {
    type ChildData = ();
    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        record_scope(&self.0, "build", ctx.context());
        ProbeWidget(Arc::clone(&self.0))
    }
}

impl Widget for ProbeWidget {
    type V = ProbeView;

    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        self.0 = Arc::clone(&view.0);
        record_scope(&self.0, "update", ctx.context());
        UpdateResult::empty()
    }

    fn input(&mut self, _: Bounds, event: &DeviceEvent, ctx: &mut Context) -> InputResult {
        record_scope(&self.0, "input", ctx);
        self.0.lock().unwrap().events.push(event.clone());
        InputResult::default()
    }

    // Use Widget's default Bounds-based hit test.

    fn measure(&self, _: Constraints, _: Children<'_>, ctx: &mut Context) -> Bounds {
        record_scope(&self.0, "measure", ctx);
        self.0.lock().unwrap().measure_calls += 1;
        Bounds {
            x: bounds().x * ctx.scale_factor(),
            ..bounds()
        }
    }

    fn layout(&self, bounds: Bounds, _: Children<'_>, ctx: &mut Context) {
        record_scope(&self.0, "layout", ctx);
        let mut observations = self.0.lock().unwrap();
        observations.layout_calls += 1;
        observations.layout_bounds.push(bounds);
    }

    fn render(&self, _: Bounds, ctx: &mut Context) -> Draw {
        record_scope(&self.0, "render", ctx);
        self.0.lock().unwrap().render_calls += 1;
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
    view_type: PhantomData<fn() -> V>,
}

impl<V: View> View for ContainerView<V> {
    type ChildData = Position;
    fn label(&self) -> Option<&str> {
        None
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_, Position>) -> impl Widget<V = Self> {
        ctx.child_with(&self.child, self.child_position);
        ContainerWidget::<V> {
            view_type: PhantomData,
        }
    }
}

impl<V: View> Widget for ContainerWidget<V> {
    type V = ContainerView<V>;

    fn update<'a>(
        &mut self,
        view: &'a Self::V,
        ctx: &mut DeclContext<'a, '_, Position>,
    ) -> UpdateResult {
        ctx.child_with(&view.child, view.child_position);
        UpdateResult::empty()
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }

    fn measure(
        &self,
        constraints: Constraints,
        mut children: Children<'_, Position>,
        ctx: &mut Context,
    ) -> Bounds {
        let mut measured = None;
        children.for_each(|_, mut child| measured = Some(child.measure(constraints, ctx)));
        measured.expect("container has one child")
    }

    fn layout(&self, _: Bounds, mut children: Children<'_, Position>, ctx: &mut Context) {
        children.for_each(|position, mut child| child.layout(*position, ctx));
    }

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
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

fn child_at(tree: &UiTree, parent: UiId, index: usize) -> UiId {
    tree.node_ref(parent)
        .unwrap()
        .child_ids()
        .nth(index)
        .unwrap()
}

fn child_access(tree: &mut UiTree, id: UiId) -> Child<'_> {
    Child { tree, id }
}

fn setup<V: View>(child: V) -> (UiTree, UiId, UiId) {
    let mut tree = UiTree::new();
    tree.update(
        &ContainerView {
            child,
            child_position: position(Matrix4::identity()),
        },
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    let root = tree.root.unwrap();
    // This fixture places the host only; each test controls when/how its child is laid out.
    tree.node_ref_mut(root).unwrap().placement = Some(Placement {
        bounds: bounds(),
        affine: Matrix4::identity(),
        affine_inverse: Some(Matrix4::identity()),
    });
    let child = child_at(&tree, root, 0);
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
    let (mut tree, _, child) = setup(ProbeView(Arc::default()));
    for affine in [
        Matrix4::identity(),
        translation(100.0, 20.0, 7.0),
        quarter_turn(),
        Matrix4::new_nonuniform_scaling(&Vector3::new(-2.0, 3.0, 4.0)),
        translation(100.0, 20.0, 7.0)
            * quarter_turn()
            * Matrix4::new_nonuniform_scaling(&Vector3::new(2.0, 3.0, 4.0)),
    ] {
        child_access(&mut tree, child).layout(
            position(affine),
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
        );
        for (local, expected) in [
            (Vector4::new(2.0, 3.0, 4.0, 1.0), true),
            (Vector4::new(11.0, 3.0, 4.0, 1.0), false),
            (Vector4::new(2.0, 3.0, 11.0, 1.0), false),
        ] {
            let parent = affine * local;
            assert_eq!(
                child_access(&mut tree, child).is_inside(
                    Point {
                        x: parent.x,
                        y: parent.y,
                        z: parent.z,
                    },
                    &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
                ),
                Some(expected),
                "affine={affine:?}, local={local:?}",
            );
        }
    }
}

#[test]
fn translated_hit_test_includes_local_bounds_edges() {
    let (mut tree, _, child) = setup(ProbeView(Arc::default()));
    // Exact inverses avoid rounding at the boundary; arbitrary affine transforms above use
    // interior/exterior points so they don't impose a floating-point tolerance policy.
    child_access(&mut tree, child).layout(
        position(translation(100.0, 20.0, 7.0)),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
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
        assert_eq!(
            child_access(&mut tree, child).is_inside(
                point,
                &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
            ),
            Some(true)
        );
    }
}

#[test]
fn hit_test_uses_updated_affine_and_bounds() {
    let observations = Arc::default();
    let (mut tree, _, child) = setup(ProbeView(Arc::clone(&observations)));
    let point = |x| Point { x, y: 5.0, z: 5.0 };
    child_access(&mut tree, child).layout(
        position(translation(100.0, 0.0, 0.0)),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(
        child_access(&mut tree, child).is_inside(
            point(105.0),
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        ),
        Some(true)
    );
    let mut updated = position(translation(200.0, 0.0, 0.0));
    child_access(&mut tree, child).layout(
        updated,
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(observations.lock().unwrap().layout_calls, 1);
    let _ = tree.input(
        &mouse_event(205.0, 5.0),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(
        observations.lock().unwrap().events[0].mouse_position(),
        Some([5.0, 5.0])
    );
    assert_eq!(
        child_access(&mut tree, child).is_inside(
            point(105.0),
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        ),
        Some(false)
    );
    assert_eq!(
        child_access(&mut tree, child).is_inside(
            point(205.0),
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        ),
        Some(true)
    );
    assert_eq!(
        child_access(&mut tree, child).is_inside(
            point(215.0),
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        ),
        Some(false)
    );
    updated.bounds.x = 20.0;
    child_access(&mut tree, child).layout(
        updated,
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    child_access(&mut tree, child).layout(
        updated,
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(
        child_access(&mut tree, child).is_inside(
            point(215.0),
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        ),
        Some(true)
    );
    let observations = observations.lock().unwrap();
    assert_eq!(observations.layout_calls, 2);
    assert_eq!(observations.layout_bounds, vec![bounds(), updated.bounds]);
}

#[test]
fn invalidating_layout_preserves_placement_and_recomputes_same_bounds() {
    let observations = Arc::default();
    let (mut tree, _, child) = setup(ProbeView(Arc::clone(&observations)));
    let placed = position(translation(100.0, 0.0, 0.0));
    child_access(&mut tree, child).layout(
        placed,
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    tree.node_ref_mut(child)
        .unwrap()
        .invalidate_caches(UpdateResult::LAYOUT);

    assert_eq!(
        child_access(&mut tree, child).is_inside(
            Point {
                x: 105.0,
                y: 5.0,
                z: 0.0
            },
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        ),
        Some(true)
    );
    let _ = tree.input(
        &mouse_event(105.0, 5.0),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(
        observations.lock().unwrap().events[0].mouse_position(),
        Some([5.0, 5.0])
    );
    child_access(&mut tree, child).layout(
        placed,
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    child_access(&mut tree, child).layout(
        placed,
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(observations.lock().unwrap().layout_calls, 2);
}

#[test]
fn moving_container_reuses_descendant_layout() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ContainerView {
        child: ProbeView(Arc::clone(&observations)),
        child_position: position(translation(10.0, 0.0, 0.0)),
    });
    child_access(&mut tree, child).layout(
        position(translation(100.0, 0.0, 0.0)),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    child_access(&mut tree, child).layout(
        position(translation(200.0, 0.0, 0.0)),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(observations.lock().unwrap().layout_calls, 1);
    assert_eq!(
        tree.hit_test(
            root,
            Point {
                x: 215.0,
                y: 5.0,
                z: 0.0
            },
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        )
        .is_some(),
        true
    );
    assert_eq!(
        tree.hit_test(
            root,
            Point {
                x: 115.0,
                y: 5.0,
                z: 0.0
            },
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        )
        .is_some(),
        false
    );
    let _ = tree.input(
        &mouse_event(215.0, 5.0),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(
        observations.lock().unwrap().events[0].mouse_position(),
        Some([5.0, 5.0])
    );
}

#[test]
fn singular_hit_test_returns_none_and_recovers_after_layout() {
    let (mut tree, _, child) = setup(ProbeView(Arc::default()));
    let point = Point {
        x: 5.0,
        y: 5.0,
        z: 5.0,
    };
    child_access(&mut tree, child).layout(
        position(Matrix4::new_scaling(0.0)),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(
        child_access(&mut tree, child).is_inside(
            point,
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        ),
        None
    );
    child_access(&mut tree, child).layout(
        position(Matrix4::identity()),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(
        child_access(&mut tree, child).is_inside(
            point,
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        ),
        Some(true)
    );
}

#[test]
fn child_and_tree_hit_tests_reject_unusable_local_coordinates() {
    let (mut tree, _, child) = setup(ProbeView(Arc::default()));
    let mut registry = crate::ui_tree::window::WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    for (affine, point) in [
        (
            Matrix4::identity(),
            Point {
                x: f32::NAN,
                y: 1.0,
                z: 0.0,
            },
        ),
        (
            Matrix4::identity(),
            Point {
                x: 1.0,
                y: f32::INFINITY,
                z: 0.0,
            },
        ),
        // Finite parent coordinates can still overflow during the inverse transform.
        (
            translation(-f32::MAX, 0.0, 0.0),
            Point {
                x: f32::MAX,
                y: 1.0,
                z: 0.0,
            },
        ),
        (
            Matrix4::new_scaling(0.0),
            Point {
                x: 1.0,
                y: 1.0,
                z: 0.0,
            },
        ),
    ] {
        child_access(&mut tree, child).layout(position(affine), &mut context);
        assert_eq!(
            child_access(&mut tree, child).is_inside(point, &mut context),
            None
        );
        assert_eq!(tree.hit_test(child, point, &mut context), None);
    }
}

#[test]
fn unplaced_and_removed_children_do_not_receive_input() {
    let observations = Arc::default();
    let (mut tree, _, child) = setup(ProbeView(Arc::clone(&observations)));
    let event = mouse_event(5.0, 5.0);
    let point = Point {
        x: 5.0,
        y: 5.0,
        z: 0.0,
    };
    {
        assert_eq!(
            child_access(&mut tree, child).is_inside(
                point,
                &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
            ),
            None
        );
        let _ = tree.input(
            &event,
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
        );
        child_access(&mut tree, child).layout(
            position(Matrix4::identity()),
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
        );
    }
    // Once the old child is removed, subsequent input cannot reach its widget.
    tree.update(
        &ProbeView(Arc::default()),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert!(tree.node_ref(child).is_none());
    let _ = tree.input(
        &event,
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
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
    child_access(&mut tree, child).layout(
        position(translation(100.0, 50.0, 0.0) * quarter_turn()),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    // Leaf-local (2, 3) -> container-local (14, 14) -> root-local (86, 64).
    assert_eq!(
        tree.hit_test(
            root,
            Point {
                x: 86.0,
                y: 64.0,
                z: 0.0
            },
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        )
        .is_some(),
        true
    );
    // Leaf-local (12, 3) is outside the leaf's bounds.
    assert_eq!(
        tree.hit_test(
            root,
            Point {
                x: 86.0,
                y: 84.0,
                z: 0.0
            },
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        )
        .is_some(),
        false
    );
}

#[test]
fn nested_input_preserves_raw_event_and_delivers_local_mouse_position() {
    let observations = Arc::default();
    let (mut tree, _, child) = setup(ContainerView {
        child: ProbeView(Arc::clone(&observations)),
        child_position: position(
            translation(10.0, 5.0, 0.0)
                * Matrix4::new_nonuniform_scaling(&Vector3::new(2.0, 3.0, 1.0)),
        ),
    });
    child_access(&mut tree, child).layout(
        position(translation(100.0, 50.0, 0.0) * quarter_turn()),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    let event = mouse_event(86.0, 64.0);
    let _ = tree.input(
        &event,
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
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
    let (mut tree, _, child) = setup(ProbeView(Arc::clone(&observations)));
    child_access(&mut tree, child).layout(
        position(Matrix4::identity()),
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    let event = DeviceEvent::stateless(DeviceEventData::ModifiersChanged(Modifiers::default()));
    assert_eq!(
        tree.input(
            &event,
            &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default())
        ),
        InputResult::default()
    );
    let observations = observations.lock().unwrap();
    assert!(observations.events.is_empty());
}
