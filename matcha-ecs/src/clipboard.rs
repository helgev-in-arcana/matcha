//! The system clipboard, as a world resource.
//!
//! # Why this is core's, not a widget's
//!
//! The clipboard is a platform capability, and the core already brokers the
//! other two: [`pointer::sync_cursor`](crate::pointer::sync_cursor) pushes the
//! cursor shape to the window, and
//! [`keyboard::sync_ime_state`](crate::keyboard::sync_ime_state) pushes IME
//! state. Leaving the clipboard to whichever widget happened to need it first
//! made that widget the owner of a process-wide resource, and any second
//! consumer — a copyable label, a canvas, a list view — would have had to
//! either depend on that widget or open a second handle.
//!
//! Nothing here is text-specific, and nothing dispatches: this is an ordinary
//! object a widget reaches for while handling its own shortcut. Copy and paste
//! look like keyboard events but are not routed like them — a paste can arrive
//! from a menu item or a middle-click with no key event at all — so there is
//! deliberately no clipboard counterpart to
//! [`KeyDispatch`](crate::components::input::KeyDispatch).
//!
//! # Lifetime
//!
//! Inserted lazily by [`clipboard`], so an app that never copies anything never
//! opens a handle — the same pattern the widget-side `FontCtx`/`ImageCtx`
//! resources follow. The handle is held open rather than opened per operation
//! because on Windows and X11 opening the clipboard is a real syscall that can
//! fail while another process holds it.

use bevy_ecs::{resource::Resource, world::World};
use std::sync::Arc;

pub use matcha_window::clipboard::Clipboard;

/// The process's clipboard handle.
///
/// Cloning shares the handle; there is only ever one per process.
#[derive(Resource, Clone, Default)]
pub struct ClipboardResource(pub Arc<Clipboard>);

/// The clipboard, opening it if this is the first use.
pub fn clipboard(world: &mut World) -> Arc<Clipboard> {
    world
        .get_resource_or_insert_with(ClipboardResource::default)
        .0
        .clone()
}
