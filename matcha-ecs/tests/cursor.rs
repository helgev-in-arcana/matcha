//! Cursor shape resolution over the hover chain.
//!
//! The chain is already resolved by `pointer::resolve`; what is checked here is
//! the *direction* of the search (leaf to root, so the innermost opinion wins)
//! and that an entity with no opinion is skipped rather than treated as
//! "default". `sync_cursor` itself needs a window to push to, so these tests
//! call the resolution the way it does and inspect the answer.

use bevy_ecs::{bundle::Bundle, entity::Entity, world::{EntityWorldMut, World}};

use matcha_ecs::{
    components::{
        input::{Cursor, Pickable},
        view::ViewChildren,
    },
    layout::{layout_root, Constraints},
    pick::{PickerResource, RectPicker},
    pointer::{set_position, PointerState},
    view::{run_view, Widget},
};
use matcha_ecs_widgets::{AlignItems, ColorRect, Column, CursorIcon};

const WINDOW: [f32; 2] = [800.0, 600.0];

/// A pickable leaf that may or may not have an opinion about the cursor.
struct Leaf {
    inner: ColorRect,
    cursor: Option<CursorIcon>,
}

impl Widget for Leaf {
    fn bundle(&self) -> impl Bundle {
        (self.inner.bundle(), Pickable)
    }
    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        Widget::after_spawn(&self.inner, entity);
        if let Some(cursor) = self.cursor {
            entity.insert(Cursor(cursor));
        }
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

/// The same search `sync_cursor` performs: leaf to root over the hover chain.
fn resolved(world: &World) -> CursorIcon {
    world
        .resource::<PointerState>()
        .hover_path()
        .iter()
        .rev()
        .find_map(|&e| world.get::<Cursor>(e).map(|c| c.0))
        .unwrap_or_default()
}

/// A column (optionally with its own cursor) holding two leaves at y 0..50 and
/// y 50..100.
fn fixture(column_cursor: Option<CursorIcon>, leaves: [Option<CursorIcon>; 2]) -> World {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();

    run_view(&mut world, root, move |s| {
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            s.leaf(Leaf {
                inner: ColorRect::new(100.0, 50.0).key(1u64),
                cursor: leaves[0],
            });
            s.leaf(Leaf {
                inner: ColorRect::new(100.0, 50.0).key(2u64),
                cursor: leaves[1],
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    if let Some(cursor) = column_cursor {
        let column = children(&world, root)[0];
        world.entity_mut(column).insert(Cursor(cursor));
    }
    world.insert_resource(PickerResource(Box::new(RectPicker::build(&world, root))));
    world
}

#[test]
fn nothing_hovered_means_the_platform_default() {
    let mut world = fixture(None, [Some(CursorIcon::Text), None]);
    set_position(&mut world, Some([400.0, 400.0]));
    assert_eq!(resolved(&world), CursorIcon::Default);
}

#[test]
fn the_hovered_widgets_own_cursor_wins() {
    let mut world = fixture(None, [Some(CursorIcon::Text), Some(CursorIcon::Pointer)]);

    set_position(&mut world, Some([10.0, 10.0]));
    assert_eq!(resolved(&world), CursorIcon::Text);

    set_position(&mut world, Some([10.0, 70.0]));
    assert_eq!(resolved(&world), CursorIcon::Pointer);
}

#[test]
fn a_child_with_no_opinion_inherits_its_ancestors() {
    // This is what makes a cursor on a container behave like CSS's inherited
    // `cursor` without any inheritance machinery: the chain search simply keeps
    // walking up.
    let mut world = fixture(Some(CursorIcon::Grab), [None, None]);
    set_position(&mut world, Some([10.0, 10.0]));
    assert_eq!(resolved(&world), CursorIcon::Grab);
}

#[test]
fn the_innermost_opinion_beats_an_ancestors() {
    let mut world = fixture(Some(CursorIcon::Grab), [Some(CursorIcon::Text), None]);

    set_position(&mut world, Some([10.0, 10.0]));
    assert_eq!(resolved(&world), CursorIcon::Text, "the leaf overrides");

    set_position(&mut world, Some([10.0, 70.0]));
    assert_eq!(
        resolved(&world),
        CursorIcon::Grab,
        "its sibling has no opinion, so the container's still applies"
    );
}

#[test]
fn hidden_is_a_shape_like_any_other_and_reports_itself_invisible() {
    let mut world = fixture(None, [Some(CursorIcon::Hidden), None]);
    set_position(&mut world, Some([10.0, 10.0]));
    assert_eq!(resolved(&world), CursorIcon::Hidden);
    assert!(!CursorIcon::Hidden.is_visible());
    assert!(CursorIcon::Default.is_visible());
}
