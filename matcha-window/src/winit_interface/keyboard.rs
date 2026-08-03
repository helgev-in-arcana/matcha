//! winit's keyboard vocabulary, mapped onto the W3C one this crate speaks.
//!
//! Both sides are encodings of the same specification — [UI Events
//! KeyboardEvent][spec] — so 305 of winit's 306 `NamedKey` variants and 191 of
//! its 194 `KeyCode` variants share a name with their `keyboard_types`
//! counterpart. That correspondence is what this module exploits.
//!
//! # Read this before changing anything here
//!
//! **Check this file when a keyboard bug is suspected** (a key that does
//! nothing, arrives as the wrong key, or arrives as `Unidentified`), and
//! **check it when profiling suggests input handling is hot**. It is the only
//! place in the crate where a key's identity is decided, and the only place
//! where a key is turned into a string and back.
//!
//! ## How the mapping is split, and why
//!
//! Three tiers, in match order:
//!
//! 1. **The exceptions** — the four places the two vocabularies genuinely
//!    disagree. winit documents all of them:
//!    - `KeyCode::SuperLeft`/`SuperRight` are the spec's `MetaLeft`/`MetaRight`.
//!    - `KeyCode::Meta` is the spec's `Super` (a legacy key, deprecated in
//!      `keyboard_types`, hence the `allow` below).
//!    - `NamedKey::Space` has no spec counterpart: the spec reports the space
//!      bar as the character `" "`, which is what this maps it to. A widget
//!      therefore handles space in its `Key::Character` arm, alongside every
//!      other text-producing key, instead of special-casing it.
//! 2. **The head** — every key this framework plausibly branches on: letters,
//!    digits, the numpad, punctuation, modifiers, arrows, editing keys, F1–F12
//!    and the IME keys. Written out by hand as `X => X`.
//! 3. **The tail** — everything else (TV remotes, media transport, launcher
//!    keys, Soft1–4, F13–F35, ...). Resolved generically: the variant's name is
//!    written into a stack buffer via its derived `Debug`, then parsed by
//!    `keyboard_types`' `FromStr`. Anything neither side can name degrades to
//!    `Unidentified`.
//!
//! ## The trade-off this encodes
//!
//! Both winit enums are `#[non_exhaustive]`, so **a match here can never be
//! exhaustive and the compiler can never report a key winit has added.** That
//! is structural: silent gaps on addition are unavoidable in a hand-written
//! table, and keeping ~500 mechanical `X => X` arms in sync by hand was the
//! cost being paid for nothing in the tail.
//!
//! So the tiers buy different things:
//!
//! - The head keeps the arms **explicit**, which means a winit release that
//!   renames or removes one of them is a *compile error*. Variant names are
//!   covered by semver, so that can only happen on a deliberate major upgrade.
//!   The head is also the fast path: it is a jump table, and in practice it is
//!   what actually gets pressed, so the tail's string round-trip is rarely
//!   executed at all.
//! - The tail gives up that compile-time signal — a rename there degrades
//!   silently to `Unidentified` — in exchange for **keys winit adds working
//!   automatically**, and for not carrying a table nobody audits. The tests at
//!   the bottom pin a sample of tail keys so a rename surfaces in CI rather
//!   than in a user's hands.
//!
//! The tail's cost is a `write_str` of at most 25 bytes into a stack buffer
//! (**no heap allocation**) plus a string match, on the order of 100ns against
//! the head's ~2ns. Key events are bounded by human fingers and autorepeat, so
//! even at 100 events/s that is ~0.001% of a frame — and the same code path
//! already allocates a `String` per keystroke, because `keyboard_types::Key`
//! stores its character text as one. If input handling ever does show up in a
//! profile, this string round-trip is not where the time is; look at what
//! happens downstream of the event instead.
//!
//! ## The `Debug` dependency, stated plainly
//!
//! The tail relies on the derived `Debug` of a fieldless enum printing the
//! variant name. `std` does not guarantee the output format of derived `Debug`.
//! The practical risk is very low (both enums are plain unit-variant enums, and
//! the derive has always printed the variant name), and the failure mode is
//! loud — every tail key would become `Unidentified` at once, and the tests
//! below would fail. Using winit's `serde` feature instead would replace this
//! with a slightly firmer contract, at the price of another dependency; it was
//! considered and judged not worth it.
//!
//! [spec]: https://w3c.github.io/uievents-key/

#![allow(deprecated)] // `Code::Super` / `NamedKey::Hyper` are legacy in the spec.

