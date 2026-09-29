//! Application-wide font registration.
//!
//! # Why an app has to do this at all
//!
//! Every desktop platform has a font database to enumerate. A browser does
//! not, and **both of this crate's text stacks degrade silently** when they
//! find nothing:
//!
//! - parley/fontique selects an explicit dummy backend on
//!   `wasm32-unknown-unknown` — empty family map, empty generic families,
//!   `family()` and `fallback()` both `None`. Shaping produces no glyph runs.
//! - suzuri's `load_system_fonts` goes through `fontdb`, whose implementation
//!   is a series of `#[cfg(target_os = ...)]` blocks with no arm matching
//!   wasm, so it is a no-op. [`crate::text`]'s `shape` queries
//!   `Family::SansSerif`, misses, and returns an empty layout.
//!
//! Neither is an error: the result is a widget that measures 0x0 and draws
//! nothing. So a web build must supply a font of its own, and register it with
//! **both** stacks — `RichText`/`TextBox` shape through parley, while
//! `Text` *and `Button`'s label* shape through suzuri. Registering with only
//! one leaves every button blank.
//!
//! # Usage
//!
//! ```ignore
//! use matcha_ecs_widgets::{FontData, WithDefaultFont};
//!
//! static FONT: LazyLock<FontData> =
//!     LazyLock::new(|| Arc::new(include_bytes!("assets/MyFont.ttf").as_slice()));
//!
//! let app = UiEcs::new(model, view, reduce).with_default_font(FONT.clone());
//! ```
//!
//! One registration covers every widget, so individual widgets do not need
//! `.font(..)` — that builder stays available for the "this one widget uses a
//! different font" case, and layers on top of whatever was registered here.
//!
//! # Which font, and the licence
//!
//! Not this crate's business any more: the app chooses. `matcha-web` embeds
//! Noto Sans JP (SIL Open Font License 1.1), which covers Latin, kana and CJK
//! from one file; see `matcha-web/build.rs` for how it is fetched and why the
//! bytes are not in git while the licence is.

use std::sync::Arc;

use bevy_ecs::world::World;

use crate::{rich_text::ParleyFontCtx, text::FontCtx};

/// Font file bytes, from wherever the app has them.
///
/// A trait object rather than a concrete `Arc<Vec<u8>>` so that **both**
/// `Arc::new(vec)` and `Arc::new(include_bytes!(..).as_slice())` are accepted
/// — the latter without copying the file out of the binary's data segment,
/// which is worth ~10 MB of a browser's heap for a CJK font.
///
/// This is also exactly the type parley's `Blob::new` takes, so the bytes
/// reach fontique with no copy at all. suzuri's `load_font_binary` wants an
/// owned `Vec<u8>` and so still copies once; that is its API, not a choice
/// made here.
pub type FontData = Arc<dyn AsRef<[u8]> + Send + Sync + 'static>;

/// The bytes behind a [`FontData`]. Two `as_ref` hops (through the `Arc`, then
/// through `AsRef`) is awkward enough at every call site to be worth naming.
pub(crate) fn font_bytes(data: &FontData) -> &[u8] {
    (**data).as_ref()
}

/// Which fonts a text stack has already been given, and whether one of them
/// has been made its default family.
///
/// Shared by both stacks because the rule is the same in both and the failure
/// mode of getting it subtly different is invisible: text that does not draw.
///
/// # Locking
///
/// The mutex around this must be held across the backend registration call,
/// not just across the membership check. Releasing it in between lets a second
/// caller observe "already registered" while the first has not actually
/// handed the bytes to the backend yet, and shape against a font that is not
/// there.
#[derive(Default)]
pub(crate) struct FontRegistry {
    /// The `Arc`s are kept alive deliberately: identity here is *pointer*
    /// identity, and a dropped allocation could be reused at the same address,
    /// making a new font compare equal to an old one and silently skip
    /// registration.
    ///
    /// A hash of the bytes would avoid that but costs a pass over ~10 MB per
    /// call, and parley cannot help: `Blob::new` stamps every blob with a fresh
    /// id from a global counter, so fontique cannot detect a duplicate either.
    fonts: Vec<FontData>,
    default_set: bool,
}

impl FontRegistry {
    pub(crate) fn contains(&self, data: &FontData) -> bool {
        self.fonts.iter().any(|font| Arc::ptr_eq(font, data))
    }

    /// Record a **successful** registration.
    ///
    /// A font that produced no families/faces is deliberately not recorded, so
    /// a later font can still become the default and re-offering the same one
    /// retries rather than being swallowed as a duplicate.
    pub(crate) fn record(&mut self, data: &FontData) {
        self.fonts.push(data.clone());
    }

