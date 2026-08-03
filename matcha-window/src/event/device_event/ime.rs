//! IME (input method editor) events.
//!
//! Deliberately a backend-neutral type carrying only owned `String`s and byte
//! offsets: no winit type and no text-layout-library type appears in it. The
//! UI layer routes these to whichever widget has focus without knowing what
//! that widget uses to shape or edit text, so a text engine can be swapped out
//! without touching the delivery path.
//!
//! IME is off by default at the OS level. A window must call
//! [`Window::set_ime_allowed`](crate::window::Window::set_ime_allowed) before
//! any of these events are produced, and should report where the caret is with
//! [`set_ime_cursor_area`](crate::window::Window::set_ime_cursor_area) so the
//! platform can place its candidate list sensibly.

/// One step of an IME composition session.
///
/// A session runs `Enabled` → (`Preedit`)* → `Commit`? → `Disabled`. Composing
/// Japanese, for example, produces a run of `Preedit` events as the user types
/// and converts, then one `Commit` carrying the accepted text when they press
/// Enter. An abandoned composition ends with an empty `Preedit` and no
/// `Commit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeEvent {
    /// The IME is now active for this window and may start composing.
    Enabled,

    /// The in-progress, unconfirmed text. Replaces any previous preedit.
    ///
    /// `cursor` is a byte range **within `text`** marking the selected or
    /// highlighted region of the composition, or `None` when the IME asks for
    /// the caret to be hidden. An empty `text` ends the composition without
    /// committing anything.
    Preedit {
        text: String,
        cursor: Option<(usize, usize)>,
    },

    /// The composition was accepted. This text should be inserted, replacing
    /// the preedit.
    Commit { text: String },

    /// The IME is no longer active. Any preedit still showing must be dropped.
    Disabled,
}
