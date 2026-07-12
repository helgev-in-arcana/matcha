//! `Text` — a leaf widget rendering a shaped, word-wrapped string (M6).
//!
//! Text shaping/rasterisation is powered by the sibling `suzuri` crate. Only
//! its CPU-facing pieces are used: `FontSystem::layout_text` (pure geometry —
//! line wrap, kerning, alignment) for shaping, and `fontdue::Font::rasterize_indexed`
//! (re-exported through suzuri) directly for per-glyph coverage bitmaps.
//! Suzuri's own `CpuRenderer`/`GpuRenderer`/`WgpuRenderer` are deliberately not
//! used: they composite a whole laid-out block into one destination in a
//! single call, which doesn't fit the per-glyph, cross-frame, cross-widget
//! atlas cache this widget needs.
//!
//! Rendering reuses the `renderer` crate's existing (previously unused by any
//! matcha-ecs widget) stencil-masking pipeline: a glyph is drawn as a small
//! solid-colour "tint" quad (`texture_atlas`) masked by a per-glyph coverage
//! bitmap (`stencil_atlas`, `R8Unorm` — the fragment shader already does
//! `final_color = texture_color * stencil_atlas.r`, see
//! `renderer/src/core_renderer/renderer_render.wgsl`).
//!
//! Word-wrap is supported, but deliberately with no shape-result caching:
//! `measure()`, `arrange()`, and the `RenderItem` builder each independently
//! (re)run `FontSystem::layout_text` from scratch. The only value threaded
//! between layout and render is the resolved wrap width (a single `f32`,
//! shared via `TextWrapWidth`'s `Arc<AtomicU32>`) — passing the actual shaped
//! glyph list between stages is left as a future optimisation.

use std::{
    collections::HashMap,
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
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch},
    view::Widget,
};

use crate::animation::{Easing, ExitFade, OpacityTween};

/// The displayed string.
#[derive(Component, Clone, PartialEq, Eq, Debug)]
pub struct TextContent(pub String);

/// Draw-relevant text properties other than the content itself.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
struct TextStyle {
    font_size: f32,
    color: [f32; 4],
}

/// Shares the most recently resolved wrap width between `TextStyle::arrange`
/// (writer, every layout pass) and the `RenderItem` builder (reader, every
/// rebuild). Deliberately not part of `TextStyle`'s `PartialEq`/`Clone`-based
/// change comparison in `patch` — `AtomicU32` has no meaningful `PartialEq`,
/// and this cell must survive being read from a system that never replaces
/// it, only the entity's `TextStyle`/`RenderItem` are replaced on patch.
///
/// Initialised to `f32::MAX` so a `RenderItem` rebuild that (implausibly)
/// runs before this entity's first `arrange()` still degrades safely to
/// "effectively no wrap" rather than wrapping after every glyph.
#[derive(Component)]
struct TextWrapWidth(Arc<AtomicU32>);

impl TextWrapWidth {
    fn new() -> Self {
        Self(Arc::new(AtomicU32::new(f32::MAX.to_bits())))
    }

    fn store(&self, width: f32) {
        self.0.store(width.to_bits(), Ordering::Relaxed);
    }
}

struct FontCtxInner {
    font_system: suzuri::FontSystem,
    /// Per-glyph rasterised coverage bitmap, cached in the stencil atlas and
    /// shared across every `Text` entity/frame that draws the same glyph at
    /// the same quantized size (`suzuri::GlyphId` bundles font+glyph+size).
    stencil_cache: Mutex<HashMap<suzuri::GlyphId, (AtlasRegion, [f32; 2]), fxhash::FxBuildHasher>>,
}

/// World resource wrapping the shared `suzuri::FontSystem` plus the glyph
/// stencil cache. Lazily inserted on first use (`world_scope` +
/// `get_resource_or_insert_with`) so core (`UiEcs`) never has to know about it
/// — matches `ECS_IMPLEMENTATION_PLAN.md`'s pre-existing plan for `FontCtx`.
/// Cheap to `Clone` (an `Arc` handle), so it can be captured directly into a
/// `RenderItem`'s `Send + Sync` builder closure.
#[derive(Resource, Clone)]
pub(crate) struct FontCtx(Arc<FontCtxInner>);

impl FontCtx {
    pub(crate) fn new() -> Self {
        let font_system = suzuri::FontSystem::new();
        font_system.load_system_fonts();
        Self(Arc::new(FontCtxInner {
            font_system,
            stencil_cache: Mutex::new(HashMap::default()),
        }))
    }

