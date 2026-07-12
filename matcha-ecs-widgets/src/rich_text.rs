//! `RichText` — a parley-backed word-wrapped text leaf widget, developed
//! alongside (not replacing) [`crate::Text`] (suzuri-backed).
//!
//! `Text` uses suzuri (fontdb + fontdue): no real shaping (kerning only) and
//! no font fallback, so mixed-script or ligature-heavy text can render
//! incorrectly. `RichText` shapes via parley (HarfRust shaping + fontique
//! font fallback) and rasterises glyphs via swash, but reuses the exact same
//! GPU compositing trick `Text` already established: a 1x1 solid-colour
//! "tint" quad (`texture_atlas`) masked by a per-glyph coverage bitmap
//! (`stencil_atlas`, `R8Unorm`) via `RenderNode::with_stencil` — see
//! `renderer/src/core_renderer/renderer_render.wgsl`'s
//! `final_color = texture_color * stencil_atlas.r`.
//!
//! **CSS text-property coverage (added in a later pass, see `CLAUDE.md`'s
//! dated entry for the full design writeup)**: `RichText` now reproduces most
//! of CSS's text-styling surface that parley 0.11.0 can express, including
//! per-span (per-substring) style overrides via [`RichText::span`]/[`RichSpan`]
//! — font-family (+ fallback lists), font-size, font-weight, font-style,
//! font-stretch/width, font-variation-settings, font-feature-settings,
//! per-span colour (via parley's `Brush`), line-height, letter-spacing,
//! word-spacing, word-break, overflow-wrap, locale, text-align, text-indent,
//! text-transform (uppercase/lowercase/capitalize — a matcha-side string
//! pre-process, not a parley feature), white-space (`Normal`/`Pre` collapsing,
//! widget-level only), and real underline/strikethrough rendering (colour,
//! offset, thickness, all span-overridable).
//!
//! **Deliberately unsupported — not "not yet implemented", but not
//! achievable without forking or wrapping parley 0.11.0** (do not file
//! against this widget expecting these are just unwired): CSS `text-overflow`
//! (ellipsis), `tab-size`, `vertical-align`, `text-shadow`, `overline`
//! decoration, forced `direction`/`unicode-bidi` override (bidi is fully
//! automatic), and vertical writing-mode/`text-orientation` (horizontal only).
//!
//! Still deferred, but *technically* approachable in a future pass (unlike
//! the list above): parley's synthetic bold/oblique font synthesis when no
//! true bold/italic face exists in the fallback chain (would need a
//! `GlyphKey` field to avoid a same-key-different-bitmap collision — see
//! `GlyphKey`'s doc comment), colour glyphs (emoji — rasterisation only
//! accepts `Content::Mask`, alpha coverage), sub-pixel glyph positioning
//! (layout is quantized to whole pixels instead), and any shape-result
//! caching (matching `Text`: `measure`/`arrange`/the `RenderItem` builder
//! each independently re-shape from scratch).

