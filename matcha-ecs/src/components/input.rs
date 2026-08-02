//! Input protocol: picking membership, the Elm-style click message
//! widgets carry, and the keyboard/IME dispatch hooks.
//!
//! `Message`/`OnClick<Msg>` originated in `matcha-ecs-widgets::button`; moved
//! into core because click dispatch (`ui_ecs.rs`'s `device_event`) must read
//! `OnClick<Msg>` off *any* clickable entity without knowing about `Button`
//! specifically — a protocol multiple widgets share, so it belongs in core per
//! `ECS_IMPLEMENTATION_PLAN.md` §3.1's crate-direction test. The widgets crate
//! re-exports both for source compatibility.
//!
//! # Why the event types are `matcha-window`'s
//!
//! [`Cursor`] holds a `matcha_window::window::CursorIcon`, and
//! [`KeyDispatch`]/[`ImeDispatch`] name `matcha_window`'s `KeyInput`/`ImeEvent`
//! in their signatures. That is deliberate, not an oversight to be tidied away.
//! `matcha-window` is *already* the abstraction over winit and baseview, so a
//! matcha-ecs-side mirror of those enums would be a second abstraction over the
//! first, kept in sync by hand, buying nothing but the ability to say the core
//! does not name a windowing crate.
//!
//! The price, which is real and should be known rather than rediscovered: any
//! widget handling raw keys or IME depends on `matcha-window` too, which is why
//! `matcha-ecs-widgets` carries that dependency for `TextBox` alone. Revisit
//! only if a second windowing backend ever wants an event vocabulary
//! `matcha-window` cannot express.

use bevy_ecs::{component::Component, world::EntityWorldMut};
use matcha_window::event::device_event::{ImeEvent, KeyInput};
use matcha_window::window::CursorIcon;

/// Marker: this entity is **opaque to picking**.
///
/// Note this declares occlusion, not interest in events. A pick that lands on
/// a `Pickable` entity stops there and is then resolved by walking *up* the
/// tree ([`crate::input::bubble_to_click_target`]); it never falls through to
/// something painted behind. An entity that should let clicks pass to whatever
/// is underneath simply omits this component — already the default for every
/// container.
///
/// The meaning carries across every picking backend: for [`crate::pick::RectPicker`]
/// it means "put my rect in the array", and for a future GPU ID-buffer backend
/// it would mean "write my id into the buffer".
#[derive(Component, Clone, Copy)]
pub struct Pickable;

/// CSS `:hover` — the pointer is inside this entity's box.
///
/// Present on the whole chain from the picked entity up to the root, exactly
/// as in CSS: hovering a button's label hovers the button, the row holding it,
/// and so on. There is therefore no `:hover-within` counterpart to
/// [`FocusWithin`](crate::components::focus::FocusWithin) — this marker
/// already is it.
///
/// Derived state, written only by [`crate::pointer::sync_pointer_components`].
/// Prefer `Has<Hovered>` or the [`PointerState`](crate::pointer::PointerState)
/// resource over `Changed<Hovered>`: change detection does not fire on removal,
/// so a `Changed` query would never see an entity *losing* hover.
#[derive(Component, Clone, Copy)]
pub struct Hovered;

/// CSS `:active` — a pointer button is held down and the press landed inside
/// this entity.
///
/// Cleared while the pointer is dragged off the pressed entity and restored
/// when it comes back, so a button does not stay looking pressed under a
/// cursor that has wandered away. Derived state, same caveats as [`Hovered`].
#[derive(Component, Clone, Copy)]
pub struct Active;

/// What the pointer should look like over this entity (CSS `cursor`).
///
/// Resolved **leaf to root** along the hover chain, so the innermost entity
/// that has an opinion wins and an ancestor's is a fallback — which is what
/// makes a plain `.cursor(Text)` on a text box survive whatever container it
/// is dropped into. An entity without this component simply has no opinion.
///
/// The chain is the one [`PointerState`](crate::pointer::PointerState) already
/// resolved; see [`crate::pointer::sync_cursor`].
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cursor(pub CursorIcon);

/// Marker bound for Elm-style messages: a cheap-to-clone, comparable value.
///
/// Deliberately `Clone` rather than `Copy`: a text widget's change notification
/// has to carry the new text (`Msg::TextChanged(String)`), which a `Copy` bound
/// makes unrepresentable. Nothing here ever needed `Copy` — dispatch clones the
/// message out of the component exactly once per event.
pub trait Message: Clone + PartialEq + Send + Sync + 'static {}
impl<T: Clone + PartialEq + Send + Sync + 'static> Message for T {}

/// The message a click emits, if any. Always present (as `Option<Msg>`) on a
/// clickable widget's bundle so its archetype is stable regardless of whether
/// a message was assigned (e.g. `Button::new(..)` without `.on(..)`).
#[derive(Component, Clone, PartialEq, Debug)]
pub struct OnClick<Msg: Message>(pub Option<Msg>);

