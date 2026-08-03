//! The font the web build ships with.
//!
//! # Why this exists
//!
//! Every desktop platform has a font database to enumerate. A browser does not
//! expose one, and both text stacks degrade *silently* when they find nothing:
//!
//! - parley/fontique selects an explicit dummy backend on
//!   `wasm32-unknown-unknown` — empty family map, empty generic families,
//!   `family()` and `fallback()` both `None`. Shaping then produces no glyph
//!   runs at all.
//! - suzuri's `load_system_fonts` goes through `fontdb`, whose implementation
//!   is a series of `#[cfg(target_os = ...)]` blocks with no arm matching wasm,
//!   so it is a no-op. `Text::shape` queries `Family::SansSerif`, misses, and
//!   returns an empty layout.
//!
//! In both cases the result is not an error but a widget that measures 0x0 and
//! draws nothing. So the web build carries a font of its own.
//!
//! # Which font, and the licence
//!
//! Noto Sans JP (SIL Open Font License 1.1), which covers Latin, kana and CJK
//! from one file — the alternative being a Latin font plus a separate CJK font
//! plus fallback wiring. The OFL permits redistribution and embedding,
//! including inside a compiled binary, provided the licence travels with it;
//! see `assets/NotoSansJP-VF.ttf.LICENSE`.
//!
//! # Size
//!
//! It is ~9.6 MB, which dominates the wasm bundle. That is a deliberate first
//! step, not the end state: a `pyftsubset` run restricted to the character set
//! a page actually uses cuts it by well over an order of magnitude, and because
//! this is a plain `include_bytes!` of one file, swapping in a subset later is
//! a file replacement and nothing more.
//!
//! Native builds do **not** include this — they enumerate real system fonts, so
//! embedding would cost 9.6 MB for nothing.

/// The embedded font, compiled into web builds only.
pub(crate) const FONT_BYTES: &[u8] = include_bytes!("assets/NotoSansJP-VF.ttf");

/// Register [`FONT_BYTES`] with a parley `FontContext` and point every generic
/// family at it.
///
/// Mapping all four generics matters: a widget that asks for `system-ui` — the
/// default `font_family` of `RichText` and `TextBox` — would otherwise resolve
/// to nothing even with the font present.
pub(crate) fn register_with_parley(font_cx: &mut parley::FontContext) {
    use parley::fontique::{Blob, GenericFamily};
    use std::sync::Arc;

    let blob = Blob::new(Arc::new(FONT_BYTES));
    let registered = font_cx.collection.register_fonts(blob, None);

    let Some((family_id, _)) = registered.first() else {
        log::error!("the embedded font registered no families; text will not render");
        return;
    };

    for generic in [
        GenericFamily::SansSerif,
        GenericFamily::SystemUi,
        GenericFamily::Serif,
        GenericFamily::Monospace,
    ] {
        font_cx
            .collection
            .set_generic_families(generic, core::iter::once(*family_id));
    }
}

/// Register [`FONT_BYTES`] with suzuri's `FontSystem` and make it the
/// sans-serif family, which is what `Text::shape` queries for.
pub(crate) fn register_with_suzuri(font_system: &suzuri::FontSystem) {
    font_system.load_font_binary(FONT_BYTES.to_vec());

    // Take the family name from the face the load produced rather than
    // hardcoding "Noto Sans JP": swapping in a subset (or a different font)
    // should not require editing a string here to match.
    match font_system
        .faces()
        .first()
        .and_then(|f| f.families.first())
        .cloned()
    {
        Some((name, _)) => font_system.set_sans_serif_family(name),
        None => log::error!("the embedded font registered no faces; text will not render"),
    }
}