use std::{
    collections::HashMap,
    num::NonZeroUsize,
    ops::Range,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

use bevy_ecs::{
    bundle::Bundle,
    change_detection::DetectChangesMut,
    component::Component,
    entity::Entity,
    resource::Resource,
    world::EntityWorldMut,
};
use gpu_utils::texture_atlas::AtlasRegion;
use nalgebra::{Matrix4, Vector3};
use parking_lot::Mutex;
use renderer::RenderNode;

use matcha_ecs::{
    components::{
        render::{RenderCtx, RenderItem, RenderOpacity},
        view::{Key, ManualDespawn},
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, SUB_PIXEL_QUANTIZE},
    view::Widget,
};

use crate::animation::{Easing, ExitFade, OpacityTween};

/// The displayed content: a fully assembled string (base text + every span's
/// text, in declaration order, each already transform/collapse-processed)
/// plus zero or more style-override spans layered over byte ranges of it —
/// CSS's "same text, differently styled sub-ranges" model. Assembled once by
/// `RichText::resolved_content` at spawn/patch time; `shape()` never touches
/// `RichText`'s own builder-side fields, only this resolved form.
#[derive(Component, Clone, PartialEq, Debug)]
pub struct RichTextContent {
    text: String,
    spans: Vec<ResolvedSpan>,
}

/// Per-span style overrides, all optional — an unset field inherits the
/// widget-level default (CSS-inheritance-like). Shared shape between the
/// public [`RichSpan`] builder and the resolved, range-anchored form stored
/// in [`RichTextContent`].
#[derive(Clone, PartialEq, Debug, Default)]
struct SpanOverrides {
    font_size: Option<f32>,
    color: Option<[f32; 4]>,
    font_family: Option<String>,
    font_weight: Option<parley::FontWeight>,
    font_style: Option<parley::FontStyle>,
    font_width: Option<parley::FontWidth>,
    font_variations: Option<String>,
    font_features: Option<String>,
    line_height: Option<parley::LineHeight>,
    letter_spacing: Option<f32>,
    word_spacing: Option<f32>,
    word_break: Option<parley::WordBreak>,
    overflow_wrap: Option<parley::OverflowWrap>,
    locale: Option<String>,
    // Each decoration sub-property is independently overridable (matches CSS
    // `text-decoration-line`/`-color`/`-offset`/`-thickness` cascading as
    // separate properties) — so these are double-`Option`: the outer layer
    // is "was this span-overridden at all" (mirroring every other field
    // above), the inner layer is the property's own "use default" `None`.
    underline: Option<bool>,
    underline_color: Option<Option<[f32; 4]>>,
    underline_offset: Option<Option<f32>>,
    underline_size: Option<Option<f32>>,
    strikethrough: Option<bool>,
    strikethrough_color: Option<Option<[f32; 4]>>,
    strikethrough_offset: Option<Option<f32>>,
    strikethrough_size: Option<Option<f32>>,
}

/// A span override resolved to its final byte range within
/// `RichTextContent::text` (after transform + whitespace-collapse
/// remapping).
#[derive(Clone, PartialEq, Debug)]
struct ResolvedSpan {
    range: Range<usize>,
    overrides: SpanOverrides,
}

/// A style-override builder passed to [`RichText::span`]. Exposes the same
/// overridable properties as `RichText`'s own widget-level builders, except
/// `text_align`/`text_indent`/`white_space` — those are block-level only
/// (parley's `Layout::align`/`set_text_indent` operate on the whole layout,
/// not a byte range; whitespace collapsing runs once over the fully
/// assembled text, not per span).
pub struct RichSpan {
    text: String,
    overrides: SpanOverrides,
    text_transform: Option<TextTransform>,
}

impl RichSpan {
    fn new(text: String) -> Self {
        Self {
            text,
            overrides: SpanOverrides::default(),
            text_transform: None,
        }
    }

    pub fn font_size(mut self, px: f32) -> Self {
        self.overrides.font_size = Some(px);
        self
    }

    pub fn color(mut self, rgba: [f32; 4]) -> Self {
        self.overrides.color = Some(rgba);
        self
    }

    pub fn font_family(mut self, css_list: impl Into<String>) -> Self {
        self.overrides.font_family = Some(css_list.into());
        self
    }

    pub fn font_weight(mut self, weight: parley::FontWeight) -> Self {
        self.overrides.font_weight = Some(weight);
        self
    }

    pub fn font_style(mut self, style: parley::FontStyle) -> Self {
        self.overrides.font_style = Some(style);
        self
    }

    pub fn font_width(mut self, width: parley::FontWidth) -> Self {
        self.overrides.font_width = Some(width);
        self
    }

    pub fn font_variations(mut self, css: impl Into<String>) -> Self {
        self.overrides.font_variations = Some(css.into());
        self
    }

    pub fn font_features(mut self, css: impl Into<String>) -> Self {
        self.overrides.font_features = Some(css.into());
        self
    }

    pub fn line_height(mut self, line_height: parley::LineHeight) -> Self {
        self.overrides.line_height = Some(line_height);
        self
    }

    pub fn letter_spacing(mut self, px: f32) -> Self {
        self.overrides.letter_spacing = Some(px);
        self
    }

    pub fn word_spacing(mut self, px: f32) -> Self {
        self.overrides.word_spacing = Some(px);
        self
    }

    pub fn word_break(mut self, word_break: parley::WordBreak) -> Self {
        self.overrides.word_break = Some(word_break);
        self
    }

    pub fn overflow_wrap(mut self, overflow_wrap: parley::OverflowWrap) -> Self {
        self.overrides.overflow_wrap = Some(overflow_wrap);
        self
    }

    pub fn locale(mut self, bcp47: impl Into<String>) -> Self {
        self.overrides.locale = Some(bcp47.into());
        self
    }

    pub fn text_transform(mut self, transform: TextTransform) -> Self {
        self.text_transform = Some(transform);
        self
    }

    pub fn underline(mut self, enabled: bool) -> Self {
        self.overrides.underline = Some(enabled);
        self
    }

    /// `None` = inherit the resolved text colour (CSS `currentColor`).
    pub fn underline_color(mut self, rgba: Option<[f32; 4]>) -> Self {
        self.overrides.underline_color = Some(rgba);
        self
    }

    /// `None` = use the font's own underline metrics.
    pub fn underline_offset(mut self, px: Option<f32>) -> Self {
        self.overrides.underline_offset = Some(px);
        self
    }

    /// `None` = use the font's own underline metrics.
    pub fn underline_size(mut self, px: Option<f32>) -> Self {
        self.overrides.underline_size = Some(px);
        self
    }

    pub fn strikethrough(mut self, enabled: bool) -> Self {
        self.overrides.strikethrough = Some(enabled);
        self
    }

    /// `None` = inherit the resolved text colour (CSS `currentColor`).
    pub fn strikethrough_color(mut self, rgba: Option<[f32; 4]>) -> Self {
        self.overrides.strikethrough_color = Some(rgba);
        self
    }

    /// `None` = use the font's own strikethrough metrics.
    pub fn strikethrough_offset(mut self, px: Option<f32>) -> Self {
        self.overrides.strikethrough_offset = Some(px);
        self
    }

    /// `None` = use the font's own strikethrough metrics.
    pub fn strikethrough_size(mut self, px: Option<f32>) -> Self {
        self.overrides.strikethrough_size = Some(px);
        self
    }
}

/// A pending span, as accumulated by `RichText::span` before final assembly
/// (`RichText::resolved_content`) resolves its text into a byte range.
#[derive(Clone, PartialEq, Debug)]
struct PendingSpan {
    text: String,
    overrides: SpanOverrides,
    text_transform: Option<TextTransform>,
}

/// CSS `text-transform`. Applied as pre-shaping string rewriting — parley has
/// no equivalent `StyleProperty`, this is pure Rust string processing baked
/// into `RichTextContent` at spawn/patch time (see `RichText::resolved_content`).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

/// CSS `white-space` collapsing behaviour. Not threaded through parley's own
/// `WhiteSpaceCollapse` (that enum is only reachable via `TreeBuilder`, and
/// `RichText` uses `RangedBuilder`) — implemented as matcha-side
/// pre-processing instead. Only the two parley-native wrapping semantics are
/// offered (no CSS `pre-wrap`/`pre-line`, which would need to also toggle
/// `TextWrapMode` alongside collapsing).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum WhiteSpace {
    #[default]
    Normal,
    Pre,
}

/// Apply CSS `text-transform` to `s`. Locale-independent (`str::to_uppercase`/
/// `to_lowercase` use Unicode default casing, not a specific language's rules).
fn apply_text_transform(s: &str, transform: TextTransform) -> String {
    match transform {
        TextTransform::None => s.to_string(),
        TextTransform::Uppercase => s.to_uppercase(),
        TextTransform::Lowercase => s.to_lowercase(),
        TextTransform::Capitalize => {
            let mut result = String::with_capacity(s.len());
            let mut at_word_start = true;
            for c in s.chars() {
                if c.is_whitespace() {
                    at_word_start = true;
                    result.push(c);
                } else if at_word_start {
                    result.extend(c.to_uppercase());
                    at_word_start = false;
                } else {
                    result.push(c);
                }
            }
            result
        }
    }
}