    /// Look up (or rasterise-and-cache) the stencil atlas region holding
    /// `glyph_id`'s coverage bitmap, plus its pixel size. Returns `None` for
    /// glyphs with no visible bitmap (e.g. space) or on allocation failure.
    pub(crate) fn stencil_region(
        &self,
        glyph_id: suzuri::GlyphId,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &gpu_utils::texture_atlas::TextureAtlas,
    ) -> Option<(AtlasRegion, [f32; 2])> {
        if let Some(cached) = self.0.stencil_cache.lock().get(&glyph_id) {
            return Some(cached.clone());
        }

        let font = self.0.font_system.font(glyph_id.font_id())?;
        let (metrics, bitmap) =
            font.rasterize_indexed(glyph_id.glyph_index(), glyph_id.font_size());
        if metrics.width == 0 || metrics.height == 0 {
            return None;
        }

        let region = match atlas.allocate(device, queue, [metrics.width as u32, metrics.height as u32])
        {
            Ok(region) => region,
            Err(e) => {
                log::error!("Text glyph stencil allocation failed: {e}");
                return None;
            }
        };
        if let Err(e) = region.write_data(queue, &bitmap) {
            log::error!("Text glyph stencil upload failed: {e}");
            return None;
        }

        let entry = (region, [metrics.width as f32, metrics.height as f32]);
        self.0
            .stencil_cache
            .lock()
            .insert(glyph_id, entry.clone());
        Some(entry)
    }
}

/// Shape `content` fresh (no caching — see module docs) at `font_size`,
/// word-wrapping at `max_width`. Returns an empty layout if no matching font
/// is found rather than panicking.
pub(crate) fn shape(font_ctx: &FontCtx, content: &str, font_size: f32, max_width: f32) -> suzuri::text::TextLayout<()> {
    let mut data = suzuri::text::TextData::<()>::new();
    if let Some((font_id, _font)) = font_ctx.0.font_system.query(&suzuri::fontdb::Query {
        families: &[suzuri::fontdb::Family::SansSerif],
        ..Default::default()
    }) {
        data.append(suzuri::text::TextElement {
            font_id,
            font_size,
            content: content.to_string(),
            user_data: (),
        });
    }
    let config = suzuri::text::TextLayoutConfig {
        wrap_style: suzuri::text::WrapStyle::WordWrap,
        max_width: Some(max_width),
        ..Default::default()
    };
    font_ctx.0.font_system.layout_text(&data, &config)
}

/// Gamma-encode a linear colour component into the sRGB space the atlas
/// texture is stored in (matches what a render pass writing to an
/// `Rgba8UnormSrgb` target does automatically; `write_data` is a raw byte
/// copy with no such conversion, so it must be done by hand here).
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
/// (UV-clamped to any on-screen size) as every glyph's stencil "tint".
///
/// Written directly via `write_data` rather than `ColorRect`'s render-pass +
/// `VertexColor` approach: a real GPU render pass whose viewport is scoped to
/// a 1x1 (or otherwise very small, single-digit-pixel) atlas region was found
/// to rasterise incorrectly (a soft, mispositioned blob instead of a flat
/// fill — reproduced in isolation with a hand-built 4x4 case, unrelated to
/// glyphs/stencils). `ColorRect` never hits this because it always sizes
/// regions to its own (typically much larger) rect, so the bug went
/// unnoticed; text's single shared tint pixel triggers it directly. Root
/// cause not identified (deferred — see `ECS_IMPLEMENTATION_PLAN.md` §8);
/// `write_data` sidesteps it entirely and is a strictly simpler upload for a
/// flat fill anyway.
pub(crate) fn paint_tint_region(ctx: &RenderCtx, color: [f32; 4]) -> Option<AtlasRegion> {
    let region = match ctx.texture_atlas.allocate(ctx.device, ctx.queue, [1, 1]) {
        Ok(region) => region,
        Err(e) => {
            log::error!("Text tint region allocation failed: {e}");
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
        log::error!("Text tint upload failed: {e}");
        return None;
    }

    Some(region)
}

/// Composite `layout`'s glyphs into `(node, local_translation)` pairs, each a
/// tint-texture quad masked by its cached stencil coverage bitmap, tinted
/// uniformly by `tint_region` (see `paint_tint_region`). Shared by `Text`'s
/// own render item and by any other widget (e.g. `Button`'s label) that needs
/// to draw a shaped single-style glyph run without duplicating the
/// suzuri-shaping/stencil-cache glue.
pub(crate) fn glyph_run_nodes(
    font_ctx: &FontCtx,
    ctx: &RenderCtx,
    layout: &suzuri::text::TextLayout<()>,
    tint_region: &AtlasRegion,
) -> Vec<(RenderNode, Matrix4<f32>)> {
    let mut out = Vec::new();
    for line in &layout.lines {
        for glyph in &line.glyphs {
            let Some((stencil_region, size)) =
                font_ctx.stencil_region(glyph.glyph_id, ctx.device, ctx.queue, ctx.stencil_atlas)
            else {
                continue;
            };
            let transform = Matrix4::new_translation(&Vector3::new(glyph.x, glyph.y, 0.0));
            let glyph_node = RenderNode::new()
                .with_texture(tint_region.clone(), size, Matrix4::identity())
                .with_stencil(stencil_region, size, Matrix4::identity());
            out.push((glyph_node, transform));
        }
    }
    out
}

/// Build a `RenderItem` that shapes `content` fresh every rebuild (reading
/// the live wrap width from `wrap_width`) and draws each glyph as a
/// tint-texture quad masked by its cached stencil coverage bitmap.
fn text_render_item(
    font_ctx: FontCtx,
    wrap_width: Arc<AtomicU32>,
    content: String,
    font_size: f32,
    color: [f32; 4],
) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let mut node = RenderNode::new();

        let max_width = f32::from_bits(wrap_width.load(Ordering::Relaxed));
        let layout = shape(&font_ctx, &content, font_size, max_width);

        let Some(tint_region) = paint_tint_region(ctx, color) else {
            return node;
        };

        for (glyph_node, transform) in glyph_run_nodes(&font_ctx, ctx, &layout, &tint_region) {
            node.push_child(glyph_node, transform);
        }

        node
    })
}