use core::fmt::{self, Write as _};
use core::str::FromStr;

use keyboard_types::{Code, Key, Location, Modifiers, NamedKey};
use winit::keyboard as w;

/// The logical key: what the keystroke *means* under the current layout.
pub(super) fn map_key(key: w::Key) -> Key {
    match key {
        w::Key::Character(s) => Key::Character(s.to_string()),
        w::Key::Named(named) => map_named_key(named),
        // winit reports a dead key's own text separately; the spec has a single
        // `Dead` value and puts the text in the composition events instead.
        w::Key::Dead(_) => Key::Named(NamedKey::Dead),
        w::Key::Unidentified(_) => Key::Named(NamedKey::Unidentified),
    }
}

fn map_named_key(named: w::NamedKey) -> Key {
    use w::NamedKey as N;

    let mapped = match named {
        // -- The one exception, and the only arm that is not `X => X`. -------
        // The spec has no `Space` named key: the space bar is a character.
        N::Space => return Key::Character(" ".to_string()),

        // -- Head: keys a widget plausibly branches on. ----------------------
        N::Enter => NamedKey::Enter,
        N::Tab => NamedKey::Tab,
        N::Backspace => NamedKey::Backspace,
        N::Delete => NamedKey::Delete,
        N::Insert => NamedKey::Insert,
        N::Escape => NamedKey::Escape,
        N::ArrowDown => NamedKey::ArrowDown,
        N::ArrowLeft => NamedKey::ArrowLeft,
        N::ArrowRight => NamedKey::ArrowRight,
        N::ArrowUp => NamedKey::ArrowUp,
        N::Home => NamedKey::Home,
        N::End => NamedKey::End,
        N::PageDown => NamedKey::PageDown,
        N::PageUp => NamedKey::PageUp,
        N::Clear => NamedKey::Clear,
        N::Cancel => NamedKey::Cancel,
        N::ContextMenu => NamedKey::ContextMenu,
        N::Help => NamedKey::Help,
        // Modifiers.
        N::Alt => NamedKey::Alt,
        N::AltGraph => NamedKey::AltGraph,
        N::Control => NamedKey::Control,
        N::Shift => NamedKey::Shift,
        N::Meta => NamedKey::Meta,
        N::Super => NamedKey::Super,
        N::Hyper => NamedKey::Hyper,
        N::Symbol => NamedKey::Symbol,
        N::SymbolLock => NamedKey::SymbolLock,
        N::CapsLock => NamedKey::CapsLock,
        N::NumLock => NamedKey::NumLock,
        N::ScrollLock => NamedKey::ScrollLock,
        N::Fn => NamedKey::Fn,
        N::FnLock => NamedKey::FnLock,
        // Editing commands that arrive as keys on some keyboards.
        N::Copy => NamedKey::Copy,
        N::Cut => NamedKey::Cut,
        N::Paste => NamedKey::Paste,
        N::Undo => NamedKey::Undo,
        N::Redo => NamedKey::Redo,
        N::Select => NamedKey::Select,
        // Composition / IME.
        N::Compose => NamedKey::Compose,
        N::Convert => NamedKey::Convert,
        N::NonConvert => NamedKey::NonConvert,
        N::Accept => NamedKey::Accept,
        N::Process => NamedKey::Process,
        N::ModeChange => NamedKey::ModeChange,
        N::KanaMode => NamedKey::KanaMode,
        N::KanjiMode => NamedKey::KanjiMode,
        N::Hiragana => NamedKey::Hiragana,
        N::Katakana => NamedKey::Katakana,
        N::HiraganaKatakana => NamedKey::HiraganaKatakana,
        N::Romaji => NamedKey::Romaji,
        N::Eisu => NamedKey::Eisu,
        N::Hankaku => NamedKey::Hankaku,
        N::Zenkaku => NamedKey::Zenkaku,
        N::ZenkakuHankaku => NamedKey::ZenkakuHankaku,
        N::Alphanumeric => NamedKey::Alphanumeric,
        N::HangulMode => NamedKey::HangulMode,
        N::HanjaMode => NamedKey::HanjaMode,
        // Function keys within reach of a normal keyboard.
        N::F1 => NamedKey::F1,
        N::F2 => NamedKey::F2,
        N::F3 => NamedKey::F3,
        N::F4 => NamedKey::F4,
        N::F5 => NamedKey::F5,
        N::F6 => NamedKey::F6,
        N::F7 => NamedKey::F7,
        N::F8 => NamedKey::F8,
        N::F9 => NamedKey::F9,
        N::F10 => NamedKey::F10,
        N::F11 => NamedKey::F11,
        N::F12 => NamedKey::F12,

        // -- Tail: resolved by name. See the module docs. --------------------
        other => return Key::Named(by_name(&other).unwrap_or(NamedKey::Unidentified)),
    };
    Key::Named(mapped)
}