/// Collapse every maximal run of Unicode whitespace (including tabs/newlines)
/// into a single space, matching CSS `white-space: normal`'s basic collapsing
/// rule — and remap a set of byte ranges (e.g. span overrides) from `text`'s
/// original offsets to their corresponding offsets in the collapsed output, in
/// the same pass. A span boundary that lands inside a collapsed-away
/// whitespace run maps to the position immediately after the single surviving
/// space, so ranges never overlap and never lose non-whitespace content
/// across a boundary. Does not trim leading/trailing whitespace — a
/// whitespace run at the very start/end of the text still collapses to
/// exactly one space rather than being removed, since removal is a
/// box-layout concern (browsers trim visually via the containing box, not by
/// deleting text), which this widget has no equivalent of.
fn collapse_white_space_with_span_remap<T: Clone>(
    text: &str,
    spans: &[(Range<usize>, T)],
) -> (String, Vec<(Range<usize>, T)>) {
    let mut new_text = String::with_capacity(text.len());
    // old_to_new[i] = byte offset in `new_text` corresponding to old byte
    // offset `i`. Only ever indexed at char-boundary offsets (span ranges are
    // always built from `String::len()` after pushing whole segments, so
    // they're always char-boundary aligned) plus `text.len()` itself.
    let mut old_to_new = vec![0usize; text.len() + 1];
    let mut prev_was_space = false;
    for (old_idx, c) in text.char_indices() {
        old_to_new[old_idx] = new_text.len();
        if c.is_whitespace() {
            if !prev_was_space {
                new_text.push(' ');
            }
            prev_was_space = true;
        } else {
            new_text.push(c);
            prev_was_space = false;
        }
    }
    old_to_new[text.len()] = new_text.len();

    let remapped = spans
        .iter()
        .map(|(range, payload)| (old_to_new[range.start]..old_to_new[range.end], payload.clone()))
        .collect();
    (new_text, remapped)
}

/// CSS `text-decoration-line`/`-color`/`-offset`/`-thickness`, for one line
/// kind (underline or strikethrough). `color`/`offset`/`size` being `None`
/// means "use the font/text default" — parley resolves that itself (falling
/// back to the text colour for `color`, and to `Run::metrics()` for
/// `offset`/`size`), not this widget.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
struct DecorationStyle {
    enabled: bool,
    color: Option<[f32; 4]>,
    offset: Option<f32>,
    size: Option<f32>,
}

/// Draw-relevant text properties other than the content itself. Not `Copy`
/// (font_family/font_variations/font_features/locale are string-backed) —
/// `Layout` only requires `Component + Clone`.
#[derive(Component, Clone, PartialEq, Debug)]
struct RichTextStyle {
    font_size: f32,
    color: [f32; 4],
    font_family: String,
    font_weight: parley::FontWeight,
    font_style: parley::FontStyle,
    font_width: parley::FontWidth,
    font_variations: Option<String>,
    font_features: Option<String>,
    line_height: parley::LineHeight,
    letter_spacing: f32,
    word_spacing: f32,
    word_break: parley::WordBreak,
    overflow_wrap: parley::OverflowWrap,
    locale: Option<String>,
    text_align: parley::Alignment,
    text_indent: f32,
    underline: DecorationStyle,
    strikethrough: DecorationStyle,
}

/// Shares the most recently resolved wrap width between
/// `RichTextStyle::arrange` (writer, every layout pass) and the `RenderItem`
/// builder (reader, every rebuild). See `Text`'s identical `TextWrapWidth`
/// for the full rationale (not shared with it directly, to keep the two
/// widgets fully independent while both exist).
#[derive(Component)]
struct RichTextWrapWidth(Arc<AtomicU32>);

impl RichTextWrapWidth {
    fn new() -> Self {
        Self(Arc::new(AtomicU32::new(f32::MAX.to_bits())))
    }

    fn store(&self, width: f32) {
        self.0.store(width.to_bits(), Ordering::Relaxed);
    }
}

/// Identifies one rasterised glyph: font face + glyph index + quantized size
/// + variation coordinates. `normalized_coords` is a variable-length slice,
/// so it's folded into a hash rather than stored verbatim — a collision
/// would only ever produce a cosmetically wrong glyph for variable fonts
/// sharing a size, never a correctness issue, and is astronomically
/// unlikely in practice.
///
/// Font-weight/style/width/features/variations differences are all already
/// distinguished correctly by the fields below: a weight/style/width change
/// that resolves to a genuinely different font file already changes
/// `font_blob_id`/`font_index`; font-features affect which glyph id gets
/// *chosen* during shaping, not how a given glyph id rasterises; and
/// font-variations changes are captured by `coords_hash` (each run carries
/// its own resolved `normalized_coords`). One real gap, deliberately not
/// closed here: `fontique::Synthesis` (synthetic bold/oblique when the
/// fallback chain has no true face for a requested weight/style) is never
/// applied, specifically because doing so would need a new field here — the
/// same `font_blob_id` + `glyph_id` would otherwise need to represent two
/// different bitmaps (plain vs. synthetically embellished) depending on
/// which run asked for it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    font_blob_id: u64,
    font_index: u32,
    glyph_id: u32,
    font_size_bits: u32,
    coords_hash: u64,
}

/// Glyph stencils are cached in a fixed-capacity, LRU + batch-protected
/// `GlyphCache` (see the `glyph-cache` crate) rather than an unbounded map:
/// unlike `Text`'s `stencil_cache`, `RichText` is expected to draw arbitrary
/// runtime text (timecodes, filenames, ...) where an unbounded cache would
/// grow forever. Eviction only ever drops this map's own entry — any
/// `AtlasRegion` already baked into a built `RenderNode` keeps itself alive
/// via its own `Arc` clone (RAII deallocation on last drop), so evicting a
/// glyph here never corrupts an already-rendered frame, only means it will
/// be re-rasterised if drawn again later.
const GLYPH_CACHE_CAPACITY: usize = 1024;

/// parley's per-glyph "paint" type — wraps a resolved RGBA colour. `Default`
/// only matters for parley-internal bookkeeping; every real run gets an
/// explicit colour pushed via `push_default`/`push` (widget default or span
/// override), so a default-valued brush is never actually surfaced.
#[derive(Clone, PartialEq, Debug, Default)]
struct RichTextBrush([f32; 4]);

struct ParleyFontCtxInner {
    font_cx: Mutex<parley::FontContext>,
    layout_cx: Mutex<parley::LayoutContext<RichTextBrush>>,
    scale_cx: Mutex<swash::scale::ScaleContext>,
    /// Per-glyph rasterised coverage bitmap (or `None` for glyphs with no
    /// visible bitmap, e.g. space — caching that avoids re-rasterising them
    /// every frame), shared across every `RichText` entity/frame drawing the
    /// same glyph at the same size.
    stencil_cache: Mutex<glyph_cache::GlyphCache<GlyphKey, Option<(AtlasRegion, [f32; 2], [i32; 2])>>>,
}

/// World resource wrapping parley's `FontContext`/`LayoutContext`, swash's
/// `ScaleContext`, and the glyph stencil cache. Lazily inserted on first use,
/// matching `Text`'s `FontCtx` pattern exactly.
#[derive(Resource, Clone)]
struct ParleyFontCtx(Arc<ParleyFontCtxInner>);

