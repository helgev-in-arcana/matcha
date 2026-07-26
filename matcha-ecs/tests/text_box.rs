//! Headless verification of `TextBox`: the declared-value sync rule, keyboard
//! editing, IME composition, and message emission. No window, no GPU — the
//! `RenderItem` builder is never invoked, matching this suite's convention.
//!
//! The rule most worth pinning is the one that makes a stateful widget safe in
//! a declarative tree: **the buffer is overwritten only when the app's declared
//! value actually changes**, never when the same value is re-declared on a
//! later view pass.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{
        input::KeyDispatch,
        view::ViewChildren,
    },
    focus::{request_focus, Focus},
    input::MessageQueue,
    keyboard::{dispatch_ime, dispatch_key},
    layout::{layout_root, Constraints},
    view::run_view,
};
use matcha_ecs_widgets::{TextBox, TextEditor};
use matcha_window::event::device_event::{
    ElementState, ImeEvent, Key, KeyCode, KeyInput, KeyLocation, KeyboardState, NamedKey,
    PhysicalKey,
};

#[derive(Clone, PartialEq, Debug)]
enum Msg {
    Edited(String),
    Confirmed(String),
}

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

/// Build a world containing one `TextBox` with the given declared value, and
/// give it focus so keyboard delivery reaches it.
fn setup(value: &str) -> (World, Entity, Entity) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    let owned = value.to_string();
    run_view(&mut world, root, |s| {
        s.leaf(
            TextBox::<Msg>::new(200.0, 80.0)
                .value(owned.clone())
                .on_update(|text| Msg::Edited(text.to_string()))
                .on_confirm(|text| Msg::Confirmed(text.to_string())),
        );
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));
    world.insert_resource(Focus::default());

    let text_box = children(&world, root)[0];
    request_focus(&mut world, text_box);
    (world, root, text_box)
}

/// Re-run the view declaring `value`, the way a frame would.
fn redeclare(world: &mut World, root: Entity, value: &str) {
    let owned = value.to_string();
    run_view(world, root, |s| {
        s.leaf(
            TextBox::<Msg>::new(200.0, 80.0)
                .value(owned.clone())
                .on_update(|text| Msg::Edited(text.to_string()))
                .on_confirm(|text| Msg::Confirmed(text.to_string())),
        );
    });
}

fn text_of(world: &World, entity: Entity) -> String {
    world
        .get::<TextEditor>(entity)
        .expect("a TextBox always carries its editor")
        .text()
}

fn character(text: &str) -> KeyInput {
    KeyInput {
        physical_key: PhysicalKey::Code(KeyCode::KeyA),
        logical_key: Key::Character(text.into()),
        text: Some(text.to_string()),
        location: KeyLocation::Standard,
        state: ElementState::Pressed(0),
        repeat: false,
        snapshot: KeyboardState::default(),
    }
}

fn named(key: NamedKey) -> KeyInput {
    KeyInput {
        physical_key: PhysicalKey::Code(KeyCode::Backspace),
        logical_key: Key::Named(key),
        text: None,
        location: KeyLocation::Standard,
        state: ElementState::Pressed(0),
        repeat: false,
        snapshot: KeyboardState::default(),
    }
}

