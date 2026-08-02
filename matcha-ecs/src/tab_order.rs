//! Tab order: moving focus with the keyboard.
//!
//! # Why this is small
//!
//! Sequential focus navigation is "the next focusable thing in document
//! order", and since [`crate::traversal`] made one order serve painting and
//! picking, document order is something this crate already has. So tab order
//! is that walk, filtered by [`FocusPolicy`], plus an index step — no second
//! ordering, no per-container registration, and nothing to keep in sync.
//!
//! It also inherits the walk's two skips for free: a [`Hidden`](crate::components::layout::Hidden)
//! subtree is not in the list (matching `display: none`), and a
//! [`FocusPolicy::Claim`] entity is a leaf here, so a text box's decorative
//! children never become separate tab stops.
//!
//! # What it is not
//!
//! There is no `tabindex`. CSS/HTML's positive `tabindex` creates a second
//! ordering that runs *before* document order, which is exactly the "two
//! orderings that can disagree" shape [`crate::traversal`]'s docs argue
//! against, and it is widely considered an accessibility mistake in HTML too.
//! Opting *out* is expressible — remove `FocusPolicy` and the entity stops
//! being a stop — which covers the useful half.
//!
//! Wrapping at the ends is deliberate and not configurable: focus escaping into
//! the browser chrome is a behaviour of documents, not of an application
//! window.

use bevy_ecs::{entity::Entity, world::World};
use matcha_window::event::device_event::{Key, KeyInput, NamedKey};

use crate::{
    components::focus::FocusPolicy,
    focus::{request_focus, Focus},
    resources::ui_root,
    traversal,
};

/// Every focusable entity under `root`, in document order.
///
/// A [`FocusPolicy::Claim`] entity is included but its subtree is not: claiming
/// means "focus stops at me", and that has to hold for the keyboard exactly as
/// it does for a click.
pub fn focusable_in_order(world: &World, root: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    traversal::walk(world, root, (), &mut |world, entity, _| {
        match world.get::<FocusPolicy>(entity) {
            Some(FocusPolicy::Claim) => {
                out.push(entity);
                // Pruning the subtree: returning `None` stops the walk here.
                None
            }
            Some(_) => {
                out.push(entity);
                Some(())
            }
            None => Some(()),
        }
    });
    out
}

/// Which way `Tab` is moving focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabDirection {
    Forward,
    Backward,
}

impl TabDirection {
    /// Read a key event as a tab step, if it is one. `Shift+Tab` goes back,
    /// the convention on every platform.
    pub fn from_key(input: &KeyInput) -> Option<Self> {
        if input.logical_key != Key::Named(NamedKey::Tab) {
            return None;
        }
        Some(if input.modifiers().shift() {
            TabDirection::Backward
        } else {
            TabDirection::Forward
        })
    }
}

/// The entity `direction` moves focus to, or `None` if nothing is focusable.
///
/// Testable core: no resources beyond [`Focus`], no window. With nothing
/// focused, forward starts at the first stop and backward at the last, which is
/// what makes the very first `Tab` press do something sensible.
pub fn next_focusable(world: &World, root: Entity, direction: TabDirection) -> Option<Entity> {
    let stops = focusable_in_order(world, root);
    if stops.is_empty() {
        return None;
    }

    let current = world
        .get_resource::<Focus>()
        .and_then(|f| f.top())
        .and_then(|top| stops.iter().position(|&e| e == top));

    let index = match (current, direction) {
        (Some(i), TabDirection::Forward) => (i + 1) % stops.len(),
        (Some(i), TabDirection::Backward) => (i + stops.len() - 1) % stops.len(),
        (None, TabDirection::Forward) => 0,
        (None, TabDirection::Backward) => stops.len() - 1,
    };
    Some(stops[index])
}

/// Move focus one tab stop. Returns whether it moved.
pub fn move_focus(world: &mut World, direction: TabDirection) -> bool {
    let Some(root) = ui_root(world) else {
        return false;
    };
    let Some(next) = next_focusable(world, root, direction) else {
        return false;
    };
    request_focus(world, next)
}

/// Handle `input` as a tab step if it is one, before it reaches the focused
/// widget.
///
/// Called ahead of [`crate::keyboard::dispatch_key`], so a widget can still
/// claim `Tab` for itself only by... not being able to. That is intentional
/// for now: a widget that inserts a literal tab character (a code editor) is
/// the one case that would need the reverse order, and it does not exist yet.
/// The note is here so the tradeoff is visible when it does.
pub fn handle_tab_key(world: &mut World, input: &KeyInput) -> bool {
    let Some(direction) = TabDirection::from_key(input) else {
        return false;
    };
    move_focus(world, direction);
    // Consumed either way: a `Tab` that found nowhere to go must not fall
    // through and be typed into whatever holds focus.
    true
}