impl ParleyFontCtx {
    fn new() -> Self {
        Self(Arc::new(ParleyFontCtxInner {
            font_cx: Mutex::new(parley::FontContext::new()),
            layout_cx: Mutex::new(parley::LayoutContext::new()),
            scale_cx: Mutex::new(swash::scale::ScaleContext::new()),
            stencil_cache: Mutex::new(glyph_cache::GlyphCache::new(
                NonZeroUsize::new(GLYPH_CACHE_CAPACITY).expect("GLYPH_CACHE_CAPACITY is a nonzero constant"),
            )),
        }))
    }

    /// Marks the start of a new eviction-protection batch. Called once per
    /// `RenderItem` builder invocation (see `rich_text_render_item`) — not
    /// once per rendered frame, since a `RenderItem` is only rebuilt when a
    /// draw-relevant prop actually changes (`RenderItem::invalidate`), which
    /// can be far less often than every frame. A "batch" is therefore one
    /// full shape-and-draw pass over one `RichText` entity's glyphs: this
    /// protects every glyph that pass touches from being evicted by a later
    /// glyph in the *same* pass, while still allowing eviction across
    /// different (unrelated, or later) rebuilds.
    fn begin_glyph_batch(&self) {
        self.0.stencil_cache.lock().new_batch();
    }

    /// Look up (or rasterise-and-cache) the stencil atlas region holding
    /// `key`'s coverage bitmap, plus its pixel size and its placement
    /// (offset of the bitmap's top-left corner from the pen position).
    /// Returns `None` both when the glyph has no visible bitmap and when the
    /// cache had no room left this batch (see `GlyphCache::get_or_insert_with`)
    /// — either way, the caller should simply skip drawing this glyph.
    fn stencil_region(
        &self,
        key: GlyphKey,
        glyph_id: swash::GlyphId,
        scaler: &mut swash::scale::Scaler,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &gpu_utils::texture_atlas::TextureAtlas,
    ) -> Option<(AtlasRegion, [f32; 2], [i32; 2])> {
        self.0
            .stencil_cache
            .lock()
            .get_or_insert_with(key, || rasterize_and_upload(glyph_id, scaler, device, queue, atlas))
            .cloned()
            .flatten()
    }
}

/// Rasterise `glyph_id` via swash (alpha coverage mask only — colour glyphs
/// are skipped, see module docs) and upload it into the stencil atlas.
fn rasterize_and_upload(
    glyph_id: swash::GlyphId,
    scaler: &mut swash::scale::Scaler,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &gpu_utils::texture_atlas::TextureAtlas,
) -> Option<(AtlasRegion, [f32; 2], [i32; 2])> {
    let image = swash::scale::Render::new(&[swash::scale::Source::Outline])
        .format(swash::zeno::Format::Alpha)
        .render(scaler, glyph_id)?;

    if image.content != swash::scale::image::Content::Mask
        || image.placement.width == 0
        || image.placement.height == 0
    {
        return None;
    }

    let region = match atlas.allocate(device, queue, [image.placement.width, image.placement.height]) {
        Ok(region) => region,
        Err(e) => {
            log::error!("RichText glyph stencil allocation failed: {e}");
            return None;
        }
    };
    if let Err(e) = region.write_data(queue, &image.data) {
        log::error!("RichText glyph stencil upload failed: {e}");
        return None;
    }

    Some((
        region,
        [image.placement.width as f32, image.placement.height as f32],
        [image.placement.left, image.placement.top],
    ))
}

/// Push every set field of `overrides` onto `builder` for `range` — the
/// per-span counterpart to `shape()`'s widget-default `push_default` calls.
/// `underline`/`strikethrough` overrides are not included here (RT4).
fn push_span_overrides(builder: &mut parley::RangedBuilder<'_, RichTextBrush>, overrides: &SpanOverrides, range: Range<usize>) {
    if let Some(v) = overrides.font_size {
        builder.push(parley::StyleProperty::FontSize(v), range.clone());
    }
    if let Some(v) = overrides.color {
        builder.push(parley::StyleProperty::Brush(RichTextBrush(v)), range.clone());
    }
    if let Some(v) = &overrides.font_family {
        builder.push(parley::StyleProperty::FontFamily(parley::FontFamily::from(v.as_str())), range.clone());
    }
    if let Some(v) = overrides.font_weight {
        builder.push(parley::StyleProperty::FontWeight(v), range.clone());
    }
    if let Some(v) = overrides.font_style {
        builder.push(parley::StyleProperty::FontStyle(v), range.clone());
    }
    if let Some(v) = overrides.font_width {
        builder.push(parley::StyleProperty::FontWidth(v), range.clone());
    }
    if let Some(v) = &overrides.font_variations {
        builder.push(parley::StyleProperty::FontVariations(v.as_str().into()), range.clone());
    }
    if let Some(v) = &overrides.font_features {
        builder.push(parley::StyleProperty::FontFeatures(v.as_str().into()), range.clone());
    }
    if let Some(v) = overrides.line_height {
        builder.push(parley::StyleProperty::LineHeight(v), range.clone());
    }
    if let Some(v) = overrides.letter_spacing {
        builder.push(parley::StyleProperty::LetterSpacing(v), range.clone());
    }
    if let Some(v) = overrides.word_spacing {
        builder.push(parley::StyleProperty::WordSpacing(v), range.clone());
    }
    if let Some(v) = overrides.word_break {
        builder.push(parley::StyleProperty::WordBreak(v), range.clone());
    }
    if let Some(v) = overrides.overflow_wrap {
        builder.push(parley::StyleProperty::OverflowWrap(v), range.clone());
    }
    if let Some(locale) = overrides.locale.as_deref().and_then(|s| parley::Language::parse(s).ok()) {
        builder.push(parley::StyleProperty::Locale(Some(locale)), range.clone());
    }
    if let Some(v) = overrides.underline {
        builder.push(parley::StyleProperty::Underline(v), range.clone());
    }
    if let Some(v) = overrides.underline_color {
        builder.push(parley::StyleProperty::UnderlineBrush(v.map(RichTextBrush)), range.clone());
    }
    if let Some(v) = overrides.underline_offset {
        builder.push(parley::StyleProperty::UnderlineOffset(v), range.clone());
    }
    if let Some(v) = overrides.underline_size {
        builder.push(parley::StyleProperty::UnderlineSize(v), range.clone());
    }
    if let Some(v) = overrides.strikethrough {
        builder.push(parley::StyleProperty::Strikethrough(v), range.clone());
    }
    if let Some(v) = overrides.strikethrough_color {
        builder.push(parley::StyleProperty::StrikethroughBrush(v.map(RichTextBrush)), range.clone());
    }
    if let Some(v) = overrides.strikethrough_offset {
        builder.push(parley::StyleProperty::StrikethroughOffset(v), range.clone());
    }
    if let Some(v) = overrides.strikethrough_size {
        builder.push(parley::StyleProperty::StrikethroughSize(v), range);
    }
}

