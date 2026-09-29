//! `RichText` — a parley-backed word-wrapped text leaf widget.
//!
//! `Text` uses suzuri (fontdb + fontdue): no real shaping (kerning only) and
//! no font fallback, so mixed-script or ligature-heavy text can render
//! incorrectly. `RichText` shapes via parley (HarfRust shaping + fontique
//! font fallback) and rasterises glyphs via swash. Scene Objects and PixelMasks
//! reference shared colour TextureSources and glyph MaskSources. Swash produces
//! bounds and pixels together; the source retains pixels for GPU regeneration.
//! Writers retain only glyph definitions used by their current layout, in
//! addition to the shared bounded LRU. A visible layout larger than that LRU
//! remains complete and keeps stable IDs on subsequent redraws.
//!
//! CSS-style text properties include
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
//! The widget does not expose CSS `text-overflow`
//! (ellipsis), `tab-size`, `vertical-align`, `text-shadow`, `overline`
//! decoration, forced `direction`/`unicode-bidi` override (bidi is fully
//! automatic), and vertical writing-mode/`text-orientation` (horizontal only).
//!
//! Glyph rasterisation accepts alpha coverage (`Content::Mask`) only, so colour
//! glyphs are skipped. Synthetic bold/oblique is not applied, and placement is
//! quantized to whole pixels. The writer caches its most recent shaped layout
//! by wrap width; layout measurement shapes independently.

use std::{num::NonZeroUsize, ops::Range, sync::Arc, time::Duration};

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    resource::Resource, world::EntityWorldMut,
};
use matcha_ecs::scene::Draw;
use nalgebra::{Matrix4, Vector3};
use parking_lot::Mutex;
use render_interface::{MaskDescriptor, MaskSource, upload_texture};

use matcha_ecs::{
    components::{
        render::{RenderCtx, RenderItem, RenderOpacity},
        view::{Key, ManualDespawn},
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured, SUB_PIXEL_QUANTIZE},
    view::Widget,
};

use crate::animation::{Easing, ExitFade, OpacityTween};
use crate::live::LiveF32;
use crate::sizing::Sizing;

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
        .map(|(range, payload)| {
            (
                old_to_new[range.start]..old_to_new[range.end],
                payload.clone(),
            )
        })
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
/// writer (reader, every redraw). Each widget type owns its wrap-width cell,
/// which survives replacement of its style and draw writer.
#[derive(Component)]
struct RichTextWrapWidth(Arc<LiveF32>);

impl RichTextWrapWidth {
    fn new() -> Self {
        Self(Arc::new(LiveF32::new(f32::MAX)))
    }

    fn store(&self, width: f32) {
        self.0.set(width);
    }
}

/// Identifies one rasterised glyph: font face + glyph index + quantized size
/// + variation coordinates. `normalized_coords` is a variable-length slice,
/// so the key stores its hash. A hash collision can reuse an incorrect glyph
/// definition for matching font, glyph and size fields.
///
/// Font-weight/style/width/features/variations differences are all already
/// distinguished correctly by the fields below: a weight/style/width change
/// that resolves to a genuinely different font file already changes
/// `font_blob_id`/`font_index`; font-features affect which glyph id gets
/// *chosen* during shaping, not how a given glyph id rasterises; and
/// font-variations changes are captured by `coords_hash` (each run carries
/// its own resolved `normalized_coords`). `fontique::Synthesis` (synthetic
/// bold/oblique when the fallback chain has no true face) is not applied. Any
/// implementation of synthesis must distinguish it in this key, since the
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

/// The shared LRU keeps definitions available across writers without retaining
/// every glyph ever encountered. Each writer separately retains the definitions
/// used by its current layout in ActiveGlyphs. Thus visible text may exceed this
/// sharing budget without disappearing or acquiring fresh IDs on every redraw.
const GLYPH_CACHE_CAPACITY: usize = 1024;

type GlyphDefinition = Option<(MaskSource, [f32; 2], [i32; 2])>;

/// Resource definitions used by a writer's current layout, not retained Objects.
/// Revisit each key during drawing and discard keys no longer present afterward.
/// Negative entries retain genuinely invisible glyphs, never capacity failures.
#[derive(Default)]
pub(crate) struct ActiveGlyphs {
    entries: fxhash::FxHashMap<GlyphKey, (GlyphDefinition, bool)>,
}