impl Layout for TextStyle {
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, constraints: Constraints) -> [f32; 2] {
        let Some(font_ctx) = ctx.world().get_resource::<FontCtx>() else {
            return [0.0, 0.0];
        };
        let Some(content) = ctx.world().get::<TextContent>(me) else {
            return [0.0, 0.0];
        };
        let layout = shape(font_ctx, &content.0, self.font_size, constraints.max_width());
        [
            layout.total_width.clamp(constraints.min_width(), constraints.max_width()),
            layout.total_height.clamp(constraints.min_height(), constraints.max_height()),
        ]
    }

    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, size: [f32; 2]) {
        // Leaf: no children. Just record the effective width `measure()` just
        // used, for the `RenderItem` builder to reshape against later.
        if let Some(wrap_width) = ctx.world().get::<TextWrapWidth>(me) {
            wrap_width.store(size[0]);
        }
    }
}

/// A word-wrapped text block of fixed style, sized to its shaped content.
pub struct Text {
    key: Key,
    content: String,
    font_size: f32,
    color: [f32; 4],
    /// See `ColorRect`'s identical fields for the M7 fade design.
    enter_fade: Option<(Duration, Easing)>,
    exit_fade: Option<(Duration, Easing)>,
}

impl Text {
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

    fn style(&self) -> TextStyle {
        TextStyle {
            font_size: self.font_size,
            color: self.color,
        }
    }

    /// Build a fresh `RenderItem` for `entity`, fetching (or lazily
    /// inserting) `FontCtx` and reusing `entity`'s existing `TextWrapWidth`
    /// cell. Shared by `after_spawn` and `patch`, the two places a `Text`
    /// entity's `RenderItem` gets (re)built.
    fn rebuild_render_item(&self, entity: &mut EntityWorldMut) -> RenderItem {
        let font_ctx = entity.world_scope(|world| world.get_resource_or_insert_with(FontCtx::new).clone());
        let wrap_width = entity
            .get::<TextWrapWidth>()
            .expect("bundle() inserted TextWrapWidth")
            .0
            .clone();
        text_render_item(font_ctx, wrap_width, self.content.clone(), self.font_size, self.color)
    }
}

impl Widget for Text {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        // Unlike `ColorRect`, the `RenderItem` can't be built here: it needs
        // the `FontCtx` resource, and `bundle()` has no world access at all
        // (it just returns a plain `Bundle` value). Built in `after_spawn`
        // instead, which does get `&mut EntityWorldMut`.
        let initial_opacity = if self.enter_fade.is_some() { 0.0 } else { 1.0 };
        (
            TextContent(self.content.clone()),
            self.style(),
            TextWrapWidth::new(),
            LayoutDispatch::of::<TextStyle>(),
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
        if let Some(mut c) = entity.get_mut::<TextContent>() {
            changed |= c.set_if_neq(TextContent(self.content.clone()));
        }
        if let Some(mut s) = entity.get_mut::<TextStyle>() {
            changed |= s.set_if_neq(self.style());
        }
        if changed {
            let item = self.rebuild_render_item(entity);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }

        // Revival: see `ColorRect::patch` for the identical reasoning.
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
    //! `TextWrapWidth` is a private implementation detail (the sole value
    //! threaded from layout to render, per the module docs), so unlike the
    //! public-API integration tests in `matcha-ecs/tests/text.rs`, its
    //! write-through from `TextStyle::arrange` can only be checked from
    //! inside this crate.
    use bevy_ecs::world::World;
    use matcha_ecs::{
        components::view::ViewChildren,
        layout::{layout_root, Constraints},
        view::run_view,
    };

    use super::*;

    #[test]
    fn arrange_writes_its_resolved_width_into_text_wrap_width() {
        let mut world = World::new();
        let root = world.spawn(ViewChildren::default()).id();
        run_view(&mut world, root, |s| {
            s.leaf(Text::new("hi").font_size(16.0));
        });
        layout_root(&mut world, root, Constraints::from_max_size([123.0, 456.0]));

        let child = world.get::<ViewChildren>(root).unwrap().slots[0].1;
        let stored_width = world.get::<TextWrapWidth>(child).unwrap().0.load(Ordering::Relaxed);
        let stored_width = f32::from_bits(stored_width);

        let out = world.get::<matcha_ecs::components::layout::LayoutOutput>(child).unwrap();
        assert_eq!(
            stored_width, out.size[0],
            "TextWrapWidth must hold exactly the width arrange() resolved this entity to"
        );
    }
}