/// Shape `content` fresh (no caching — see module docs) under `style`
/// (widget-level defaults) plus `content`'s per-span overrides, word-wrapping
/// at `max_width`.
fn shape(
    font_ctx: &ParleyFontCtx,
    content: &RichTextContent,
    style: &RichTextStyle,
    max_width: f32,
) -> parley::Layout<RichTextBrush> {
    let inner = &font_ctx.0;
    let mut fcx = inner.font_cx.lock();
    let mut lcx = inner.layout_cx.lock();

    let mut builder = lcx.ranged_builder(&mut fcx, &content.text, 1.0, true);
    builder.push_default(parley::StyleProperty::FontFamily(parley::FontFamily::from(
        style.font_family.as_str(),
    )));
    builder.push_default(parley::StyleProperty::FontSize(style.font_size));
    builder.push_default(parley::StyleProperty::FontWeight(style.font_weight));
    builder.push_default(parley::StyleProperty::FontStyle(style.font_style));
    builder.push_default(parley::StyleProperty::FontWidth(style.font_width));
    builder.push_default(parley::StyleProperty::Brush(RichTextBrush(style.color)));
    builder.push_default(parley::StyleProperty::LineHeight(style.line_height));
    builder.push_default(parley::StyleProperty::LetterSpacing(style.letter_spacing));
    builder.push_default(parley::StyleProperty::WordSpacing(style.word_spacing));
    builder.push_default(parley::StyleProperty::WordBreak(style.word_break));
    builder.push_default(parley::StyleProperty::OverflowWrap(style.overflow_wrap));
    if let Some(variations) = &style.font_variations {
        builder.push_default(parley::StyleProperty::FontVariations(variations.as_str().into()));
    }
    if let Some(features) = &style.font_features {
        builder.push_default(parley::StyleProperty::FontFeatures(features.as_str().into()));
    }
    if let Some(locale) = style.locale.as_deref().and_then(|s| parley::Language::parse(s).ok()) {
        builder.push_default(parley::StyleProperty::Locale(Some(locale)));
    }
    builder.push_default(parley::StyleProperty::Underline(style.underline.enabled));
    builder.push_default(parley::StyleProperty::UnderlineBrush(style.underline.color.map(RichTextBrush)));
    builder.push_default(parley::StyleProperty::UnderlineOffset(style.underline.offset));
    builder.push_default(parley::StyleProperty::UnderlineSize(style.underline.size));
    builder.push_default(parley::StyleProperty::Strikethrough(style.strikethrough.enabled));
    builder.push_default(parley::StyleProperty::StrikethroughBrush(style.strikethrough.color.map(RichTextBrush)));
    builder.push_default(parley::StyleProperty::StrikethroughOffset(style.strikethrough.offset));
    builder.push_default(parley::StyleProperty::StrikethroughSize(style.strikethrough.size));

    for span in &content.spans {
        push_span_overrides(&mut builder, &span.overrides, span.range.clone());
    }

    let mut layout: parley::Layout<RichTextBrush> = builder.build(&content.text);
    layout.break_all_lines(Some(max_width));
    layout.align(style.text_align, parley::AlignmentOptions::default());
    if style.text_indent != 0.0 {
        layout.set_text_indent(style.text_indent, parley::IndentOptions::default());
    }
    layout
}

/// Gamma-encode a linear colour component into the sRGB space the atlas
/// texture is stored in. See `Text`'s identical `linear_to_srgb_u8` for the
/// full rationale (`write_data` is a raw byte copy with no automatic
/// linear->sRGB conversion).
fn linear_to_srgb_u8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let encoded = if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

/// Paint a 1x1 solid-colour region into `ctx.texture_atlas`, reused
/// (UV-clamped to any on-screen size) as every glyph's stencil "tint". See
/// `Text`'s identical `paint_tint_region` for why this is written directly
/// via `write_data` rather than a `ColorRect`-style render pass.
fn paint_tint_region(ctx: &RenderCtx, color: [f32; 4]) -> Option<AtlasRegion> {
    let region = match ctx.texture_atlas.allocate(ctx.device, ctx.queue, [1, 1]) {
        Ok(region) => region,
        Err(e) => {
            log::error!("RichText tint region allocation failed: {e}");
            return None;
        }
    };

    let alpha = (color[3] * ctx.opacity).clamp(0.0, 1.0);
    let bytes = [
        linear_to_srgb_u8(color[0]),
        linear_to_srgb_u8(color[1]),
        linear_to_srgb_u8(color[2]),
        (alpha * 255.0).round() as u8,
    ];
    if let Err(e) = region.write_data(ctx.queue, &bytes) {
        log::error!("RichText tint upload failed: {e}");
        return None;
    }

    Some(region)
}

