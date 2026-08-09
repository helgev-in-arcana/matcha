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
//! use matcha_ecs_widgets::font::WithDefaultFont;
//!
//! let app = UiEcs::new(model, view, reduce);
//! #[cfg(target_arch = "wasm32")]
//! let app = app.with_default_font(FONT.clone());
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

/// Register `data` as the application's default font with both text stacks.
///
/// Idempotent per `Arc` identity: passing the same `Arc` again is a no-op, so
/// this is safe to call from a startup path that may run more than once. The
/// **first** font registered becomes the default family for both stacks (every
/// parley generic family, and suzuri's sans-serif); later ones are available
/// for lookup but do not displace it.
///
/// This is not gated on the target: it is an explicit request, and honouring
/// it only in a browser would be a platform difference with no signal. A
/// native app that never calls it keeps enumerating system fonts as before.
pub fn register_default_font(world: &mut World, data: Arc<Vec<u8>>) {
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
    fn with_default_font(self, data: Arc<Vec<u8>>) -> Self;
}

impl<M, Msg, F, R> WithDefaultFont for matcha_ecs::ui_ecs::UiEcs<M, Msg, F, R>
where
    M: Send + Sync + 'static,
    Msg: matcha_ecs::components::input::Message,
    F: Fn(&M, &mut matcha_ecs::view::Scope) + Send + Sync + 'static,
    R: Fn(&mut M, Msg) + Send + Sync + 'static,
{
    fn with_default_font(mut self, data: Arc<Vec<u8>>) -> Self {
        register_default_font(self.world_mut(), data);
        self
    }
}
