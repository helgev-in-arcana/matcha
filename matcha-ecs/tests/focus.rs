//! Headless verification of the focus model: build a view, run layout, pick,
//! and assert the resolved focus path. No window, no GPU — same style as
//! `tests/input.rs`.
//!
//! The cases that matter here are the ones a *flat* focus search would get
//! wrong: an element overlapping another, a parent claiming focus away from
//! its descendants, and a container restoring focus into the child it
//! remembers.

use bevy_ecs::{
    bundle::Bundle,
    entity::Entity,
    world::{EntityWorldMut, World},
};

use matcha_ecs::{
    components::{
        focus::{FocusPolicy, LastFocusedChild},
        input::Pickable,
        view::ViewChildren,
    },
    focus::{clear_focus, focus_from_pick, resolve_focus_path, validate_focus, Focus},
    layout::{layout_root, Constraints},
    pick::{PickQuery, Picker, RectPicker},
    view::{despawn_ui_entity, run_view, Widget},
};
use matcha_ecs_widgets::{ColorRect, Column, Container};

// ---------------------------------------------------------------------------
// Test widgets: minimal leaves/containers with explicit focus behaviour
// ---------------------------------------------------------------------------

/// A pickable, focusable leaf.
struct FocusLeaf {
    inner: ColorRect,
    policy: FocusPolicy,
}

impl FocusLeaf {
    fn new(w: f32, h: f32, policy: FocusPolicy) -> Self {
        Self {
            inner: ColorRect::new(w, h),
            policy,
        }
    }
}

impl Widget for FocusLeaf {
    fn bundle(&self) -> impl Bundle {
        (self.inner.bundle(), Pickable, self.policy)
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

/// A pickable leaf that is *not* focusable — focus must walk past it.
struct PlainLeaf {
    inner: ColorRect,
}

impl PlainLeaf {
    fn new(w: f32, h: f32) -> Self {
        Self {
            inner: ColorRect::new(w, h),
        }
    }
}

impl Widget for PlainLeaf {
    fn bundle(&self) -> impl Bundle {
        (self.inner.bundle(), Pickable)
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

/// A container with a focus policy, and `LastFocusedChild` memory attached so
/// `RestoreLast` has something to read.
struct FocusBox {
    inner: Container,
    policy: FocusPolicy,
}

impl FocusBox {
    fn new(policy: FocusPolicy) -> Self {
        Self {
            inner: Container::new(),
            policy,
        }
    }
}

impl Widget for FocusBox {
    fn bundle(&self) -> impl Bundle {
        (
            self.inner.bundle(),
            Pickable,
            self.policy,
            LastFocusedChild::default(),
        )
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

/// A plain container carrying no focus policy at all — the `:focus-within`
/// pass-through case.
struct PlainBox {
    inner: Column,
}

impl PlainBox {
    fn new() -> Self {
        Self {
            inner: Column::new(),
        }
    }
}

impl Widget for PlainBox {
    fn bundle(&self) -> impl Bundle {
        self.inner.bundle()
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

fn setup(world: &mut World, view: impl FnOnce(&mut matcha_ecs::view::Scope)) -> Entity {
    let root = world.spawn(ViewChildren::default()).id();
    run_view(world, root, view);
    layout_root(world, root, Constraints::from_max_size([800.0, 600.0]));
    world.insert_resource(Focus::default());
    root
}

fn pick_at(world: &World, root: Entity, pos: [f32; 2]) -> Option<Entity> {
    RectPicker::build(world, root)
        .pick(world, &PickQuery { viewport_pos: pos })
        .map(|h| h.entity)
}

fn focus_at(world: &mut World, root: Entity, pos: [f32; 2]) -> bool {
    let hit = pick_at(world, root, pos);
    focus_from_pick(world, hit)
}

// ---------------------------------------------------------------------------
// Structure is respected even when a flat search would not be
// ---------------------------------------------------------------------------

/// A focusable leaf overlapping a *different* subtree's focusable leaf. A flat
/// z-ordered search returns only "the topmost focusable thing"; what makes the
/// result structurally correct is that the path is then rebuilt by walking up
/// from that hit, so the focused leaf's own ancestors — not the ones of
/// whatever it happens to overlap — end up on the path.
#[test]
fn focus_path_follows_the_picked_entitys_own_ancestors_not_the_overlapped_ones() {
    let mut world = World::new();
    // Both boxes are top-level, so both are arranged at window origin and
    // overlap exactly. The later-declared one paints on top.
    let root = setup(&mut world, |s| {
        s.node(PlainBox::new(), |s| {
            s.leaf(FocusLeaf::new(100.0, 100.0, FocusPolicy::Normal));
        });
        s.node(PlainBox::new(), |s| {
            s.leaf(FocusLeaf::new(60.0, 60.0, FocusPolicy::Normal));
        });
    });

    let [under_box, over_box]: [Entity; 2] = children(&world, root).try_into().unwrap();
    let under_leaf = children(&world, under_box)[0];
    let over_leaf = children(&world, over_box)[0];

    assert!(focus_at(&mut world, root, [10.0, 10.0]));
    let focus = world.resource::<Focus>();

    assert_eq!(focus.top(), Some(over_leaf));
    assert_eq!(
        focus.path(),
        &[root, over_box, over_leaf],
        "the path is the picked leaf's own ancestry"
    );
    assert!(
        !focus.is_focus_within(under_box),
        "the overlapped subtree is not focus-within just because it sits underneath"
    );
    assert!(!focus.is_focus_within(under_leaf));
}

/// `:focus-within` holds for every ancestor on the path, including ones with
/// no `FocusPolicy` of their own — a plain `Column` needs no opt-in.
#[test]
fn focus_within_holds_for_non_focusable_intermediate_ancestors() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(PlainBox::new(), |s| {
            s.node(PlainBox::new(), |s| {
                s.leaf(FocusLeaf::new(50.0, 50.0, FocusPolicy::Normal));
            });
        });
    });

    let outer = children(&world, root)[0];
    let inner = children(&world, outer)[0];
    let leaf = children(&world, inner)[0];

    focus_at(&mut world, root, [10.0, 10.0]);
    let focus = world.resource::<Focus>();

    assert_eq!(focus.top(), Some(leaf));
    assert!(focus.is_focused(leaf));
    assert!(!focus.is_focused(inner));
    for ancestor in [root, outer, inner] {
        assert!(
            focus.is_focus_within(ancestor),
            "{ancestor:?} should be focus-within"
        );
    }
}

/// A pick landing on a leaf that is not focusable resolves to the nearest
/// focusable ancestor.
#[test]
fn a_pick_on_a_non_focusable_leaf_resolves_to_its_nearest_focusable_ancestor() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(FocusBox::new(FocusPolicy::Normal), |s| {
            s.leaf(PlainLeaf::new(40.0, 40.0));
        });
    });

    let container = children(&world, root)[0];
    let leaf = children(&world, container)[0];

    assert_eq!(pick_at(&world, root, [10.0, 10.0]), Some(leaf));

    focus_at(&mut world, root, [10.0, 10.0]);
    assert_eq!(world.resource::<Focus>().top(), Some(container));
}

// ---------------------------------------------------------------------------
// FocusPolicy::Claim — the parent's dominance over its subtree
// ---------------------------------------------------------------------------

/// The text-box case: a container that owns its decorative children. Even
/// though the pick lands on a focusable descendant, the downward pass
/// truncates the path at the claimer.
#[test]
fn claim_truncates_the_path_so_descendants_never_become_the_vertex() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(FocusBox::new(FocusPolicy::Claim), |s| {
            s.leaf(FocusLeaf::new(40.0, 40.0, FocusPolicy::Normal));
        });
    });

    let claimer = children(&world, root)[0];
    let child = children(&world, claimer)[0];

    assert_eq!(
        pick_at(&world, root, [10.0, 10.0]),
        Some(child),
        "the child is still the frontmost pickable surface"
    );

    focus_at(&mut world, root, [10.0, 10.0]);
    let focus = world.resource::<Focus>();
    assert_eq!(focus.top(), Some(claimer), "the claimer takes the vertex");
    assert_eq!(focus.path(), &[root, claimer]);
    assert!(
        !focus.is_focus_within(child),
        "the claimed subtree is cut out of the path entirely"
    );
}