/// The physical key: which button was pressed, regardless of layout.
pub(super) fn map_code(key: w::PhysicalKey) -> Code {
    use w::KeyCode as C;

    let code = match key {
        w::PhysicalKey::Code(code) => code,
        w::PhysicalKey::Unidentified(_) => return Code::Unidentified,
    };
    match code {
        // -- The three exceptions, the only arms that are not `X => X`. ------
        C::SuperLeft => Code::MetaLeft,
        C::SuperRight => Code::MetaRight,
        C::Meta => Code::Super,

        // -- Head: keys a widget plausibly branches on. ----------------------
        C::KeyA => Code::KeyA,
        C::KeyB => Code::KeyB,
        C::KeyC => Code::KeyC,
        C::KeyD => Code::KeyD,
        C::KeyE => Code::KeyE,
        C::KeyF => Code::KeyF,
        C::KeyG => Code::KeyG,
        C::KeyH => Code::KeyH,
        C::KeyI => Code::KeyI,
        C::KeyJ => Code::KeyJ,
        C::KeyK => Code::KeyK,
        C::KeyL => Code::KeyL,
        C::KeyM => Code::KeyM,
        C::KeyN => Code::KeyN,
        C::KeyO => Code::KeyO,
        C::KeyP => Code::KeyP,
        C::KeyQ => Code::KeyQ,
        C::KeyR => Code::KeyR,
        C::KeyS => Code::KeyS,
        C::KeyT => Code::KeyT,
        C::KeyU => Code::KeyU,
        C::KeyV => Code::KeyV,
        C::KeyW => Code::KeyW,
        C::KeyX => Code::KeyX,
        C::KeyY => Code::KeyY,
        C::KeyZ => Code::KeyZ,
        C::Digit0 => Code::Digit0,
        C::Digit1 => Code::Digit1,
        C::Digit2 => Code::Digit2,
        C::Digit3 => Code::Digit3,
        C::Digit4 => Code::Digit4,
        C::Digit5 => Code::Digit5,
        C::Digit6 => Code::Digit6,
        C::Digit7 => Code::Digit7,
        C::Digit8 => Code::Digit8,
        C::Digit9 => Code::Digit9,
        // Punctuation, including the keys only JIS/ISO boards have.
        C::Backquote => Code::Backquote,
        C::Backslash => Code::Backslash,
        C::BracketLeft => Code::BracketLeft,
        C::BracketRight => Code::BracketRight,
        C::Comma => Code::Comma,
        C::Equal => Code::Equal,
        C::Minus => Code::Minus,
        C::Period => Code::Period,
        C::Quote => Code::Quote,
        C::Semicolon => Code::Semicolon,
        C::Slash => Code::Slash,
        C::IntlBackslash => Code::IntlBackslash,
        C::IntlRo => Code::IntlRo,
        C::IntlYen => Code::IntlYen,
        // Whitespace and editing.
        C::Space => Code::Space,
        C::Tab => Code::Tab,
        C::Enter => Code::Enter,
        C::Backspace => Code::Backspace,
        C::Delete => Code::Delete,
        C::Insert => Code::Insert,
        C::Escape => Code::Escape,
        C::Home => Code::Home,
        C::End => Code::End,
        C::PageDown => Code::PageDown,
        C::PageUp => Code::PageUp,
        C::ArrowDown => Code::ArrowDown,
        C::ArrowLeft => Code::ArrowLeft,
        C::ArrowRight => Code::ArrowRight,
        C::ArrowUp => Code::ArrowUp,
        C::ContextMenu => Code::ContextMenu,
        C::Help => Code::Help,
        // Modifiers and locks.
        C::AltLeft => Code::AltLeft,
        C::AltRight => Code::AltRight,
        C::ControlLeft => Code::ControlLeft,
        C::ControlRight => Code::ControlRight,
        C::ShiftLeft => Code::ShiftLeft,
        C::ShiftRight => Code::ShiftRight,
        C::CapsLock => Code::CapsLock,
        C::NumLock => Code::NumLock,
        C::ScrollLock => Code::ScrollLock,
        C::Fn => Code::Fn,
        C::FnLock => Code::FnLock,
        // IME keys, as found on Japanese and Korean keyboards.
        C::Convert => Code::Convert,
        C::NonConvert => Code::NonConvert,
        C::KanaMode => Code::KanaMode,
        C::Lang1 => Code::Lang1,
        C::Lang2 => Code::Lang2,
        C::Lang3 => Code::Lang3,
        C::Lang4 => Code::Lang4,
        C::Lang5 => Code::Lang5,
        // The numpad.
        C::Numpad0 => Code::Numpad0,
        C::Numpad1 => Code::Numpad1,
        C::Numpad2 => Code::Numpad2,
        C::Numpad3 => Code::Numpad3,
        C::Numpad4 => Code::Numpad4,
        C::Numpad5 => Code::Numpad5,
        C::Numpad6 => Code::Numpad6,
        C::Numpad7 => Code::Numpad7,
        C::Numpad8 => Code::Numpad8,
        C::Numpad9 => Code::Numpad9,
        C::NumpadAdd => Code::NumpadAdd,
        C::NumpadSubtract => Code::NumpadSubtract,
        C::NumpadMultiply => Code::NumpadMultiply,
        C::NumpadDivide => Code::NumpadDivide,
        C::NumpadDecimal => Code::NumpadDecimal,
        C::NumpadComma => Code::NumpadComma,
        C::NumpadEnter => Code::NumpadEnter,
        C::NumpadEqual => Code::NumpadEqual,
        // Function keys within reach of a normal keyboard.
        C::F1 => Code::F1,
        C::F2 => Code::F2,
        C::F3 => Code::F3,
        C::F4 => Code::F4,
        C::F5 => Code::F5,
        C::F6 => Code::F6,
        C::F7 => Code::F7,
        C::F8 => Code::F8,
        C::F9 => Code::F9,
        C::F10 => Code::F10,
        C::F11 => Code::F11,
        C::F12 => Code::F12,
        C::PrintScreen => Code::PrintScreen,
        C::Pause => Code::Pause,

        // -- Tail: resolved by name. See the module docs. --------------------
        other => by_name(&other).unwrap_or(Code::Unidentified),
    }
}

