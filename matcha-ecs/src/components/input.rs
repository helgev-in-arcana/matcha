//! Input protocol: picking membership/order and the Elm-style click message
//! widgets carry.
//!
//! `Message`/`OnClick<Msg>` originated in `matcha-ecs-widgets::button`; moved
//! into core because click dispatch (`ui_ecs.rs`'s `device_event`) must read
//! `OnClick<Msg>` off *any* clickable entity without knowing about `Button`
//! specifically — a protocol multiple widgets share, so it belongs in core per
//! `ECS_IMPLEMENTATION_PLAN.md` §3.1's crate-direction test. The widgets crate
//! re-exports both for source compatibility.

use bevy_ecs::component::Component;

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
