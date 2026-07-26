//! Input protocol: picking membership/order, the Elm-style click message
//! widgets carry, and the keyboard/IME dispatch hooks.
//!
//! `Message`/`OnClick<Msg>` originated in `matcha-ecs-widgets::button`; moved
//! into core because click dispatch (`ui_ecs.rs`'s `device_event`) must read
//! `OnClick<Msg>` off *any* clickable entity without knowing about `Button`
//! specifically — a protocol multiple widgets share, so it belongs in core per
//! `ECS_IMPLEMENTATION_PLAN.md` §3.1's crate-direction test. The widgets crate
//! re-exports both for source compatibility.

use bevy_ecs::{component::Component, world::EntityWorldMut};
use matcha_window::event::device_event::{ImeEvent, KeyInput};

/// Marker: this entity is **opaque to picking**.
///
/// Note this declares occlusion, not interest in events. A pick that lands on
/// a `Pickable` entity stops there and is then resolved by walking *up* the
/// tree ([`crate::input::bubble_to_click_target`]); it never falls through to
/// something painted behind. An entity that should let clicks pass to whatever
/// is underneath simply omits this component — already the default for every
/// container.
///
/// The meaning carries across every picking backend: for [`crate::pick::RectZPicker`]
/// it means "put my rect in the array", and for a future GPU ID-buffer backend
/// it would mean "write my id into the buffer".
#[derive(Component, Clone, Copy)]
pub struct Pickable;

/// Stacking order for picking: higher wins on overlap. Ties fall back to paint
/// order (later-painted wins). Entities without this component default to `0`.
///
/// **Backend-specific**: this is a hint for [`crate::pick::RectZPicker`] only.
/// A BVH or ID-buffer backend derives order from actual depth and ignores it.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ZOrder(pub i32);

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