/// Handles a key event on this entity, returning whether it was consumed.
///
/// Same fn-pointer dispatch idiom as [`LayoutDispatch`](crate::layout::LayoutDispatch):
/// a widget bakes `KeyDispatch::new(..)` into its `bundle()` and the core calls
/// it without knowing the widget type. There is no registry.
///
/// Delivery walks the focus path **root to leaf** (see
/// [`crate::keyboard::dispatch_key`]), so an ancestor is offered every event
/// before its descendants and can swallow it by returning `true`.
#[derive(Component, Clone, Copy)]
pub struct KeyDispatch {
    handle: fn(&mut EntityWorldMut, &KeyInput) -> bool,
}

impl KeyDispatch {
    pub fn new(handle: fn(&mut EntityWorldMut, &KeyInput) -> bool) -> Self {
        Self { handle }
    }

    pub(crate) fn call(&self, entity: &mut EntityWorldMut, input: &KeyInput) -> bool {
        (self.handle)(entity, input)
    }
}

/// Handles an IME composition event on this entity, returning whether it was
/// consumed. Delivery and dispatch work exactly like [`KeyDispatch`].
///
/// Carrying this component is also what tells the core that the OS IME should
/// be enabled while this entity holds focus (see `crate::keyboard::sync_ime_state`)
/// — the core never inspects the text itself.
#[derive(Component, Clone, Copy)]
pub struct ImeDispatch {
    handle: fn(&mut EntityWorldMut, &ImeEvent) -> bool,
}

impl ImeDispatch {
    pub fn new(handle: fn(&mut EntityWorldMut, &ImeEvent) -> bool) -> Self {
        Self { handle }
    }

    pub(crate) fn call(&self, entity: &mut EntityWorldMut, event: &ImeEvent) -> bool {
        (self.handle)(entity, event)
    }
}

/// Where the caret is, in **window space** (`[min_x, min_y, max_x, max_y]`), so
/// the platform can place its IME candidate list beside the text.
///
/// Written by whichever widget is editing text, read by the core and pushed to
/// the OS window. The core treats it as an opaque rectangle.
#[derive(Component, Clone, Copy, PartialEq, Debug, Default)]
pub struct ImeCursorArea(pub [f32; 4]);

/// What a pointer is doing, for [`PointerDispatch`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerPhase {
    /// Button went down. `count` is 1 for a single click, 2 for a double, ...
    Press { count: u32 },
    /// Button held and moved since the press.
    Drag,
    /// Wheel or trackpad scroll. `delta` is in **pixels** — `matcha-window`
    /// has already normalised a line-based delta by its lines-to-pixels
    /// factor, so the two are indistinguishable here.
    ///
    /// The sign is winit's: a positive `y` means scrolling *up*, i.e. the
    /// content should move down, so a scroll container subtracts the delta
    /// from its offset.
    ///
    /// A handler that cannot move any further should return `false` so the
    /// event bubbles to the next scrollable ancestor — that is what produces
    /// CSS-style scroll chaining.
    Scroll { delta: [f32; 2] },
}

/// A pointer event in the receiving entity's own coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerInput {
    /// Position relative to the entity's top-left corner.
    pub local_pos: [f32; 2],
    pub phase: PointerPhase,
}

/// Handles a pointer event that landed on this entity (or a descendant),
/// returning whether it was consumed.
///
/// This is the hook for widgets that need to know *where* inside themselves a
/// click landed rather than merely that one happened — placing a text caret,
/// dragging out a selection. `OnClick<Msg>` stays the right tool for a widget
/// that only cares that it was clicked.
///
/// Unlike keyboard delivery this bubbles **leaf to root**, matching
/// [`bubble_to_click_target`](crate::input::bubble_to_click_target): the event
/// has a position, so the innermost entity containing it is the natural first
/// responder.
///
/// Consuming a [`PointerPhase::Press`] also captures the pointer: every
/// [`PointerPhase::Drag`] until the button is released is delivered from this
/// entity, not from whatever the cursor is over by then (see
/// [`PointerCapture`](crate::input::PointerCapture)). A handler therefore never
/// has to check whether a drag it is offered actually belongs to it.
#[derive(Component, Clone, Copy)]
pub struct PointerDispatch {
    handle: fn(&mut EntityWorldMut, &PointerInput) -> bool,
}

impl PointerDispatch {
    pub fn new(handle: fn(&mut EntityWorldMut, &PointerInput) -> bool) -> Self {
        Self { handle }
    }

    pub(crate) fn call(&self, entity: &mut EntityWorldMut, input: &PointerInput) -> bool {
        (self.handle)(entity, input)
    }
}