/// Build a `RenderItem` that shapes `content` fresh every rebuild (reading
/// the live wrap width from `wrap_width`) and draws each glyph as a
/// tint-texture quad masked by its cached stencil coverage bitmap.
fn rich_text_render_item(
    font_ctx: ParleyFontCtx,
    wrap_width: Arc<AtomicU32>,
    content: RichTextContent,
    style: RichTextStyle,
) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let mut node = RenderNode::new();
        if content.text.is_empty() {
            return node;
        }

        let max_width = f32::from_bits(wrap_width.load(Ordering::Relaxed));
        let layout = shape(&font_ctx, &content, &style, max_width);

        // Per-span colour means a single build can need several distinct
        // tint regions (one per distinct colour actually used) — deduped
        // locally, scoped to this one `RenderItem` rebuild, no persistent
        // cache/eviction needed (typically only a handful of colours).
        let mut tint_regions: HashMap<[u32; 4], AtlasRegion> = HashMap::new();
        let mut tint_for = |color: [f32; 4]| -> Option<AtlasRegion> {
            let key = [color[0].to_bits(), color[1].to_bits(), color[2].to_bits(), color[3].to_bits()];
            if let Some(region) = tint_regions.get(&key) {
                return Some(region.clone());
            }
            let region = paint_tint_region(ctx, color)?;
            tint_regions.insert(key, region.clone());
            Some(region)
        };

        font_ctx.begin_glyph_batch();
        let mut scale_cx = font_ctx.0.scale_cx.lock();

        for line in layout.lines() {
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font = run.font();
                let font_size_px = run.font_size();
                let coords = run.normalized_coords();

                let Some(font_ref) = swash::FontRef::from_index(font.data.as_ref(), font.index as usize) else {
                    continue;
                };
                let mut scaler = scale_cx
                    .builder(font_ref)
                    .size(font_size_px)
                    .hint(true)
                    .normalized_coords(coords)
                    .build();

                let font_size_bits = (font_size_px * SUB_PIXEL_QUANTIZE).round() as u32;
                let coords_hash = fxhash::hash64(coords);

                // Each `GlyphRun` carries one resolved style (parley starts a
                // new run wherever a style — including brush — changes), so
                // the run's colour is already fully resolved: no manual
                // span/byte-range lookup needed here.
                let Some(tint_region) = tint_for(glyph_run.style().brush.0) else {
                    continue;
                };

                let mut pen_x = glyph_run.offset();
                let baseline = glyph_run.baseline();

                for glyph in glyph_run.glyphs() {
                    let gx = pen_x + glyph.x;
                    let gy = baseline + glyph.y;
                    pen_x += glyph.advance;

                    let key = GlyphKey {
                        font_blob_id: font.data.id(),
                        font_index: font.index,
                        glyph_id: glyph.id,
                        font_size_bits,
                        coords_hash,
                    };

                    let Some((stencil_region, size, placement)) = font_ctx.stencil_region(
                        key,
                        glyph.id as swash::GlyphId,
                        &mut scaler,
                        ctx.device,
                        ctx.queue,
                        ctx.stencil_atlas,
                    ) else {
                        continue;
                    };

                    let px = gx.floor() + placement[0] as f32;
                    let py = gy.floor() - placement[1] as f32;
                    let transform = Matrix4::new_translation(&Vector3::new(px, py, 0.0));
                    let glyph_node = RenderNode::new()
                        .with_texture(tint_region.clone(), size, Matrix4::identity())
                        .with_stencil(stencil_region, size, Matrix4::identity());
                    node.push_child(glyph_node, transform);
                }

                // Underline/strikethrough: a flat filled rectangle, not a
                // glyph — no `.with_stencil(..)` coverage mask needed.
                // `y = baseline - offset` matches parley's own reference
                // renderers (e.g. `examples/swash_render` in the parley
                // repo) exactly.
                let run_metrics = run.metrics();
                let run_style = glyph_run.style();
                for (decoration, default_offset, default_size) in [
                    (&run_style.underline, run_metrics.underline_offset, run_metrics.underline_size),
                    (&run_style.strikethrough, run_metrics.strikethrough_offset, run_metrics.strikethrough_size),
                ] {
                    let Some(decoration) = decoration else {
                        continue;
                    };
                    let Some(deco_tint) = tint_for(decoration.brush.0) else {
                        continue;
                    };
                    let offset = decoration.offset.unwrap_or(default_offset);
                    let size = decoration.size.unwrap_or(default_size).max(1.0);
                    let y = baseline - offset;
                    let deco_transform = Matrix4::new_translation(&Vector3::new(glyph_run.offset(), y, 0.0));
                    let deco_node = RenderNode::new().with_texture(deco_tint, [glyph_run.advance(), size], Matrix4::identity());
                    node.push_child(deco_node, deco_transform);
                }
            }
        }

        node
    })
}

impl Layout for RichTextStyle {
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, constraints: Constraints) -> [f32; 2] {
        let Some(font_ctx) = ctx.world().get_resource::<ParleyFontCtx>() else {
            return [0.0, 0.0];
        };
        let Some(content) = ctx.world().get::<RichTextContent>(me) else {
            return [0.0, 0.0];
        };
        if content.text.is_empty() {
            return [0.0, 0.0];
        }
        let layout = shape(font_ctx, content, self, constraints.max_width());
        [
            layout.width().clamp(constraints.min_width(), constraints.max_width()),
            layout.height().clamp(constraints.min_height(), constraints.max_height()),
        ]
    }

    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, size: [f32; 2]) {
        if let Some(wrap_width) = ctx.world().get::<RichTextWrapWidth>(me) {
            wrap_width.store(size[0]);
        }
    }
}

/// A word-wrapped, shaped-via-parley text block of fixed style, sized to its
/// shaped content. See module docs for how this relates to [`crate::Text`].
pub struct RichText {
    key: Key,
    content: String,
    font_size: f32,
    color: [f32; 4],
    font_family: String,
    font_weight: parley::FontWeight,
    font_style: parley::FontStyle,
    font_width: parley::FontWidth,
    font_variations: Option<String>,
    font_features: Option<String>,
    line_height: parley::LineHeight,
    letter_spacing: f32,
    word_spacing: f32,
    word_break: parley::WordBreak,
    overflow_wrap: parley::OverflowWrap,
    locale: Option<String>,
    text_align: parley::Alignment,
    text_indent: f32,
    text_transform: TextTransform,
    white_space: WhiteSpace,
    underline: DecorationStyle,
    strikethrough: DecorationStyle,
    spans: Vec<PendingSpan>,
    enter_fade: Option<(Duration, Easing)>,
    exit_fade: Option<(Duration, Easing)>,
}