/// The shallowest claimer wins: an outer claim beats an inner one.
#[test]
fn the_shallowest_claimer_wins() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(FocusBox::new(FocusPolicy::Claim), |s| {
            s.node(FocusBox::new(FocusPolicy::Claim), |s| {
                s.leaf(FocusLeaf::new(30.0, 30.0, FocusPolicy::Normal));
            });
        });
    });

    let outer = children(&world, root)[0];
    focus_at(&mut world, root, [10.0, 10.0]);
    assert_eq!(world.resource::<Focus>().top(), Some(outer));
}

// ---------------------------------------------------------------------------
// FocusPolicy::RestoreLast — element-local memory
// ---------------------------------------------------------------------------

/// Clicking a container that remembers where focus was descends back into it,
/// rather than parking focus on the container. The memory is written by the
/// earlier focus of the child, entirely element-locally.
#[test]
fn restore_last_descends_into_the_remembered_child() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(FocusBox::new(FocusPolicy::RestoreLast), |s| {
            // Small, so there is container area not covered by the child.
            s.leaf(FocusLeaf::new(20.0, 20.0, FocusPolicy::Normal));
        });
    });

    let container = children(&world, root)[0];
    let child = children(&world, container)[0];

    // Focus the child directly: this is what records the memory.
    focus_at(&mut world, root, [5.0, 5.0]);
    assert_eq!(world.resource::<Focus>().top(), Some(child));
    assert_eq!(
        world.get::<LastFocusedChild>(container).unwrap().get(),
        Some(child),
        "the container remembers, element-locally"
    );

    // Now focus the container itself. It must hand focus back to the child.
    clear_focus(&mut world);
    matcha_ecs::focus::request_focus(&mut world, container);

    let focus = world.resource::<Focus>();
    assert_eq!(focus.top(), Some(child), "restored into the remembered child");
    assert_eq!(focus.path(), &[root, container, child]);
}

