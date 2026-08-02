//! Keyboard events, in the W3C UI Events vocabulary.
//!
//! The key types are re-exported from [`keyboard_types`] rather than defined
//! here or taken from a backend. That crate is nothing but the [UI Events
//! KeyboardEvent][spec] enums — its only dependency is `bitflags` — and both
//! backends already speak it: it is baseview's native vocabulary, and winit's
//! own enums are the same specification under two renamed variants (see
//! `winit_interface::keyboard`). Re-exporting it is therefore not a leak of the
//! kind `winit::keyboard` was: nothing here is tied to a windowing library, and
//! a third backend would map onto the same spec rather than onto winit's
//! reading of it.
//!
//! [spec]: https://w3c.github.io/uievents-key/

use super::{ElementState, KeyboardState};

pub use keyboard_types::{Code, Key, Location, Modifiers, NamedKey};

/// A keyboard event.
///
/// This struct contains the decoded key event data and a snapshot of the entire
/// keyboard state at the moment the event occurred.
///
/// When used as state-machine **input** (from `winit_interface`), `snapshot`
/// should be left as `KeyboardState::default()` — the state machine fills it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInput {
    /// Which physical button was pressed, regardless of layout.
    /// [`Code::Unidentified`] when the platform did not say.
    pub physical_key: Code,
    /// What the keystroke means under the current layout.
    pub logical_key: Key,
    /// The text produced by this key event, if any.
    pub text: Option<String>,
    pub location: Location,
    /// `Pressed(0)` / `Released(0)` when used as input; populated by state machine on output.
    pub state: ElementState,
    pub repeat: bool,
    /// Snapshot of all currently-held keys. Filled in by `KeyboardState::keyboard_input`.
    pub snapshot: KeyboardState,
}

// --- Methods for the key that triggered the event ---

impl KeyInput {
    /// Returns `true` if the logical key that triggered this event is `CapsLock`.
    pub fn caps_lock(&self) -> bool {
        if let Key::Named(named_key) = self.logical_key() {
            *named_key == NamedKey::CapsLock
        } else {
            false
        }
    }

    // todo: Implement a rest of keys
}

/// Raw information about the key that triggered this event.
impl KeyInput {
    pub fn physical_key(&self) -> Code {
        self.physical_key
    }

    pub fn logical_key(&self) -> &Key {
        &self.logical_key
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    pub fn location(&self) -> Location {
        self.location
    }

    pub fn state(&self) -> ElementState {
        self.state
    }

    pub fn is_repeat(&self) -> bool {
        self.repeat
    }
}

// --- Methods for the keyboard state at the moment of the event ---

/// Methods to query the state of modifier keys at the moment the event occurred.
impl KeyInput {
    /// Returns `true` if the `Control` key was held down when the event occurred.
    pub fn ctrl_held(&self) -> bool {
        self.modifiers().ctrl()
    }

    /// Returns `true` if the `Shift` key was held down when the event occurred.
    pub fn shift_held(&self) -> bool {
        self.modifiers().shift()
    }

    /// Returns `true` if the `Alt` key was held down when the event occurred.
    pub fn alt_held(&self) -> bool {
        self.modifiers().alt()
    }

    /// Returns `true` if the `Super` key (e.g., Windows or Command) was held
    /// down when the event occurred. The spec calls this modifier `Meta`.
    pub fn super_held(&self) -> bool {
        self.modifiers().meta()
    }
}

/// General information about the keyboard state at the moment the event occurred.
impl KeyInput {
    /// Returns `true` if the given physical key was held down when the event occurred.
    pub fn is_physical_pressed(&self, key: Code) -> bool {
        self.snapshot.is_physical_pressed(&key)
    }

    /// Returns `true` if the given logical key was held down when the event occurred.
    pub fn is_logical_pressed(&self, key: Key) -> bool {
        self.snapshot.is_logical_pressed(&key)
    }

    /// Returns the state of the modifier keys at the moment the event occurred.
    pub fn modifiers(&self) -> Modifiers {
        self.snapshot.modifiers()
    }

    /// Returns a list of all keys currently held down, in the order they were pressed.
    pub fn press_order(&self) -> Vec<(Code, Key)> {
        self.snapshot.press_order()
    }
}
