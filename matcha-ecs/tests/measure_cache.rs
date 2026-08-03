//! The measure cache: repeated measurement of the same child against the same
//! constraints must resolve to one call to that child's `Layout::measure`.
//!
//! This matters because resolving a layout legitimately measures the same child
//! more than once — `Column::arrange` re-measures every child after
//! `Column::measure` already did, and intrinsic sizing needs a second pass by
//! construction. Without a cache those repeats multiply with depth, which is
//! what would make flex distribution and grid unaffordable.
//!
//! Headless: a bare `World` driven through `layout_root`, no window or GPU.

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use bevy_ecs::{bundle::Bundle, component::Component, entity::Entity, world::EntityWorldMut};
use bevy_ecs::world::World;

use matcha_ecs::{
    components::view::{Key, ViewChildren},
    layout::{layout_root, Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
    view::{run_view, Widget},
};
use matcha_ecs_widgets::{AlignItems, Column};

/// A fixed-size leaf that counts how often it is measured.
#[derive(Component, Clone)]
struct CountingLeaf {
    size: [f32; 2],
    measures: Arc<AtomicU32>,
}

impl Layout for CountingLeaf {
    fn measure(&self, _ctx: &mut LayoutCtx, _me: Entity, c: Constraints) -> Measured {
        self.measures.fetch_add(1, Ordering::Relaxed);
        Measured::exact([
            self.size[0].clamp(c.min_width(), c.max_width()),
            self.size[1].clamp(c.min_height(), c.max_height()),
        ])
    }

    fn arrange(&self, _ctx: &mut LayoutCtx, _me: Entity, _size: [f32; 2]) {}
}

/// Widget wrapper so the leaf can be declared through a `Scope` like any other.
struct Counter {
    size: [f32; 2],
    measures: Arc<AtomicU32>,
}

impl Widget for Counter {
    fn key(&self) -> Key {
        Key::Auto
    }

    fn bundle(&self) -> impl Bundle {
        (
            CountingLeaf {
                size: self.size,
                measures: self.measures.clone(),
            },
            LayoutDispatch::of::<CountingLeaf>(),
        )
    }

    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

fn setup() -> (World, Entity) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    (world, root)
}

/// Resolving a layout asks a child exactly two distinct questions, and the
/// cache is what stops each from being re-asked at every level above it.
///
/// The two are genuinely different, which is why the count is 2 and not 1:
/// the measure pass hands a child the *incoming* maximum (here the window's
/// 800×600), while the arrange pass hands it the size the parent actually
/// resolved to (here the column's fit-content 50×20).
const DISTINCT_QUESTIONS_PER_CHILD: u32 = 2;

#[test]
fn the_measure_pass_and_the_arrange_pass_each_measure_a_child_once() {
    let (mut world, root) = setup();
    let measures = Arc::new(AtomicU32::new(0));

    // `AlignItems::Start` so the arrange pass asks only its own question;
    // `Stretch` deliberately adds a third (cross axis pinned) — see below.
    let m = measures.clone();
    run_view(&mut world, root, move |s| {
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            s.leaf(Counter {
                size: [50.0, 20.0],
                measures: m.clone(),
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    assert_eq!(
        measures.load(Ordering::Relaxed),
        DISTINCT_QUESTIONS_PER_CHILD
    );
}

#[test]
fn a_different_constraint_is_a_genuine_second_question_and_is_not_served_from_cache() {
    let (mut world, root) = setup();
    let measures = Arc::new(AtomicU32::new(0));

    // `AlignItems::Stretch` re-measures each child with the cross axis pinned
    // to the container's width — a different `Constraints`, so a different
    // cache entry. Caching must not collapse it into the natural measurement.
    let m = measures.clone();
    run_view(&mut world, root, move |s| {
        s.node(Column::new().align_items(AlignItems::Stretch), |s| {
            s.leaf(Counter {
                size: [50.0, 20.0],
                measures: m.clone(),
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    assert_eq!(
        measures.load(Ordering::Relaxed),
        DISTINCT_QUESTIONS_PER_CHILD + 1
    );
}

#[test]
fn repeat_measurement_does_not_multiply_with_nesting_depth() {
    // This is the test the cache exists for. Each `Column` level measures its
    // child once in `measure` and once in `arrange`, so uncached the calls
    // double per level: a leaf under N levels costs 2^N. Cached, every level
    // of the measure pass asks the same question (each passes the incoming
    // max straight down) and so does every level of the arrange pass (every
    // column here resolves to the same fit-content size), leaving exactly the
    // two distinct questions no matter how deep the tree is.
    const DEPTH: usize = 8;

    let (mut world, root) = setup();
    let measures = Arc::new(AtomicU32::new(0));

    fn nest(s: &mut matcha_ecs::view::Scope, depth: usize, measures: &Arc<AtomicU32>) {
        if depth == 0 {
            s.leaf(Counter {
                size: [50.0, 20.0],
                measures: measures.clone(),
            });
            return;
        }
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            nest(s, depth - 1, measures);
        });
    }

    let m = measures.clone();
    run_view(&mut world, root, move |s| nest(s, DEPTH, &m));
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let count = measures.load(Ordering::Relaxed);
    assert_eq!(
        count,
        DISTINCT_QUESTIONS_PER_CHILD,
        "at depth {DEPTH} the uncached cost would be 2^{DEPTH} = {}, got {count}",
        1u32 << DEPTH
    );
}

#[test]
fn the_cache_does_not_survive_into_the_next_layout_pass() {
    // The cache lives on `LayoutCtx`, which `layout_root` builds and drops, so
    // a second pass re-measures from scratch. This is what keeps a changed
    // widget from being served last frame's size.
    let (mut world, root) = setup();
    let measures = Arc::new(AtomicU32::new(0));

    let m = measures.clone();
    run_view(&mut world, root, move |s| {
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            s.leaf(Counter {
                size: [50.0, 20.0],
                measures: m.clone(),
            });
        });
    });

    let constraints = Constraints::from_max_size([800.0, 600.0]);
    layout_root(&mut world, root, constraints);
    assert_eq!(
        measures.load(Ordering::Relaxed),
        DISTINCT_QUESTIONS_PER_CHILD
    );

    layout_root(&mut world, root, constraints);
    assert_eq!(
        measures.load(Ordering::Relaxed),
        DISTINCT_QUESTIONS_PER_CHILD * 2
    );
}