impl ActiveGlyphs {
    fn begin(&mut self) {
        for (_, seen) in self.entries.values_mut() {
            *seen = false;
        }
    }

    fn glyph_source(
        &mut self,
        font_ctx: &ParleyFontCtx,
        key: GlyphKey,
        build: impl FnOnce() -> GlyphDefinition,
    ) -> GlyphDefinition {
        let (definition, seen) = self
            .entries
            .entry(key)
            .or_insert_with(|| (font_ctx.glyph_source(key, build), true));
        *seen = true;
        definition.clone()
    }

    fn finish(&mut self) {
        self.entries.retain(|_, (_, seen)| *seen);
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// parley's per-glyph "paint" type — wraps a resolved RGBA colour. `Default`
/// only matters for parley-internal bookkeeping; every real run gets an
/// explicit colour pushed via `push_default`/`push` (widget default or span
/// override), so a default-valued brush is never actually surfaced.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct RichTextBrush(pub(crate) [f32; 4]);

pub(crate) struct ParleyFontCtxInner {
    pub(crate) font_cx: Mutex<parley::FontContext>,
    pub(crate) layout_cx: Mutex<parley::LayoutContext<RichTextBrush>>,
    pub(crate) scale_cx: Mutex<swash::scale::ScaleContext>,
    /// Per-glyph rasterised coverage bitmap (or `None` for glyphs with no
    /// visible bitmap, e.g. space — caching that avoids re-rasterising them
    /// every frame), shared across every `RichText` entity/frame drawing the
    /// same glyph at the same size.
    stencil_cache: Mutex<glyph_cache::GlyphCache<GlyphKey, GlyphDefinition>>,
}

/// World resource wrapping parley's `FontContext`/`LayoutContext`, swash's
/// `ScaleContext`, and the glyph stencil cache. Lazily inserted on first use,
/// matching `Text`'s `FontCtx` pattern exactly.
#[derive(Resource, Clone)]
pub(crate) struct ParleyFontCtx(pub(crate) Arc<ParleyFontCtxInner>);

impl ParleyFontCtx {
    pub(crate) fn new() -> Self {
        Self(Arc::new(ParleyFontCtxInner {
            font_cx: Mutex::new(parley::FontContext::new()),
            layout_cx: Mutex::new(parley::LayoutContext::new()),
            scale_cx: Mutex::new(swash::scale::ScaleContext::new()),
            stencil_cache: Mutex::new(glyph_cache::GlyphCache::new(
                NonZeroUsize::new(GLYPH_CACHE_CAPACITY)
                    .expect("GLYPH_CACHE_CAPACITY is a nonzero constant"),
            )),
        }))
    }

    /// Protect one writer's glyph run from eviction during that traversal.
    /// This runs each redraw; shaping itself is cached separately by wrap width.
    pub(crate) fn begin_glyph_batch(&self) {
        self.0.stencil_cache.lock().new_batch();
    }

    /// Look up the MaskSource defining
    /// `key`'s coverage bitmap, plus its pixel size and its placement
    /// (offset of the bitmap's top-left corner from the pen position).
    /// Returns `None` only for genuinely invisible/unsupported glyphs. If every
    /// shared cache entry is protected, build outside that cache; ActiveGlyphs
    /// retains the result for this writer's following frames.
    fn glyph_source(
        &self,
        key: GlyphKey,
        build: impl FnOnce() -> GlyphDefinition,
    ) -> GlyphDefinition {
        let mut build = Some(build);
        let cached = self
            .0
            .stencil_cache
            .lock()
            .get_or_insert_with(key, || build.take().expect("cache miss builds once")())
            .cloned();
        match cached {
            Some(definition) => definition,
            None => build
                .take()
                .expect("a full protected cache does not invoke its builder")(),
        }
    }
}

/// Rasterise `glyph_id` via swash (alpha coverage mask only — colour glyphs
/// are skipped, see module docs) and retain the bytes in a lazy MaskSource.
fn rasterize_bitmap(
    glyph_id: swash::GlyphId,
    scaler: &mut swash::scale::Scaler,
) -> Option<(MaskSource, [f32; 2], [i32; 2])> {
    let image = swash::scale::Render::new(&[swash::scale::Source::Outline])
        .format(swash::zeno::Format::Alpha)
        .render(scaler, glyph_id)?;

    if image.content != swash::scale::image::Content::Mask
        || image.placement.width == 0
        || image.placement.height == 0
    {
        return None;
    }

    let region = MaskSource::new(
        MaskDescriptor::new(
            [image.placement.width, image.placement.height],
            wgpu::TextureFormat::R8Unorm,
        ),
        move |mut c| upload_texture(&mut c.gpu, &c.target, &image.data),
    )
    .with_output_layout(render_interface::PrepareOutputLayout::AnyRegion);

    Some((
        region,
        [image.placement.width as f32, image.placement.height as f32],
        [image.placement.left, image.placement.top],
    ))
}

/// Push every set field of `overrides` onto `builder` for `range` — the
/// per-span counterpart to `shape()`'s widget-default `push_default` calls.
/// Includes underline and strikethrough properties, resolved independently.
fn push_span_overrides(
    builder: &mut parley::RangedBuilder<'_, RichTextBrush>,
    overrides: &SpanOverrides,
    range: Range<usize>,
) {
    if let Some(v) = overrides.font_size {
        builder.push(parley::StyleProperty::FontSize(v), range.clone());
    }
    if let Some(v) = overrides.color {
        builder.push(
            parley::StyleProperty::Brush(RichTextBrush(v)),
            range.clone(),
        );
    }
    if let Some(v) = &overrides.font_family {
        builder.push(
            parley::StyleProperty::FontFamily(parley::FontFamily::from(v.as_str())),
            range.clone(),
        );
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
        builder.push(
            parley::StyleProperty::FontVariations(v.as_str().into()),
            range.clone(),
        );
    }
    if let Some(v) = &overrides.font_features {
        builder.push(
            parley::StyleProperty::FontFeatures(v.as_str().into()),
            range.clone(),
        );
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
    if let Some(locale) = overrides
        .locale
        .as_deref()
        .and_then(|s| parley::Language::parse(s).ok())
    {
        builder.push(parley::StyleProperty::Locale(Some(locale)), range.clone());
    }
    if let Some(v) = overrides.underline {
        builder.push(parley::StyleProperty::Underline(v), range.clone());
    }
    if let Some(v) = overrides.underline_color {
        builder.push(
            parley::StyleProperty::UnderlineBrush(v.map(RichTextBrush)),
            range.clone(),
        );
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
        builder.push(
            parley::StyleProperty::StrikethroughBrush(v.map(RichTextBrush)),
            range.clone(),
        );
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
        builder.push_default(parley::StyleProperty::FontVariations(
            variations.as_str().into(),
        ));
    }
    if let Some(features) = &style.font_features {
        builder.push_default(parley::StyleProperty::FontFeatures(
            features.as_str().into(),
        ));
    }
    if let Some(locale) = style
        .locale
        .as_deref()
        .and_then(|s| parley::Language::parse(s).ok())
    {
        builder.push_default(parley::StyleProperty::Locale(Some(locale)));
    }
    builder.push_default(parley::StyleProperty::Underline(style.underline.enabled));
    builder.push_default(parley::StyleProperty::UnderlineBrush(
        style.underline.color.map(RichTextBrush),
    ));
    builder.push_default(parley::StyleProperty::UnderlineOffset(
        style.underline.offset,
    ));
    builder.push_default(parley::StyleProperty::UnderlineSize(style.underline.size));
    builder.push_default(parley::StyleProperty::Strikethrough(
        style.strikethrough.enabled,
    ));
    builder.push_default(parley::StyleProperty::StrikethroughBrush(
        style.strikethrough.color.map(RichTextBrush),
    ));
    builder.push_default(parley::StyleProperty::StrikethroughOffset(
        style.strikethrough.offset,
    ));
    builder.push_default(parley::StyleProperty::StrikethroughSize(
        style.strikethrough.size,
    ));

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

/// Emit a shaped parley layout through the frame writer: one stencil-masked
/// quad per glyph, plus any underline/strikethrough the run carries.
///
/// Shared by [`RichText`] and [`crate::TextBox`]: parley hands both of them the
/// same `Layout` type (`PlainEditor::layout()` returns one too), so the drawing
/// pass is identical and there is no reason to grow a second copy of it.
pub(crate) fn draw_parley_layout(
    draw: &mut Draw<'_>,
    font_ctx: &ParleyFontCtx,
    ctx: &RenderCtx,
    layout: &parley::Layout<RichTextBrush>,
    tints: &crate::shape::ShapeCtx,
    glyphs: &mut ActiveGlyphs,
) {
    let tint_for = |color| tints.tint_source(color, ctx);

    font_ctx.begin_glyph_batch();
    glyphs.begin();
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

            let Some(font_ref) =
                swash::FontRef::from_index(font.data.as_ref(), font.index as usize)
            else {
                continue;
            };
            let font_size_bits = (font_size_px * SUB_PIXEL_QUANTIZE).round() as u32;
            let coords_hash = fxhash::hash64(coords);

            // Each `GlyphRun` carries one resolved style (parley starts a
            // new run wherever a style — including brush — changes), so
            // the run's colour is already fully resolved: no manual
            // span/byte-range lookup needed here.
            let Some(tint_source) = tint_for(glyph_run.style().brush.0) else {
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

                let Some((glyph_source, size, placement)) =
                    glyphs.glyph_source(font_ctx, key, || {
                        // Creating a scaler can allocate and initialize font programs.
                        // Warm draw writers only need the existing glyph definition.
                        let mut scaler = scale_cx
                            .builder(font_ref)
                            .size(font_size_px)
                            .hint(true)
                            .normalized_coords(coords)
                            .build();
                        rasterize_bitmap(glyph.id as swash::GlyphId, &mut scaler)
                    })
                else {
                    continue;
                };

                let px = gx.floor() + placement[0] as f32;
                let py = gy.floor() - placement[1] as f32;
                let transform = Matrix4::new_translation(&Vector3::new(px, py, 0.0));
                matcha_ecs::scene::push_quad(
                    draw,
                    &tint_source,
                    size,
                    transform,
                    Some(&glyph_source),
                );
            }

            // Underline/strikethrough: a flat filled rectangle, not a
            // glyph — no PixelMask needed.
            // `y = baseline - offset` matches parley's own reference
            // renderers (e.g. `examples/swash_render` in the parley
            // repo) exactly.
            let run_metrics = run.metrics();
            let run_style = glyph_run.style();
            for (decoration, default_offset, default_size) in [
                (
                    &run_style.underline,
                    run_metrics.underline_offset,
                    run_metrics.underline_size,
                ),
                (
                    &run_style.strikethrough,
                    run_metrics.strikethrough_offset,
                    run_metrics.strikethrough_size,
                ),
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
                let deco_transform =
                    Matrix4::new_translation(&Vector3::new(glyph_run.offset(), y, 0.0));
                matcha_ecs::scene::push_quad(
                    draw,
                    &deco_tint,
                    [glyph_run.advance(), size],
                    deco_transform,
                    None,
                );
            }
        }
    }
    glyphs.finish();
}

/// Build a writer that retains a shaped layout keyed by the
/// live wrap width from `wrap_width`.
fn rich_text_render_item(
    font_ctx: ParleyFontCtx,
    wrap_width: Arc<LiveF32>,
    content: RichTextContent,
    style: RichTextStyle,
) -> RenderItem {
    let cached = Mutex::new(None);
    let glyphs = Mutex::new(ActiveGlyphs::default());
    // Declared span colors live with this writer, not forever in the font context.
    let tints = crate::shape::ShapeCtx::default();
    RenderItem::new(move |ctx: &RenderCtx, draw| {
        if content.text.is_empty() {
            return;
        }

        let max_width = wrap_width.get();
        let mut cached = cached.lock();
        if cached.as_ref().is_none_or(|(width, _)| *width != max_width) {
            *cached = Some((max_width, shape(&font_ctx, &content, &style, max_width)));
        }
        draw_parley_layout(
            draw,
            &font_ctx,
            ctx,
            &cached.as_ref().expect("shaped width").1,
            &tints,
            &mut glyphs.lock(),
        )
    })
}

impl Layout for RichTextStyle {
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, constraints: Constraints) -> Measured {
        let sizing = Sizing::of(ctx, me);
        let inner = sizing.content_constraints(constraints);

        let Some(font_ctx) = ctx.world().get_resource::<ParleyFontCtx>() else {
            return Measured::exact([0.0, 0.0]);
        };
        let Some(content) = ctx.world().get::<RichTextContent>(me) else {
            return Measured::exact([0.0, 0.0]);
        };
        if content.text.is_empty() {
            return Measured::exact([0.0, 0.0]);
        }
        let layout = shape(font_ctx, content, self, inner.max_width());

        // CSS min-content / max-content, straight from the layout already
        // built: every soft break taken, and none taken. Deliberately *not*
        // clamped — a contribution is what this text would want, which is the
        // whole point of reporting it apart from the size it settled for.
        //
        // The height is reported as a single value rather than a range: it
        // depends on the width finally chosen, and neither content width is
        // that width (see `Measured`'s docs).
        let widths = layout.calculate_content_widths();
        let shaped = [layout.width(), layout.height()];
        sizing.measured(
            constraints,
            Measured::new(
                [widths.min.min(shaped[0]), shaped[1]],
                shaped,
                [widths.max.max(shaped[0]), shaped[1]],
            ),
        )
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
    sizing: Sizing,
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
            sizing: Sizing::default(),
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
    pub fn span(
        mut self,
        content: impl Into<String>,
        build: impl FnOnce(RichSpan) -> RichSpan,
    ) -> Self {
        let span = build(RichSpan::new(content.into()));
        self.spans.push(PendingSpan {
            text: span.text,
            overrides: span.overrides,
            text_transform: span.text_transform,
        });
        self
    }

    crate::sizing_builders!();

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
            spans: spans
                .into_iter()
                .map(|(range, overrides)| ResolvedSpan { range, overrides })
                .collect(),
        }
    }

