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
//! Deliberately out of scope for v1 (see `ECS_IMPLEMENTATION_PLAN.md`'s
//! RichText milestone notes): parley's `Brush` type (per-span colour/rich
//! text), underline/strikethrough, colour glyphs (emoji), sub-pixel glyph
//! positioning (layout is quantized to whole pixels instead), and any
//! shape-result caching (matching `Text`: `measure`/`arrange`/the
//! `RenderItem` builder each independently re-shape from scratch).

use std::{
    num::NonZeroUsize,
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
    animation::{Animated, Easing, ExitTransition, Opacity, Target, ToBeDespawn, Tween},
    components::{
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, SUB_PIXEL_QUANTIZE},
    view::Widget,
};

/// The displayed string.
#[derive(Component, Clone, PartialEq, Eq, Debug)]
pub struct RichTextContent(pub String);

/// Draw-relevant text properties other than the content itself.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
struct RichTextStyle {
    font_size: f32,
    color: [f32; 4],
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

struct ParleyFontCtxInner {
    font_cx: Mutex<parley::FontContext>,
    layout_cx: Mutex<parley::LayoutContext<()>>,
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

/// Shape `content` fresh (no caching — see module docs) at `font_size`,
/// word-wrapping at `max_width`.
fn shape(font_ctx: &ParleyFontCtx, content: &str, font_size: f32, max_width: f32) -> parley::Layout<()> {
    let inner = &font_ctx.0;
    let mut fcx = inner.font_cx.lock();
    let mut lcx = inner.layout_cx.lock();

    let mut builder = lcx.ranged_builder(&mut fcx, content, 1.0, true);
    builder.push_default(parley::StyleProperty::FontFamily(parley::FontFamily::from("system-ui")));
    builder.push_default(parley::StyleProperty::FontSize(font_size));
    builder.push_default(parley::LineHeight::FontSizeRelative(1.3));

    let mut layout: parley::Layout<()> = builder.build(content);
    layout.break_all_lines(Some(max_width));
    layout.align(parley::Alignment::Start, parley::AlignmentOptions::default());
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
    content: String,
    font_size: f32,
    color: [f32; 4],
) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let mut node = RenderNode::new();
        if content.is_empty() {
            return node;
        }

        let max_width = f32::from_bits(wrap_width.load(Ordering::Relaxed));
        let layout = shape(&font_ctx, &content, font_size, max_width);

        let Some(tint_region) = paint_tint_region(ctx, color) else {
            return node;
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
        if content.0.is_empty() {
            return [0.0, 0.0];
        }
        let layout = shape(font_ctx, &content.0, self.font_size, constraints.max_width());
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
            enter_fade: None,
            exit_fade: None,
        }
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
        }
    }

    fn rebuild_render_item(&self, entity: &mut EntityWorldMut) -> RenderItem {
        let font_ctx = entity.world_scope(|world| world.get_resource_or_insert_with(ParleyFontCtx::new).clone());
        let wrap_width = entity
            .get::<RichTextWrapWidth>()
            .expect("bundle() inserted RichTextWrapWidth")
            .0
            .clone();
        rich_text_render_item(font_ctx, wrap_width, self.content.clone(), self.font_size, self.color)
    }
}

impl Widget for RichText {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        let initial_opacity = if self.enter_fade.is_some() { 0.0 } else { 1.0 };
        (
            RichTextContent(self.content.clone()),
            self.style(),
            RichTextWrapWidth::new(),
            LayoutDispatch::of::<RichTextStyle>(),
            Target(Opacity(1.0)),
            Animated(Opacity(initial_opacity)),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        let item = self.rebuild_render_item(entity);
        entity.insert(item);

        if let Some((duration, easing)) = self.enter_fade {
            entity.insert(Tween::<Opacity> {
                from: Opacity(0.0),
                start: web_time::Instant::now(),
                duration,
                easing,
            });
        }
        if let Some((duration, easing)) = self.exit_fade {
            entity.insert(ExitTransition::<Opacity> {
                to: Opacity(0.0),
                duration,
                easing,
            });
        }
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        let mut changed = false;
        if let Some(mut c) = entity.get_mut::<RichTextContent>() {
            changed |= c.set_if_neq(RichTextContent(self.content.clone()));
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

        // Revival (M7 pattern): see `Text::patch`/`ColorRect::patch` for the
        // identical reasoning.
        if entity.get::<ToBeDespawn>().is_some() {
            if let Some(exit) = entity.get::<ExitTransition<Opacity>>().copied() {
                let current = entity
                    .get::<Animated<Opacity>>()
                    .copied()
                    .unwrap_or(Animated(Opacity(1.0)));
                entity.insert((
                    Target(Opacity(1.0)),
                    Tween::<Opacity> {
                        from: current.0,
                        start: web_time::Instant::now(),
                        duration: exit.duration,
                        easing: exit.easing,
                    },
                ));
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
}