/// With nothing remembered yet, `RestoreLast` simply keeps the vertex on the
/// container — no descent, no panic.
#[test]
fn restore_last_with_no_memory_keeps_focus_on_the_container() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(FocusBox::new(FocusPolicy::RestoreLast), |s| {
            s.leaf(PlainLeaf::new(20.0, 20.0));
        });
    });

    let container = children(&world, root)[0];
    matcha_ecs::focus::request_focus(&mut world, container);
    assert_eq!(world.resource::<Focus>().top(), Some(container));
}

/// A remembered child that has since been despawned must not be followed.
/// Entity ids are recycled, so the memory is validated for liveness *and*
/// parentage before descending.
#[test]
fn restore_last_ignores_a_remembered_child_that_no_longer_exists() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(FocusBox::new(FocusPolicy::RestoreLast), |s| {
            s.leaf(FocusLeaf::new(20.0, 20.0, FocusPolicy::Normal));
        });
    });

    let container = children(&world, root)[0];
    let child = children(&world, container)[0];

    focus_at(&mut world, root, [5.0, 5.0]);
    assert_eq!(
        world.get::<LastFocusedChild>(container).unwrap().get(),
        Some(child)
    );

    despawn_ui_entity(&mut world, child);
    clear_focus(&mut world);
    matcha_ecs::focus::request_focus(&mut world, container);

    assert_eq!(
        world.resource::<Focus>().top(),
        Some(container),
        "the dead child is not followed"
    );
}

// ---------------------------------------------------------------------------
// Staying in sync with a changing tree
// ---------------------------------------------------------------------------

/// When the focused entity is despawned, validation falls back to the deepest
/// surviving focusable ancestor rather than leaving a dangling vertex.
#[test]
fn validate_focus_falls_back_to_the_nearest_surviving_ancestor() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(FocusBox::new(FocusPolicy::Normal), |s| {
            s.leaf(FocusLeaf::new(20.0, 20.0, FocusPolicy::Normal));
        });
    });

    let container = children(&world, root)[0];
    let child = children(&world, container)[0];

    focus_at(&mut world, root, [5.0, 5.0]);
    assert_eq!(world.resource::<Focus>().top(), Some(child));

    despawn_ui_entity(&mut world, child);
    validate_focus(&mut world, root);

    let focus = world.resource::<Focus>();
    assert_eq!(focus.top(), Some(container));
    assert!(!focus.is_focus_within(child));
}

/// Nothing focusable survives: focus clears rather than pointing at a dead
/// entity.
#[test]
fn validate_focus_clears_when_the_whole_focused_subtree_is_gone() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(PlainBox::new(), |s| {
            s.leaf(FocusLeaf::new(20.0, 20.0, FocusPolicy::Normal));
        });
    });

    let container = children(&world, root)[0];
    focus_at(&mut world, root, [5.0, 5.0]);
    assert!(world.resource::<Focus>().top().is_some());

    despawn_ui_entity(&mut world, container);
    validate_focus(&mut world, root);

    let focus = world.resource::<Focus>();
    assert_eq!(focus.top(), None);
    assert!(focus.path().is_empty());
}

// ---------------------------------------------------------------------------
// Misses
// ---------------------------------------------------------------------------

/// Pressing on empty background clears focus by default.
#[test]
fn a_press_on_nothing_clears_focus_by_default() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.leaf(FocusLeaf::new(20.0, 20.0, FocusPolicy::Normal));
    });

    focus_at(&mut world, root, [5.0, 5.0]);
    assert!(world.resource::<Focus>().top().is_some());

    // Far outside the only widget.
    assert!(focus_at(&mut world, root, [500.0, 500.0]));
    assert_eq!(world.resource::<Focus>().top(), None);
}

/// With `clear_on_miss` disabled, focus is sticky until something else claims
/// it.
#[test]
fn a_press_on_nothing_keeps_focus_when_clear_on_miss_is_disabled() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.leaf(FocusLeaf::new(20.0, 20.0, FocusPolicy::Normal));
    });
    world.insert_resource(matcha_ecs::focus::FocusConfig {
        clear_on_miss: false,
    });

    focus_at(&mut world, root, [5.0, 5.0]);
    let focused = world.resource::<Focus>().top();
    assert!(focused.is_some());

    assert!(!focus_at(&mut world, root, [500.0, 500.0]));
    assert_eq!(world.resource::<Focus>().top(), focused);
}

/// `resolve_focus_path` is pure: no pick, no resource, no writes.
#[test]
fn resolve_focus_path_is_pure_and_returns_root_first_order() {
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(PlainBox::new(), |s| {
            s.leaf(FocusLeaf::new(20.0, 20.0, FocusPolicy::Normal));
        });
    });

    let container = children(&world, root)[0];
    let leaf = children(&world, container)[0];

    let path = resolve_focus_path(&world, Some(leaf));
    assert_eq!(path, vec![root, container, leaf]);
    assert_eq!(
        world.resource::<Focus>().top(),
        None,
        "resolving must not have written anything"
    );
}
