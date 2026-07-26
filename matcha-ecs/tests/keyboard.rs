//! Headless verification of keyboard/IME delivery along the focus path, and of
//! focus enter/leave notification. No window, no GPU — same style as
//! `tests/focus.rs`.
//!
//! The property under test throughout is that delivery is a **root→leaf
//! capture walk**: an ancestor sees every event before its descendants and can
//! swallow it. That is the direction the focus model stores its path in, and
//! this is its first consumer.

use std::sync::{Arc, Mutex};

use bevy_ecs::{
    bundle::Bundle,
    component::Component,
    entity::Entity,
    world::{EntityWorldMut, World},
};

use matcha_ecs::{
    components::{
        focus::{FocusDispatch, FocusPolicy},
        input::{ImeDispatch, KeyDispatch, Pickable},
        view::ViewChildren,
    },
    focus::{clear_focus, request_focus, sync_focus_components, Focus},
    keyboard::{dispatch_ime, dispatch_key},
    layout::{layout_root, Constraints},
    view::{run_view, Widget},
};
use matcha_ecs_widgets::{ColorRect, Container};
use matcha_window::event::device_event::{
    ElementState, ImeEvent, Key, KeyCode, KeyInput, KeyLocation, KeyboardState, PhysicalKey,
};

// ---------------------------------------------------------------------------
// A recording sink: every handler appends its own label to a shared log
// ---------------------------------------------------------------------------

/// Shared, order-preserving record of which handlers ran, in which order.
type Log = Arc<Mutex<Vec<String>>>;

/// The component a test widget carries so its handlers know who they are and
/// where to record. `consume` decides whether the handler swallows the event.
#[derive(Component, Clone)]
struct Sink {
    label: String,
    log: Log,
    consume: bool,
}

impl Sink {
    fn record(entity: &EntityWorldMut, what: &str) -> bool {
        let Some(sink) = entity.get::<Sink>() else {
            return false;
        };
        sink.log
            .lock()
            .expect("test log mutex is never poisoned")
            .push(format!("{}:{what}", sink.label));
        sink.consume
    }
}

fn on_key(entity: &mut EntityWorldMut, input: &KeyInput) -> bool {
    let what = match input.text() {
        Some(text) => format!("key({text})"),
        None => "key".to_string(),
    };
    Sink::record(entity, &what)
}

fn on_ime(entity: &mut EntityWorldMut, event: &ImeEvent) -> bool {
    let what = match event {
        ImeEvent::Enabled => "ime(enabled)".to_string(),
        ImeEvent::Preedit { text, .. } => format!("ime(preedit:{text})"),
        ImeEvent::Commit { text } => format!("ime(commit:{text})"),
        ImeEvent::Disabled => "ime(disabled)".to_string(),
    };
    Sink::record(entity, &what)
}

fn on_focus(entity: &mut EntityWorldMut, gained: bool) {
    let what = if gained { "focus(gained)" } else { "focus(lost)" };
    Sink::record(entity, what);
}

/// A focusable, pickable widget that records every event it is offered.
struct Recorder {
    inner: ColorRect,
    sink: Sink,
}

impl Recorder {
    fn new(label: &str, log: &Log, consume: bool) -> Self {
        Self {
            inner: ColorRect::new(50.0, 50.0),
            sink: Sink {
                label: label.to_string(),
                log: log.clone(),
                consume,
            },
        }
    }
}