    /// Claim the "this one is the default family" slot, once.
    ///
    /// Keyed on whether a default has been set — *not* on whether this is the
    /// first font offered — so a first font that failed to load does not
    /// consume the slot and leave the stack with no default forever.
    pub(crate) fn claim_default_slot(&mut self) -> bool {
        let unclaimed = !self.default_set;
        self.default_set = true;
        unclaimed
    }
}

/// Register `data` as the application's default font with both text stacks.
///
/// Idempotent per `Arc` identity: passing the same handle again is a no-op, so
/// this is safe to call from a startup path that may run more than once. The
/// first font to register **successfully** becomes the default family for both
/// stacks (every parley generic family, and suzuri's sans-serif); later ones
/// are available for lookup but do not displace it.
///
/// This is not gated on the target: it is an explicit request, and honouring
/// it only in a browser would be a platform difference with no signal. A
/// native app that never calls it keeps enumerating system fonts as before.
///
/// [`WithDefaultFont::with_default_font`] is the same thing as a builder step,
/// and is the normal way to call this. Use this form when you already hold a
/// `&mut World` — from a system, or after the app is running.
pub fn register_default_font(world: &mut World, data: FontData) {
    world
        .get_resource_or_insert_with(ParleyFontCtx::new)
        .clone()
        .ensure_registered(&data);
    world
        .get_resource_or_insert_with(FontCtx::new)
        .clone()
        .ensure_registered(&data);
}

/// [`register_default_font`] as a builder step on `UiEcs`.
///
/// An extension trait rather than an inherent method because `matcha-ecs`
/// depends on nothing from this crate — the font contexts are widget-owned
/// types, so the core cannot name them.
pub trait WithDefaultFont {
    /// Register `data` as the application's default font. See
    /// [`register_default_font`].
    fn with_default_font(self, data: FontData) -> Self;
}

impl<M, Msg, F, R> WithDefaultFont for matcha_ecs::ui_ecs::UiEcs<M, Msg, F, R>
where
    M: Send + Sync + 'static,
    Msg: matcha_ecs::components::input::Message,
    F: Fn(&M, &mut matcha_ecs::view::Scope) + Send + Sync + 'static,
    R: Fn(&mut M, Msg) + Send + Sync + 'static,
{
    fn with_default_font(self, data: FontData) -> Self {
        let for_suzuri = data.clone();
        self.configure_resource(ParleyFontCtx::new, |ctx| ctx.ensure_registered(&data))
            .configure_resource(FontCtx::new, |ctx| ctx.ensure_registered(&for_suzuri))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(bytes: &[u8]) -> FontData {
        Arc::new(bytes.to_vec())
    }

    #[test]
    fn identity_is_the_handle_not_the_bytes() {
        let mut registry = FontRegistry::default();
        let a = font(b"same");
        let b = font(b"same");

        registry.record(&a);

        assert!(registry.contains(&a));
        assert!(
            !registry.contains(&b),
            "identical bytes behind a different Arc are a different font: comparing \
             contents would cost a pass over ~10 MB per call"
        );
        assert!(registry.contains(&a.clone()), "a clone is the same handle");
    }

    #[test]
    fn only_the_first_recorded_font_claims_the_default_slot() {
        let mut registry = FontRegistry::default();
        assert!(registry.claim_default_slot());
        assert!(!registry.claim_default_slot());
        assert!(!registry.claim_default_slot());
    }

    /// The bug this shape exists to prevent: keying the default family on
    /// "is this the first font offered" rather than "has a default been set"
    /// meant a first font that failed to load consumed the slot, and **no
    /// later font could ever become the default** — every widget then drew
    /// nothing, with one log line to go on.
    #[test]
    fn a_font_that_failed_to_load_leaves_the_default_slot_open() {
        let mut registry = FontRegistry::default();
        let broken = font(b"not a font");
        let good = font(b"pretend this one loaded");

        // The failure path returns before recording, so nothing is remembered.
        assert!(!registry.contains(&broken));

        registry.record(&good);
        assert!(
            registry.claim_default_slot(),
            "the second font must still be able to become the default"
        );
    }

    /// And re-offering a font that failed is a retry, not a silently swallowed
    /// duplicate.
    #[test]
    fn a_font_that_failed_to_load_can_be_offered_again() {
        let mut registry = FontRegistry::default();
        let broken = font(b"not a font");
        assert!(!registry.contains(&broken));
        assert!(!registry.contains(&broken));
    }
}