/// Which of several same-named keys was pressed.
pub(super) fn map_location(location: w::KeyLocation) -> Location {
    match location {
        w::KeyLocation::Standard => Location::Standard,
        w::KeyLocation::Left => Location::Left,
        w::KeyLocation::Right => Location::Right,
        w::KeyLocation::Numpad => Location::Numpad,
    }
}

/// The modifiers held at the moment of the event.
///
/// winit tracks only these four; the spec's lock and symbol flags
/// (`CAPS_LOCK`, `NUM_LOCK`, `FN`, ...) have no winit counterpart and stay
/// clear. `SUPER` is the spec's `META` — see the module docs.
pub(super) fn map_modifiers(state: w::ModifiersState) -> Modifiers {
    let mut out = Modifiers::empty();
    out.set(Modifiers::SHIFT, state.shift_key());
    out.set(Modifiers::CONTROL, state.control_key());
    out.set(Modifiers::ALT, state.alt_key());
    out.set(Modifiers::META, state.super_key());
    out
}

/// Resolve a winit key by the name it and the spec share.
///
/// This is the tail of the mapping — see the module docs for what it buys and
/// what it costs. `None` means the two vocabularies do not agree on a name,
/// which every caller turns into `Unidentified`.
fn by_name<T, U>(key: &T) -> Option<U>
where
    T: fmt::Debug,
    U: FromStr,
{
    let mut name = NameBuf::default();
    // Fails only if the name outgrows the buffer, which no current variant
    // does; either way the caller degrades to `Unidentified`.
    write!(name, "{key:?}").ok()?;
    name.as_str().parse().ok()
}

/// A stack buffer just large enough for the longest variant name either enum
/// has (`TVAudioDescriptionMixDown`, 25 bytes), so resolving a key by name
/// costs no allocation.
#[derive(Default)]
struct NameBuf {
    bytes: [u8; 32],
    len: usize,
}

impl NameBuf {
    fn as_str(&self) -> &str {
        // Variant names are ASCII identifiers, and only `write_str` ever fills
        // this buffer — with whole `&str`s, so no char is ever split.
        core::str::from_utf8(&self.bytes[..self.len])
            .expect("a variant name is ASCII, so the buffer holds valid UTF-8")
    }
}