impl Widget for Recorder {
    fn bundle(&self) -> impl Bundle {
        (
            self.inner.bundle(),
            Pickable,
            FocusPolicy::Normal,
            self.sink.clone(),
            KeyDispatch::new(on_key),
            ImeDispatch::new(on_ime),
            FocusDispatch::new(on_focus),
        )
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

/// A container version of [`Recorder`], so tests can nest one inside another.
struct RecorderBox {
    inner: Container,
    sink: Sink,
}

impl RecorderBox {
    fn new(label: &str, log: &Log, consume: bool) -> Self {
        Self {
            inner: Container::new(),
            sink: Sink {
                label: label.to_string(),
                log: log.clone(),
                consume,
            },
        }
    }
}

impl Widget for RecorderBox {
    fn bundle(&self) -> impl Bundle {
        (
            self.inner.bundle(),
            Pickable,
            FocusPolicy::Normal,
            self.sink.clone(),
            KeyDispatch::new(on_key),
            ImeDispatch::new(on_ime),
            FocusDispatch::new(on_focus),
        )
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

/// A plain container carrying no dispatch at all — delivery must walk past it.
struct PlainBox {
    inner: Container,
}

impl PlainBox {
    fn new() -> Self {
        Self {
            inner: Container::new(),
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

/// A synthetic key press. Shape matches what `winit_interface` produces.
fn key(text: &str) -> KeyInput {
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

fn entries(log: &Log) -> Vec<String> {
    log.lock()
        .expect("test log mutex is never poisoned")
        .clone()
}

// ---------------------------------------------------------------------------
// Delivery order
// ---------------------------------------------------------------------------

/// The focus path is walked root→leaf, so an ancestor is offered the event
/// before the focused leaf. Entities on the path with no handler are skipped
/// silently.
#[test]
fn key_delivery_walks_the_focus_path_from_root_to_leaf() {
    let log: Log = Arc::default();
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(RecorderBox::new("outer", &log, false), |s| {
            s.node(PlainBox::new(), |s| {
                s.leaf(Recorder::new("leaf", &log, false));
            });
        });
    });

    let outer = children(&world, root)[0];
    let plain = children(&world, outer)[0];
    let leaf = children(&world, plain)[0];

    request_focus(&mut world, leaf);
    assert_eq!(world.resource::<Focus>().top(), Some(leaf));

    let consumed = dispatch_key(&mut world, &key("a"));

    assert_eq!(
        entries(&log),
        vec!["outer:key(a)", "leaf:key(a)"],
        "the ancestor sees the event first; the handler-less container is skipped"
    );
    assert!(!consumed, "nobody claimed it");
}

/// An ancestor returning `true` swallows the event: descendants never see it.
/// This is the "parent has full control over its subtree" case the capture
/// direction exists for.
#[test]
fn an_ancestor_returning_true_stops_delivery_to_descendants() {
    let log: Log = Arc::default();
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(RecorderBox::new("outer", &log, true), |s| {
            s.leaf(Recorder::new("leaf", &log, false));
        });
    });

    let outer = children(&world, root)[0];
    let leaf = children(&world, outer)[0];

    request_focus(&mut world, leaf);
    let consumed = dispatch_key(&mut world, &key("a"));

    assert_eq!(entries(&log), vec!["outer:key(a)"]);
    assert!(consumed);
}

/// With nothing focused there is no path, so nothing is delivered anywhere.
#[test]
fn delivery_is_a_no_op_when_nothing_has_focus() {
    let log: Log = Arc::default();
    let mut world = World::new();
    setup(&mut world, |s| {
        s.leaf(Recorder::new("leaf", &log, false));
    });

    assert!(!dispatch_key(&mut world, &key("a")));
    assert!(!dispatch_ime(&mut world, &ImeEvent::Enabled));
    assert!(entries(&log).is_empty());
}

// ---------------------------------------------------------------------------
// IME
// ---------------------------------------------------------------------------

/// A full composition session reaches the focused widget in order, through the
/// same path walk the keyboard uses.
#[test]
fn ime_composition_reaches_the_focused_widget_in_order() {
    let log: Log = Arc::default();
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.leaf(Recorder::new("editor", &log, true));
    });
    let editor = children(&world, root)[0];
    request_focus(&mut world, editor);

    for event in [
        ImeEvent::Enabled,
        ImeEvent::Preedit {
            text: "にほん".into(),
            cursor: Some((0, 9)),
        },
        ImeEvent::Commit {
            text: "日本".into(),
        },
        ImeEvent::Disabled,
    ] {
        assert!(dispatch_ime(&mut world, &event), "the editor consumes IME");
    }

    assert_eq!(
        entries(&log),
        vec![
            "editor:ime(enabled)",
            "editor:ime(preedit:にほん)",
            "editor:ime(commit:日本)",
            "editor:ime(disabled)",
        ]
    );
}

/// Key and IME dispatch are independent: a widget that handles only one of them
/// is transparent to the other.
#[test]
fn a_widget_without_an_ime_handler_is_transparent_to_ime() {
    /// Keyboard only — no `ImeDispatch`.
    struct KeyOnly {
        inner: ColorRect,
        sink: Sink,
    }
    impl Widget for KeyOnly {
        fn bundle(&self) -> impl Bundle {
            (
                self.inner.bundle(),
                Pickable,
                FocusPolicy::Normal,
                self.sink.clone(),
                KeyDispatch::new(on_key),
            )
        }
        fn patch(&self, _entity: &mut EntityWorldMut) {}
    }

    let log: Log = Arc::default();
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.leaf(KeyOnly {
            inner: ColorRect::new(50.0, 50.0),
            sink: Sink {
                label: "keyonly".into(),
                log: log.clone(),
                consume: true,
            },
        });
    });
    let entity = children(&world, root)[0];
    request_focus(&mut world, entity);