fn queued(world: &mut World) -> Vec<Msg> {
    world
        .get_resource_mut::<MessageQueue<Msg>>()
        .map(|mut q| q.drain())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The declared-value sync rule
// ---------------------------------------------------------------------------

/// The initial declared value seeds the buffer.
#[test]
fn the_declared_value_seeds_the_buffer() {
    let (world, _root, text_box) = setup("hello");
    assert_eq!(text_of(&world, text_box), "hello");
}

/// Re-declaring the *same* value must not discard what the user typed. This is
/// what makes a stateful widget safe inside a declarative view that re-runs on
/// every unrelated model change.
#[test]
fn redeclaring_an_unchanged_value_does_not_clobber_user_edits() {
    let (mut world, root, text_box) = setup("hello");

    dispatch_key(&mut world, &character("!"));
    let typed = text_of(&world, text_box);
    assert_ne!(typed, "hello", "the keystroke landed");

    redeclare(&mut world, root, "hello");

    assert_eq!(
        text_of(&world, text_box),
        typed,
        "the same declared value must leave the buffer alone"
    );
}

/// A genuinely different declared value *does* overwrite the buffer — this is
/// how an app resets a field or loads new content into it.
#[test]
fn a_changed_declared_value_overwrites_the_buffer() {
    let (mut world, root, text_box) = setup("hello");
    dispatch_key(&mut world, &character("!"));

    redeclare(&mut world, root, "replaced");

    assert_eq!(text_of(&world, text_box), "replaced");
}

// ---------------------------------------------------------------------------
// Keyboard editing
// ---------------------------------------------------------------------------

/// Typing inserts at the caret and queues the app's update message carrying the
/// resulting text — the whole reason `Message` had to become `Clone`.
#[test]
fn typing_inserts_text_and_queues_an_update_message_carrying_it() {
    let (mut world, _root, text_box) = setup("");

    for ch in ["a", "b", "c"] {
        assert!(dispatch_key(&mut world, &character(ch)), "consumed");
    }

    assert_eq!(text_of(&world, text_box), "abc");
    assert_eq!(
        queued(&mut world),
        vec![
            Msg::Edited("a".into()),
            Msg::Edited("ab".into()),
            Msg::Edited("abc".into()),
        ]
    );
}

/// Backspace deletes backwards from the caret.
#[test]
fn backspace_deletes_the_preceding_character() {
    let (mut world, _root, text_box) = setup("");
    for ch in ["a", "b"] {
        dispatch_key(&mut world, &character(ch));
    }
    let _ = queued(&mut world);

    dispatch_key(&mut world, &named(NamedKey::Backspace));

    assert_eq!(text_of(&world, text_box), "a");
    assert_eq!(queued(&mut world), vec![Msg::Edited("a".into())]);
}

/// Enter inserts a newline rather than confirming: v1 is multi-line, so
/// confirmation needs its own chord (Ctrl+Enter) or focus loss.
#[test]
fn enter_inserts_a_newline_instead_of_confirming() {
    let (mut world, _root, text_box) = setup("");
    dispatch_key(&mut world, &character("a"));
    let _ = queued(&mut world);

    dispatch_key(&mut world, &named(NamedKey::Enter));

    assert_eq!(text_of(&world, text_box), "a\n");
    assert_eq!(
        queued(&mut world),
        vec![Msg::Edited("a\n".into())],
        "a newline is an edit, not a confirmation"
    );
}

/// Keys the box does not handle are left for something else — an unhandled key
/// must not be silently swallowed just because a text box has focus.
#[test]
fn unhandled_keys_are_not_consumed() {
    let (mut world, _root, _text_box) = setup("");
    assert!(!dispatch_key(&mut world, &named(NamedKey::Tab)));
    assert!(!dispatch_key(&mut world, &named(NamedKey::F1)));
}

// ---------------------------------------------------------------------------
// IME
// ---------------------------------------------------------------------------

/// A composition shows its preedit inline while in progress, and the committed
/// text replaces it. `TextEditor::text()` deliberately excludes the preedit, so
/// an app never sees unconfirmed characters.
#[test]
fn ime_composition_commits_only_the_confirmed_text() {
    let (mut world, _root, text_box) = setup("");

    dispatch_ime(&mut world, &ImeEvent::Enabled);
    dispatch_ime(
        &mut world,
        &ImeEvent::Preedit {
            text: "にほん".into(),
            cursor: Some((0, 9)),
        },
    );
    assert_eq!(
        text_of(&world, text_box),
        "",
        "an in-progress preedit is not part of the committed text"
    );

    dispatch_ime(
        &mut world,
        &ImeEvent::Commit {
            text: "日本".into(),
        },
    );
    dispatch_ime(&mut world, &ImeEvent::Disabled);

    assert_eq!(text_of(&world, text_box), "日本");
    assert!(queued(&mut world).contains(&Msg::Edited("日本".into())));
}

/// An abandoned composition (empty preedit, no commit) leaves nothing behind.
#[test]
fn an_abandoned_composition_leaves_the_text_untouched() {
    let (mut world, _root, text_box) = setup("start");

    dispatch_ime(&mut world, &ImeEvent::Enabled);
    dispatch_ime(
        &mut world,
        &ImeEvent::Preedit {
            text: "か".into(),
            cursor: Some((0, 3)),
        },
    );
    dispatch_ime(
        &mut world,
        &ImeEvent::Preedit {
            text: String::new(),
            cursor: None,
        },
    );
    dispatch_ime(&mut world, &ImeEvent::Disabled);

    assert_eq!(text_of(&world, text_box), "start");
}

/// While the IME is composing it owns the keyboard: raw keys must not also be
/// inserted, or every composed character would be typed twice.
#[test]
fn keys_are_swallowed_while_the_ime_is_composing() {
    let (mut world, _root, text_box) = setup("");

    dispatch_ime(&mut world, &ImeEvent::Enabled);
    dispatch_ime(
        &mut world,
        &ImeEvent::Preedit {
            text: "か".into(),
            cursor: Some((0, 3)),
        },
    );
    let _ = queued(&mut world);

    assert!(
        dispatch_key(&mut world, &character("k")),
        "consumed, so nothing else acts on it"
    );
    assert_eq!(text_of(&world, text_box), "");
    assert!(queued(&mut world).is_empty(), "no edit was made");
}

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------

/// The box opts into keyboard delivery, and nothing reaches it without focus.
#[test]
fn keyboard_delivery_requires_focus() {
    let (mut world, _root, text_box) = setup("");
    assert!(world.get::<KeyDispatch>(text_box).is_some());

    matcha_ecs::focus::clear_focus(&mut world);
    assert!(!dispatch_key(&mut world, &character("a")));
    assert_eq!(text_of(&world, text_box), "");
}
