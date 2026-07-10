//! Headless verification of the view / reconcile core: run view functions
//! against a bare `World` and assert the resulting entity tree.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::view::ViewChildren,
    view::{run_view, Scope},
};
use matcha_ecs_widgets::{Button, ButtonLabel, Column, LayoutKind, OnClick, Row, Text, TextContent};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Msg {
    Inc,
    Dec,
}

// --- helpers ---------------------------------------------------------------

fn setup() -> (World, Entity) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    (world, root)
}

fn live_count(world: &mut World) -> usize {
    world.query::<Entity>().iter(world).count()
}

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

/// All view-managed descendants of `e` (excluding `e`), pre-order.
fn descendants(world: &World, e: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    for c in children(world, e) {
        out.push(c);
        out.extend(descendants(world, c));
    }
    out
}

// --- example views ---------------------------------------------------------

fn counter(s: &mut Scope, count: i32) {
    s.node(Column::new().gap(8.0), |s| {
        s.leaf(Text::new(format!("count: {count}")));
        s.node(Row::new(), |s| {
            s.leaf(Button::new("-").on(Msg::Dec));
            s.leaf(Button::new("+").on(Msg::Inc));
        });
    });
}

/// A list keyed by id (order comes from `ids`).
fn id_list(s: &mut Scope, ids: &[u64]) {
    s.node(Column::new(), |s| {
        for &id in ids {
            s.leaf(Text::new(format!("item-{id}")).key(id));
        }
    });
}

// --- 1. spawn --------------------------------------------------------------

#[test]
fn spawn_builds_tree() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| counter(s, 5));

    let kids = children(&world, root);
    assert_eq!(kids.len(), 1);

    let col = kids[0];
    assert_eq!(world.get::<LayoutKind>(col), Some(&LayoutKind::Column));

    let col_kids = children(&world, col);
    assert_eq!(col_kids.len(), 2, "Text + Row");
    assert_eq!(
        world.get::<TextContent>(col_kids[0]),
        Some(&TextContent("count: 5".into()))
    );

    let row = col_kids[1];
    assert_eq!(world.get::<LayoutKind>(row), Some(&LayoutKind::Row));

    let row_kids = children(&world, row);
    assert_eq!(row_kids.len(), 2);
    assert_eq!(
        world.get::<ButtonLabel>(row_kids[0]),
        Some(&ButtonLabel("-".into()))
    );
    assert_eq!(
        world.get::<OnClick<Msg>>(row_kids[0]),
        Some(&OnClick(Some(Msg::Dec)))
    );
    assert_eq!(
        world.get::<OnClick<Msg>>(row_kids[1]),
        Some(&OnClick(Some(Msg::Inc)))
    );
}

// --- 2. patch is idempotent (no rebuild) -----------------------------------

#[test]
fn patch_is_idempotent() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| counter(s, 5));
    let before = descendants(&world, root);

    run_view(&mut world, root, |s| counter(s, 5));
    let after = descendants(&world, root);

    assert_eq!(before, after, "same model must reuse every entity");
    assert_eq!(live_count(&mut world), before.len() + 1); // +root
}

// --- 3. value update reuses the entity -------------------------------------

#[test]
fn value_update_patches_in_place() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| counter(s, 5));
    let text = children(&world, children(&world, root)[0])[0];

    run_view(&mut world, root, |s| counter(s, 6));
    let text_after = children(&world, children(&world, root)[0])[0];

    assert_eq!(text, text_after, "text entity must be reused");
    assert_eq!(
        world.get::<TextContent>(text_after),
        Some(&TextContent("count: 6".into()))
    );
}

// --- 4. type change rebuilds the entity ------------------------------------

#[test]
fn type_change_rebuilds() {
    let (mut world, root) = setup();

    run_view(&mut world, root, |s| s.leaf(Text::new("x")));
    let old = children(&world, root)[0];
    assert!(world.get::<TextContent>(old).is_some());

    // Same slot (Auto/occurrence 0), different widget type.
    run_view(&mut world, root, |s| s.leaf(Button::<Msg>::new("x")));
    let new = children(&world, root)[0];

    assert_ne!(old, new, "type change must despawn + respawn");
    assert!(!world.entities().contains(old), "old entity despawned");
    assert!(world.get::<ButtonLabel>(new).is_some());
    assert_eq!(live_count(&mut world), 2); // root + new
}

// --- 5. prune removes untouched children -----------------------------------

#[test]
fn prune_removes_dropped_children() {
    let (mut world, root) = setup();

    run_view(&mut world, root, |s| id_list(s, &[1, 2, 3]));
    let col = children(&world, root)[0];
    assert_eq!(children(&world, col).len(), 3);
    let dropped = children(&world, col)[2];

    run_view(&mut world, root, |s| id_list(s, &[1]));
    assert_eq!(children(&world, col).len(), 1);
    assert!(!world.entities().contains(dropped), "extra items despawned");
    // root + column + 1 item
    assert_eq!(live_count(&mut world), 3);
}

// --- 6. keyed reorder preserves identity -----------------------------------

#[test]
fn keyed_reorder_preserves_identity() {
    let (mut world, root) = setup();

    run_view(&mut world, root, |s| id_list(s, &[1, 2, 3]));
    let col = children(&world, root)[0];
    let entity_of = |world: &World, id: u64| -> Entity {
        let want = TextContent(format!("item-{id}"));
        children(world, col)
            .into_iter()
            .find(|&e| world.get::<TextContent>(e) == Some(&want))
            .expect("item present")
    };
    let (e1, e2, e3) = (
        entity_of(&world, 1),
        entity_of(&world, 2),
        entity_of(&world, 3),
    );

    // Reorder the same keys.
    run_view(&mut world, root, |s| id_list(s, &[3, 1, 2]));

    assert_eq!(entity_of(&world, 1), e1, "id 1 keeps its entity");
    assert_eq!(entity_of(&world, 2), e2, "id 2 keeps its entity");
    assert_eq!(entity_of(&world, 3), e3, "id 3 keeps its entity");

    // And the declared order now follows the new sequence.
    let ordered: Vec<Entity> = children(&world, col);
    assert_eq!(ordered, vec![e3, e1, e2]);
}

// --- 7. pruning a subtree despawns the whole branch ------------------------

#[test]
fn prune_removes_whole_subtree() {
    let (mut world, root) = setup();

    // A conditional Row (with two Buttons) alongside a Text.
    let view = |s: &mut Scope, show_row: bool| {
        s.node(Column::new(), |s| {
            s.leaf(Text::new("header"));
            if show_row {
                s.node(Row::new(), |s| {
                    s.leaf(Button::new("-").on(Msg::Dec));
                    s.leaf(Button::new("+").on(Msg::Inc));
                });
            }
        });
    };

    run_view(&mut world, root, |s| view(s, true));
    let col = children(&world, root)[0];
    let row = children(&world, col)[1];
    let buttons = children(&world, row);
    assert_eq!(buttons.len(), 2);

    // Drop the Row: the Row and both Buttons must be despawned recursively.
    run_view(&mut world, root, |s| view(s, false));
    assert_eq!(children(&world, col).len(), 1, "only header remains");
    assert!(!world.entities().contains(row));
    assert!(!world.entities().contains(buttons[0]));
    assert!(!world.entities().contains(buttons[1]));
    // root + column + header
    assert_eq!(live_count(&mut world), 3);
}