impl fmt::Write for NameBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        let room = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        room.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(code: w::KeyCode) -> Code {
        map_code(w::PhysicalKey::Code(code))
    }

    fn named(key: w::NamedKey) -> Key {
        map_key(w::Key::Named(key))
    }

    // -- The exceptions, which are the hand-written part of the mapping. -----

    #[test]
    fn the_space_bar_arrives_as_a_character_not_a_named_key() {
        // The spec has no `Space` named key: a widget must handle the space bar
        // in the same arm as every other text-producing key.
        assert_eq!(named(w::NamedKey::Space), Key::Character(" ".to_string()));
    }

    #[test]
    fn winits_super_is_the_specs_meta_and_vice_versa() {
        assert_eq!(code_of(w::KeyCode::SuperLeft), Code::MetaLeft);
        assert_eq!(code_of(w::KeyCode::SuperRight), Code::MetaRight);
        assert_eq!(code_of(w::KeyCode::Meta), Code::Super);
        // And the modifier flag follows the same swap.
        assert_eq!(map_modifiers(w::ModifiersState::SUPER), Modifiers::META);
    }

    // -- The head. These arms are written out, so a rename would already be a
    // -- compile error; a spot check is enough.

    #[test]
    fn head_keys_map_to_their_namesake() {
        assert_eq!(code_of(w::KeyCode::KeyA), Code::KeyA);
        assert_eq!(code_of(w::KeyCode::NumpadEnter), Code::NumpadEnter);
        assert_eq!(code_of(w::KeyCode::IntlYen), Code::IntlYen);
        assert_eq!(named(w::NamedKey::Enter), Key::Named(NamedKey::Enter));
        assert_eq!(named(w::NamedKey::ArrowUp), Key::Named(NamedKey::ArrowUp));
        assert_eq!(named(w::NamedKey::Convert), Key::Named(NamedKey::Convert));
    }

    // -- The tail. These arms do NOT exist in the source, so a winit rename
    // -- would not be a compile error — it would silently produce
    // -- `Unidentified`. The assertions below are the early-warning system for
    // -- that, and are deliberately written out one key at a time rather than
    // -- generated. See the module docs.

    #[test]
    fn tail_codes_still_resolve_by_name() {
        assert_eq!(code_of(w::KeyCode::F13), Code::F13);
        assert_eq!(code_of(w::KeyCode::F24), Code::F24);
        assert_eq!(code_of(w::KeyCode::AudioVolumeMute), Code::AudioVolumeMute);
        assert_eq!(code_of(w::KeyCode::AudioVolumeUp), Code::AudioVolumeUp);
        assert_eq!(code_of(w::KeyCode::MediaPlayPause), Code::MediaPlayPause);
        assert_eq!(code_of(w::KeyCode::MediaTrackNext), Code::MediaTrackNext);
        assert_eq!(code_of(w::KeyCode::MediaSelect), Code::MediaSelect);
        assert_eq!(code_of(w::KeyCode::BrowserBack), Code::BrowserBack);
        assert_eq!(code_of(w::KeyCode::BrowserRefresh), Code::BrowserRefresh);
        assert_eq!(code_of(w::KeyCode::LaunchMail), Code::LaunchMail);
        assert_eq!(code_of(w::KeyCode::LaunchApp1), Code::LaunchApp1);
        assert_eq!(code_of(w::KeyCode::Hiragana), Code::Hiragana);
        assert_eq!(code_of(w::KeyCode::Katakana), Code::Katakana);
        assert_eq!(code_of(w::KeyCode::Eject), Code::Eject);
        assert_eq!(code_of(w::KeyCode::Power), Code::Power);
        assert_eq!(code_of(w::KeyCode::Sleep), Code::Sleep);
        assert_eq!(code_of(w::KeyCode::WakeUp), Code::WakeUp);
        assert_eq!(code_of(w::KeyCode::Turbo), Code::Turbo);
        assert_eq!(code_of(w::KeyCode::Abort), Code::Abort);
        assert_eq!(code_of(w::KeyCode::Again), Code::Again);
        assert_eq!(code_of(w::KeyCode::Props), Code::Props);
        assert_eq!(code_of(w::KeyCode::Find), Code::Find);
        assert_eq!(code_of(w::KeyCode::Hyper), Code::Hyper);
        assert_eq!(code_of(w::KeyCode::NumpadHash), Code::NumpadHash);
        assert_eq!(
            code_of(w::KeyCode::NumpadMemoryRecall),
            Code::NumpadMemoryRecall
        );
    }

    #[test]
    fn tail_named_keys_still_resolve_by_name() {
        assert_eq!(named(w::NamedKey::F13), Key::Named(NamedKey::F13));
        assert_eq!(named(w::NamedKey::F35), Key::Named(NamedKey::F35));
        assert_eq!(
            named(w::NamedKey::MediaPlayPause),
            Key::Named(NamedKey::MediaPlayPause)
        );
        assert_eq!(
            named(w::NamedKey::MediaTrackPrevious),
            Key::Named(NamedKey::MediaTrackPrevious)
        );
        assert_eq!(
            named(w::NamedKey::AudioVolumeDown),
            Key::Named(NamedKey::AudioVolumeDown)
        );
        assert_eq!(
            named(w::NamedKey::BrightnessUp),
            Key::Named(NamedKey::BrightnessUp)
        );
        assert_eq!(
            named(w::NamedKey::LaunchWebBrowser),
            Key::Named(NamedKey::LaunchWebBrowser)
        );
        assert_eq!(
            named(w::NamedKey::BrowserFavorites),
            Key::Named(NamedKey::BrowserFavorites)
        );
        assert_eq!(
            named(w::NamedKey::AllCandidates),
            Key::Named(NamedKey::AllCandidates)
        );
        assert_eq!(
            named(w::NamedKey::NextCandidate),
            Key::Named(NamedKey::NextCandidate)
        );
        assert_eq!(named(w::NamedKey::Soft1), Key::Named(NamedKey::Soft1));
        assert_eq!(named(w::NamedKey::Soft4), Key::Named(NamedKey::Soft4));
        assert_eq!(
            named(w::NamedKey::ZoomToggle),
            Key::Named(NamedKey::ZoomToggle)
        );
        assert_eq!(
            named(w::NamedKey::ColorF0Red),
            Key::Named(NamedKey::ColorF0Red)
        );
        assert_eq!(named(w::NamedKey::Standby), Key::Named(NamedKey::Standby));
        assert_eq!(named(w::NamedKey::Eject), Key::Named(NamedKey::Eject));
        assert_eq!(named(w::NamedKey::Print), Key::Named(NamedKey::Print));
        assert_eq!(named(w::NamedKey::Save), Key::Named(NamedKey::Save));
        assert_eq!(named(w::NamedKey::GoHome), Key::Named(NamedKey::GoHome));
        // The longest name either enum has — the one that sizes `NameBuf`.
        assert_eq!(
            named(w::NamedKey::TVAudioDescriptionMixDown),
            Key::Named(NamedKey::TVAudioDescriptionMixDown)
        );
    }

    // -- Degradation. --------------------------------------------------------

    #[test]
    fn keys_the_platform_could_not_name_degrade_rather_than_panic() {
        assert_eq!(
            map_code(w::PhysicalKey::Unidentified(w::NativeKeyCode::Windows(
                0xffff
            ))),
            Code::Unidentified
        );
        assert_eq!(
            map_key(w::Key::Unidentified(w::NativeKey::Windows(0xffff))),
            Key::Named(NamedKey::Unidentified)
        );
        // winit reports a dead key's text separately; the spec puts it in the
        // composition events, so only the marker survives the mapping.
        assert_eq!(map_key(w::Key::Dead(Some('^'))), Key::Named(NamedKey::Dead));
    }

    #[test]
    fn a_name_the_spec_does_not_share_yields_nothing() {
        #[derive(Debug)]
        enum Fake {
            NotAKeyTheSpecKnows,
        }
        assert_eq!(by_name::<_, Code>(&Fake::NotAKeyTheSpecKnows), None);
    }

    #[test]
    fn a_name_too_long_for_the_buffer_yields_nothing_rather_than_truncating() {
        // A truncated name could in principle parse as some *other* key, so the
        // buffer refuses the write instead of filling what fits.
        #[derive(Debug)]
        enum Fake {
            AVariantNameDefinitelyLongerThanThirtyTwoBytes,
        }
        assert_eq!(
            by_name::<_, Code>(&Fake::AVariantNameDefinitelyLongerThanThirtyTwoBytes),
            None
        );
    }

    #[test]
    fn every_modifier_winit_tracks_survives() {
        let all = w::ModifiersState::SHIFT
            | w::ModifiersState::CONTROL
            | w::ModifiersState::ALT
            | w::ModifiersState::SUPER;
        let mapped = map_modifiers(all);
        assert!(mapped.shift() && mapped.ctrl() && mapped.alt() && mapped.meta());
        assert!(map_modifiers(w::ModifiersState::empty()).is_empty());
    }
}