impl RichText {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            key: Key::Auto,
            content: content.into(),
            font_size: 16.0,
            color: [0.0, 0.0, 0.0, 1.0],
            font_family: "system-ui".to_string(),
            font_weight: parley::FontWeight::NORMAL,
            font_style: parley::FontStyle::Normal,
            font_width: parley::FontWidth::NORMAL,
            font_variations: None,
            font_features: None,
            line_height: parley::LineHeight::FontSizeRelative(1.3),
            letter_spacing: 0.0,
            word_spacing: 0.0,
            word_break: parley::WordBreak::Normal,
            overflow_wrap: parley::OverflowWrap::Normal,
            locale: None,
            text_align: parley::Alignment::Start,
            text_indent: 0.0,
            text_transform: TextTransform::None,
            white_space: WhiteSpace::Normal,
            underline: DecorationStyle::default(),
            strikethrough: DecorationStyle::default(),
            spans: Vec::new(),
            enter_fade: None,
            exit_fade: None,
        }
    }

    /// Append a differently-styled run of text after whatever's been
    /// declared so far (the base text, and/or any earlier spans) — CSS's
    /// "same text, differently styled sub-ranges" model. Any style field not
    /// set on the span inherits this `RichText`'s widget-level default.
    pub fn span(mut self, content: impl Into<String>, build: impl FnOnce(RichSpan) -> RichSpan) -> Self {
        let span = build(RichSpan::new(content.into()));
        self.spans.push(PendingSpan {
            text: span.text,
            overrides: span.overrides,
            text_transform: span.text_transform,
        });
        self
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    pub fn font_size(mut self, font_size: f32) -> Self {
        self.font_size = font_size;
        self
    }

    pub fn color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    /// CSS `font-family` list syntax (e.g. `"Inter, system-ui, sans-serif"`) —
    /// parsed by parley itself, including generic family keywords.
    pub fn font_family(mut self, css_list: impl Into<String>) -> Self {
        self.font_family = css_list.into();
        self
    }

    pub fn font_weight(mut self, weight: parley::FontWeight) -> Self {
        self.font_weight = weight;
        self
    }

    pub fn font_style(mut self, style: parley::FontStyle) -> Self {
        self.font_style = style;
        self
    }

    /// CSS `font-stretch`/`font-width`.
    pub fn font_width(mut self, width: parley::FontWidth) -> Self {
        self.font_width = width;
        self
    }

    /// Raw CSS `font-variation-settings` string, e.g. `"'wght' 650"`.
    pub fn font_variations(mut self, css: impl Into<String>) -> Self {
        self.font_variations = Some(css.into());
        self
    }

    /// Raw CSS `font-feature-settings` string, e.g. `"'liga' 0"`.
    pub fn font_features(mut self, css: impl Into<String>) -> Self {
        self.font_features = Some(css.into());
        self
    }

    pub fn line_height(mut self, line_height: parley::LineHeight) -> Self {
        self.line_height = line_height;
        self
    }

    pub fn letter_spacing(mut self, px: f32) -> Self {
        self.letter_spacing = px;
        self
    }

    pub fn word_spacing(mut self, px: f32) -> Self {
        self.word_spacing = px;
        self
    }

    pub fn word_break(mut self, word_break: parley::WordBreak) -> Self {
        self.word_break = word_break;
        self
    }

    pub fn overflow_wrap(mut self, overflow_wrap: parley::OverflowWrap) -> Self {
        self.overflow_wrap = overflow_wrap;
        self
    }

    /// BCP 47 language tag, e.g. `"ja"` or `"zh-Hans-CN"`.
    pub fn locale(mut self, bcp47: impl Into<String>) -> Self {
        self.locale = Some(bcp47.into());
        self
    }

    pub fn text_align(mut self, align: parley::Alignment) -> Self {
        self.text_align = align;
        self
    }

    pub fn text_indent(mut self, px: f32) -> Self {
        self.text_indent = px;
        self
    }

    pub fn text_transform(mut self, transform: TextTransform) -> Self {
        self.text_transform = transform;
        self
    }

    pub fn white_space(mut self, white_space: WhiteSpace) -> Self {
        self.white_space = white_space;
        self
    }

    pub fn underline(mut self, enabled: bool) -> Self {
        self.underline.enabled = enabled;
        self
    }

    /// `None` = inherit the resolved text colour (CSS `currentColor`).
    pub fn underline_color(mut self, rgba: Option<[f32; 4]>) -> Self {
        self.underline.color = rgba;
        self
    }

    /// `None` = use the font's own underline metrics.
    pub fn underline_offset(mut self, px: Option<f32>) -> Self {
        self.underline.offset = px;
        self
    }

    /// `None` = use the font's own underline metrics.
    pub fn underline_size(mut self, px: Option<f32>) -> Self {
        self.underline.size = px;
        self
    }

    pub fn strikethrough(mut self, enabled: bool) -> Self {
        self.strikethrough.enabled = enabled;
        self
    }

    /// `None` = inherit the resolved text colour (CSS `currentColor`).
    pub fn strikethrough_color(mut self, rgba: Option<[f32; 4]>) -> Self {
        self.strikethrough.color = rgba;
        self
    }

    /// `None` = use the font's own strikethrough metrics.
    pub fn strikethrough_offset(mut self, px: Option<f32>) -> Self {
        self.strikethrough.offset = px;
        self
    }

    /// `None` = use the font's own strikethrough metrics.
    pub fn strikethrough_size(mut self, px: Option<f32>) -> Self {
        self.strikethrough.size = px;
        self
    }

    pub fn enter_fade(mut self, duration: Duration, easing: Easing) -> Self {
        self.enter_fade = Some((duration, easing));
        self
    }

    pub fn exit_fade(mut self, duration: Duration, easing: Easing) -> Self {
        self.exit_fade = Some((duration, easing));
        self
    }

    fn style(&self) -> RichTextStyle {
        RichTextStyle {
            font_size: self.font_size,
            color: self.color,
            font_family: self.font_family.clone(),
            font_weight: self.font_weight,
            font_style: self.font_style,
            font_width: self.font_width,
            font_variations: self.font_variations.clone(),
            font_features: self.font_features.clone(),
            line_height: self.line_height,
            letter_spacing: self.letter_spacing,
            word_spacing: self.word_spacing,
            word_break: self.word_break,
            overflow_wrap: self.overflow_wrap,
            locale: self.locale.clone(),
            text_align: self.text_align,
            text_indent: self.text_indent,
            underline: self.underline,
            strikethrough: self.strikethrough,
        }
    }

    /// Assemble the base text and every span's text (each independently
    /// `text_transform`-applied first, since order doesn't matter for casing)
    /// into one string, tracking each span's byte range as it's appended;
    /// then collapse whitespace once over the whole assembled text (if
    /// `white_space == Normal`), remapping every span's range through that
    /// collapse in the same pass. Baked in once here rather than redone by
    /// `shape()` on every call.
    fn resolved_content(&self) -> RichTextContent {
        let mut assembled = apply_text_transform(&self.content, self.text_transform);
        let mut spans: Vec<(Range<usize>, SpanOverrides)> = Vec::with_capacity(self.spans.len());
        for pending in &self.spans {
            let transform = pending.text_transform.unwrap_or(self.text_transform);
            let transformed = apply_text_transform(&pending.text, transform);
            let start = assembled.len();
            assembled.push_str(&transformed);
            spans.push((start..assembled.len(), pending.overrides.clone()));
        }

        let (text, spans) = match self.white_space {
            WhiteSpace::Normal => collapse_white_space_with_span_remap(&assembled, &spans),
            WhiteSpace::Pre => (assembled, spans),
        };

        RichTextContent {
            text,
            spans: spans.into_iter().map(|(range, overrides)| ResolvedSpan { range, overrides }).collect(),
        }
    }

    fn rebuild_render_item(&self, entity: &mut EntityWorldMut) -> RenderItem {
        let font_ctx = entity.world_scope(|world| world.get_resource_or_insert_with(ParleyFontCtx::new).clone());
        let wrap_width = entity
            .get::<RichTextWrapWidth>()
            .expect("bundle() inserted RichTextWrapWidth")
            .0
            .clone();
        rich_text_render_item(font_ctx, wrap_width, self.resolved_content(), self.style())
    }
}