    assert!(dispatch_key(&mut world, &key("a")));
    assert!(!dispatch_ime(&mut world, &ImeEvent::Enabled));
    assert_eq!(entries(&log), vec!["keyonly:key(a)"]);
}

// ---------------------------------------------------------------------------
// Focus enter/leave notification
// ---------------------------------------------------------------------------

/// `FocusDispatch` fires in both directions. The "lost" half is the reason this
/// lives in `sync_focus_components` rather than in a `Changed<Focused>` system:
/// `Changed` does not fire on component removal.
#[test]
fn focus_dispatch_fires_on_both_gaining_and_losing_focus() {
    let log: Log = Arc::default();
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.leaf(Recorder::new("first", &log, false));
        s.leaf(Recorder::new("second", &log, false));
    });
    let [first, second]: [Entity; 2] = children(&world, root).try_into().unwrap();

    request_focus(&mut world, first);
    sync_focus_components(&mut world);
    assert_eq!(entries(&log), vec!["first:focus(gained)"]);

    request_focus(&mut world, second);
    sync_focus_components(&mut world);
    assert_eq!(
        entries(&log),
        vec![
            "first:focus(gained)",
            "first:focus(lost)",
            "second:focus(gained)",
        ]
    );

    clear_focus(&mut world);
    sync_focus_components(&mut world);
    assert_eq!(
        entries(&log).last().map(String::as_str),
        Some("second:focus(lost)")
    );
}

/// Re-syncing without a focus change must not re-fire the notification, or a
/// widget would restart its input session every frame.
#[test]
fn focus_dispatch_does_not_refire_while_focus_is_unchanged() {
    let log: Log = Arc::default();
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.leaf(Recorder::new("only", &log, false));
    });
    let only = children(&world, root)[0];

    request_focus(&mut world, only);
    for _ in 0..3 {
        sync_focus_components(&mut world);
    }

    assert_eq!(entries(&log), vec!["only:focus(gained)"]);
}

/// Only the focus *vertex* is notified, not every `:focus-within` ancestor —
/// the hook exists for widgets that own an input session, which is a property
/// of being focused, not of containing something focused.
#[test]
fn focus_dispatch_notifies_only_the_vertex_not_focus_within_ancestors() {
    let log: Log = Arc::default();
    let mut world = World::new();
    let root = setup(&mut world, |s| {
        s.node(RecorderBox::new("outer", &log, false), |s| {
            s.leaf(Recorder::new("leaf", &log, false));
        });
    });
    let outer = children(&world, root)[0];
    let leaf = children(&world, outer)[0];

    request_focus(&mut world, leaf);
    sync_focus_components(&mut world);

    assert_eq!(world.resource::<Focus>().top(), Some(leaf));
    assert!(
        world.resource::<Focus>().is_focus_within(outer),
        "the ancestor is focus-within"
    );
    assert_eq!(
        entries(&log),
        vec!["leaf:focus(gained)"],
        "but only the vertex is notified"
    );
}