    fn rebuild_render_item(&self, entity: &mut EntityWorldMut) -> RenderItem {
        let font_ctx = entity.world_scope(|world| {
            world
                .get_resource_or_insert_with(ParleyFontCtx::new)
                .clone()
        });
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
            self.sizing,
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
        self.sync_sizing(entity);
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
    //! public-API integration tests in `tests/rich_text.rs`, its
    //! write-through from `RichTextStyle::arrange` can only be checked from
    //! inside this crate. Mirrors `text.rs`'s identical unit test.
    use bevy_ecs::world::World;
    use matcha_ecs::{
        components::view::ViewChildren,
        layout::{Constraints, layout_root},
        view::run_view,
    };

    use super::*;

    fn synthetic_glyph_key(index: u32) -> GlyphKey {
        GlyphKey {
            font_blob_id: 1,
            font_index: 0,
            glyph_id: index,
            font_size_bits: 16 * SUB_PIXEL_QUANTIZE as u32,
            coords_hash: 0,
        }
    }

    fn synthetic_glyph() -> GlyphDefinition {
        Some((
            MaskSource::new(
                MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
                |_| panic!("CPU cache tests never prepare GPU content"),
            ),
            [1., 1.],
            [0, 0],
        ))
    }

    #[test]
    fn active_glyphs_keep_overflow_definitions_and_prune_obsolete_layout_keys() {
        let fonts = ParleyFontCtx::new();
        let mut active = ActiveGlyphs::default();
        let count = GLYPH_CACHE_CAPACITY + 3;
        let mut first_ids = Vec::new();
        fonts.begin_glyph_batch();
        active.begin();
        for index in 0..count {
            let source = active
                .glyph_source(&fonts, synthetic_glyph_key(index as u32), synthetic_glyph)
                .expect("LRU capacity must not hide visible glyphs");
            first_ids.push(source.0.id());
        }
        active.finish();
        assert_eq!(fonts.0.stencil_cache.lock().len(), GLYPH_CACHE_CAPACITY);
        assert_eq!(active.entries.len(), count);

        fonts.begin_glyph_batch();
        active.begin();
        for (index, expected) in first_ids.iter().enumerate() {
            let source = active
                .glyph_source(&fonts, synthetic_glyph_key(index as u32), || {
                    panic!("warm active glyphs must not rasterize again")
                })
                .expect("warm visible definition");
            assert_eq!(source.0.id(), *expected);
        }
        active.finish();

        // An editor changes to a layout containing only one former overflow
        // glyph and an invisible glyph. Both survive; old layout keys do not.
        let survivor = synthetic_glyph_key((count - 1) as u32);
        let invisible = synthetic_glyph_key(count as u32);
        active.begin();
        let source = active
            .glyph_source(&fonts, survivor, || panic!("retained overflow definition"))
            .expect("overflow definition retained");
        assert_eq!(source.0.id(), first_ids[count - 1]);
        assert!(active.glyph_source(&fonts, invisible, || None).is_none());
        active.finish();
        assert_eq!(active.entries.len(), 2);

        active.begin();
        assert!(
            active
                .glyph_source(&fonts, invisible, || panic!("invisible glyph cached"))
                .is_none()
        );
        active.finish();
        assert_eq!(active.entries.len(), 1);
        active.clear();
        assert!(active.entries.is_empty());
    }

    #[test]
    fn independent_writers_do_not_thrash_the_shared_lru_on_warm_frames() {
        let fonts = ParleyFontCtx::new();
        let mut writers = [ActiveGlyphs::default(), ActiveGlyphs::default()];
        let per_writer = GLYPH_CACHE_CAPACITY / 2 + 64;
        let built = std::cell::Cell::new(0);
        let mut first_ids = Vec::new();
        for frame in 0..3 {
            let mut ids = Vec::new();
            for (writer_index, writer) in writers.iter_mut().enumerate() {
                fonts.begin_glyph_batch();
                writer.begin();
                for index in 0..per_writer {
                    let key = synthetic_glyph_key((writer_index * per_writer + index) as u32);
                    let definition = writer
                        .glyph_source(&fonts, key, || {
                            built.set(built.get() + 1);
                            synthetic_glyph()
                        })
                        .expect("visible glyph");
                    ids.push(definition.0.id());
                }
                writer.finish();
            }
            if frame == 0 {
                first_ids = ids;
            } else {
                assert_eq!(
                    ids, first_ids,
                    "each writer retains the current layout definitions"
                );
            }
            assert_eq!(
                built.get(),
                2 * per_writer,
                "rasterize only the first frame"
            );
        }
    }

    #[test]
    fn arrange_writes_its_resolved_width_into_rich_text_wrap_width() {
        let mut world = World::new();
        let root = world.spawn(ViewChildren::default()).id();
        run_view(&mut world, root, |s| {
            s.leaf(RichText::new("hi").font_size(16.0));
        });
        layout_root(&mut world, root, Constraints::from_max_size([123.0, 456.0]));

        let child = world.get::<ViewChildren>(root).unwrap().slots[0].1;
        let stored_width = world.get::<RichTextWrapWidth>(child).unwrap().0.get();

        let out = world
            .get::<matcha_ecs::components::layout::LayoutOutput>(child)
            .unwrap();
        assert_eq!(
            stored_width, out.size[0],
            "RichTextWrapWidth must hold exactly the width arrange() resolved this entity to"
        );
    }

    #[test]
    fn apply_text_transform_uppercase_lowercase_capitalize() {
        assert_eq!(
            apply_text_transform("Hello World", TextTransform::None),
            "Hello World"
        );
        assert_eq!(
            apply_text_transform("Hello World", TextTransform::Uppercase),
            "HELLO WORLD"
        );
        assert_eq!(
            apply_text_transform("Hello World", TextTransform::Lowercase),
            "hello world"
        );
        assert_eq!(
            apply_text_transform("hello   world", TextTransform::Capitalize),
            "Hello   World"
        );
        // Non-ASCII: uppercasing an accented character must not just no-op.
        assert_eq!(
            apply_text_transform("café", TextTransform::Uppercase),
            "CAFÉ"
        );
        assert_eq!(
            apply_text_transform("CAFÉ", TextTransform::Lowercase),
            "café"
        );
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
        assert!(
            a_range.end <= b_range.start,
            "spans must not overlap after remapping"
        );
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
        let (collapsed, remapped) = collapse_white_space_with_span_remap(
            text,
            &[(outer, "outer"), (inner_tail_of_run, "inner")],
        );

        assert_eq!(collapsed, "a b");
        let (outer_range, _) = &remapped[0];
        let (inner_range, _) = &remapped[1];
        assert_eq!(&collapsed[outer_range.clone()], "a b");
        assert!(
            inner_range.is_empty(),
            "a span covering only already-collapsed whitespace must remap to empty"
        );
    }
}