impl Widget for RichText {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        let initial_opacity = if self.enter_fade.is_some() { 0.0 } else { 1.0 };
        (
            self.resolved_content(),
            self.style(),
            RichTextWrapWidth::new(),
            LayoutDispatch::of::<RichTextStyle>(),
            RenderOpacity(initial_opacity),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        let item = self.rebuild_render_item(entity);
        entity.insert(item);

        if let Some((duration, easing)) = self.enter_fade {
            entity.insert(OpacityTween {
                from: 0.0,
                to: 1.0,
                start: web_time::Instant::now(),
                duration,
                easing,
            });
        }
        if let Some((duration, easing)) = self.exit_fade {
            entity.insert((ManualDespawn::new(), ExitFade { duration, easing }));
        }
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        let mut changed = false;
        if let Some(mut c) = entity.get_mut::<RichTextContent>() {
            changed |= c.set_if_neq(self.resolved_content());
        }
        if let Some(mut s) = entity.get_mut::<RichTextStyle>() {
            changed |= s.set_if_neq(self.style());
        }
        if changed {
            let item = self.rebuild_render_item(entity);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }

        // Revival: see `Text::patch`/`ColorRect::patch` for the identical
        // reasoning.
        if entity.get::<ManualDespawn>().is_some_and(|m| m.is_pruned()) {
            if let Some(exit) = entity.get::<ExitFade>().copied() {
                let current = entity.get::<RenderOpacity>().copied().unwrap_or_default();
                entity.insert(OpacityTween {
                    from: current.0,
                    to: 1.0,
                    start: web_time::Instant::now(),
                    duration: exit.duration,
                    easing: exit.easing,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! `RichTextWrapWidth` is a private implementation detail, so unlike the
    //! public-API integration tests in `matcha-ecs/tests/rich_text.rs`, its
    //! write-through from `RichTextStyle::arrange` can only be checked from
    //! inside this crate. Mirrors `text.rs`'s identical unit test.
    use bevy_ecs::world::World;
    use matcha_ecs::{
        components::view::ViewChildren,
        layout::{layout_root, Constraints},
        view::run_view,
    };

    use super::*;

    #[test]
    fn arrange_writes_its_resolved_width_into_rich_text_wrap_width() {
        let mut world = World::new();
        let root = world.spawn(ViewChildren::default()).id();
        run_view(&mut world, root, |s| {
            s.leaf(RichText::new("hi").font_size(16.0));
        });
        layout_root(&mut world, root, Constraints::from_max_size([123.0, 456.0]));

        let child = world.get::<ViewChildren>(root).unwrap().slots[0].1;
        let stored_width = world.get::<RichTextWrapWidth>(child).unwrap().0.load(Ordering::Relaxed);
        let stored_width = f32::from_bits(stored_width);

        let out = world.get::<matcha_ecs::components::layout::LayoutOutput>(child).unwrap();
        assert_eq!(
            stored_width, out.size[0],
            "RichTextWrapWidth must hold exactly the width arrange() resolved this entity to"
        );
    }

    #[test]
    fn apply_text_transform_uppercase_lowercase_capitalize() {
        assert_eq!(apply_text_transform("Hello World", TextTransform::None), "Hello World");
        assert_eq!(apply_text_transform("Hello World", TextTransform::Uppercase), "HELLO WORLD");
        assert_eq!(apply_text_transform("Hello World", TextTransform::Lowercase), "hello world");
        assert_eq!(
            apply_text_transform("hello   world", TextTransform::Capitalize),
            "Hello   World"
        );
        // Non-ASCII: uppercasing an accented character must not just no-op.
        assert_eq!(apply_text_transform("café", TextTransform::Uppercase), "CAFÉ");
        assert_eq!(apply_text_transform("CAFÉ", TextTransform::Lowercase), "café");
    }

    #[test]
    fn collapse_white_space_merges_runs_into_a_single_space() {
        let collapse = |s: &str| collapse_white_space_with_span_remap::<()>(s, &[]).0;
        assert_eq!(collapse("hello   world"), "hello world");
        assert_eq!(collapse("hello\t\nworld"), "hello world");
        assert_eq!(collapse(" leading"), " leading");
        assert_eq!(collapse("trailing "), "trailing ");
        assert_eq!(collapse("no-runs-here"), "no-runs-here");
    }

    #[test]
    fn collapse_white_space_with_span_remap_keeps_ranges_consistent_across_a_boundary() {
        // "hello " (span A, trailing space) + " world" (span B, leading space)
        // — the two spaces at the join must collapse to exactly one, and both
        // spans' remapped ranges must stay non-overlapping and lose no
        // non-whitespace content.
        let text = "hello  world";
        let span_a = 0..6; // "hello " (includes the first of the two joining spaces)
        let span_b = 6..12; // " world" (includes the second joining space)
        let (collapsed, remapped) =
            collapse_white_space_with_span_remap(text, &[(span_a, "a"), (span_b, "b")]);

        assert_eq!(collapsed, "hello world");
        assert_eq!(remapped.len(), 2);
        let (a_range, a_payload) = &remapped[0];
        let (b_range, b_payload) = &remapped[1];
        assert_eq!(*a_payload, "a");
        assert_eq!(*b_payload, "b");
        assert_eq!(&collapsed[a_range.clone()], "hello ");
        assert_eq!(&collapsed[b_range.clone()], "world");
        assert!(a_range.end <= b_range.start, "spans must not overlap after remapping");
    }

    #[test]
    fn collapse_white_space_with_span_remap_handles_a_span_entirely_inside_a_collapsed_run() {
        // A span covering only the second and third spaces of a 3-space run
        // (not the first) must remap to an empty range at the position right
        // after the single surviving space, not something that overlaps or
        // duplicates content.
        let text = "a   b";
        let outer = 0..5; // whole string
        let inner_tail_of_run = 2..4; // the 2nd and 3rd spaces only
        let (collapsed, remapped) =
            collapse_white_space_with_span_remap(text, &[(outer, "outer"), (inner_tail_of_run, "inner")]);

        assert_eq!(collapsed, "a b");
        let (outer_range, _) = &remapped[0];
        let (inner_range, _) = &remapped[1];
        assert_eq!(&collapsed[outer_range.clone()], "a b");
        assert!(inner_range.is_empty(), "a span covering only already-collapsed whitespace must remap to empty");
    }
}
