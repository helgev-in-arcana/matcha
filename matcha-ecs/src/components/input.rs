//! Input protocol: hit-testing membership/order and the Elm-style click
//! message widgets carry.
//!
//! `Message`/`OnClick<Msg>` originated in `matcha-ecs-widgets::button`; moved
//! into core because hit-test dispatch (`ui_ecs.rs`'s `device_event`) must
//! read `OnClick<Msg>` off *any* clickable entity without knowing about
//! `Button` specifically — a protocol multiple widgets share, so it belongs
//! in core per `ECS_IMPLEMENTATION_PLAN.md` §3.1's crate-direction test. The
//! widgets crate re-exports both for source compatibility.

use bevy_ecs::component::Component;

/// Marker: this entity participates in hit-testing. [`crate::input::update_hit_test_cache`]
/// only considers entities that carry it (plus `LayoutOutput`/`GlobalTransform`).
#[derive(Component, Clone, Copy)]
pub struct HitTestEnabled;

/// Hit-test stacking order: higher wins on overlap. Ties fall back to paint
/// order (later-painted wins). Entities without this component default to `0`.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ZOrder(pub i32);

/// Marker bound for Elm-style messages: a cheap, copyable, comparable value.
pub trait Message: Copy + PartialEq + Send + Sync + 'static {}
impl<T: Copy + PartialEq + Send + Sync + 'static> Message for T {}

/// The message a click emits, if any. Always present (as `Option<Msg>`) on a
/// clickable widget's bundle so its archetype is stable regardless of whether
/// a message was assigned (e.g. `Button::new(..)` without `.on(..)`).
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct OnClick<Msg: Message>(pub Option<Msg>);
