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
//! shared via `TextWrapWidth`'s `Arc<LiveF32>`) — passing the actual shaped
//! glyph list between stages is left as a future optimisation.

use std::{
    collections::HashMap,
    sync::Arc,
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
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
    view::Widget,
};

use crate::live::LiveF32;
use crate::sizing::Sizing;
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
/// change comparison in `patch` — `LiveF32` has no meaningful `PartialEq`,
/// and this cell must survive being read from a system that never replaces
/// it, only the entity's `TextStyle`/`RenderItem` are replaced on patch.
///
/// Initialised to `f32::MAX` so a `RenderItem` rebuild that (implausibly)
/// runs before this entity's first `arrange()` still degrades safely to
/// "effectively no wrap" rather than wrapping after every glyph.
#[derive(Component)]
struct TextWrapWidth(Arc<LiveF32>);

impl TextWrapWidth {
    fn new() -> Self {
        Self(Arc::new(LiveF32::new(f32::MAX)))
    }

    fn store(&self, width: f32) {
        self.0.set(width);
    }
}

struct FontCtxInner {
    font_system: suzuri::FontSystem,
    /// Per-glyph rasterised coverage bitmap, cached in the stencil atlas and
    /// shared across every `Text` entity/frame that draws the same glyph at
    /// the same quantized size (`suzuri::GlyphId` bundles font+glyph+size).
    stencil_cache: Mutex<HashMap<suzuri::GlyphId, (AtlasRegion, [f32; 2]), fxhash::FxBuildHasher>>,
    /// Fonts registered so far, and the deduplication key for
    /// [`FontCtx::ensure_registered`]. The `Arc`s are kept alive deliberately —
    /// see `RichText`'s `ParleyFontCtxInner::registered_fonts` for why identity
    /// has to outlive the comparison.
    registered_fonts: Mutex<Vec<Arc<Vec<u8>>>>,
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
        #[cfg(not(web))]
        font_system.load_system_fonts();
        Self(Arc::new(FontCtxInner {
            font_system,
            stencil_cache: Mutex::new(HashMap::default()),
            registered_fonts: Mutex::new(Vec::new()),
        }))
    }

    /// Register `data` with suzuri's font system, unless the very same `Arc`
    /// was registered before, and make it the sans-serif family — which is
    /// what [`shape`] queries for, so a font registered but not mapped still
    /// draws nothing.
    ///
    /// # Why this exists
    ///
    /// A browser exposes no font database to enumerate: `load_system_fonts`
    /// goes through `fontdb`, whose implementation is a series of
    /// `#[cfg(target_os = ...)]` blocks with no arm matching wasm, so it is a
    /// no-op there. `shape` then queries `Family::SansSerif`, misses, and
    /// returns an empty layout — not an error, just a widget that measures
    /// 0x0 and draws nothing. `Button` draws its label through this same
    /// context, so without a registered font every button is blank too.
    ///
    /// Only the **first** font registered becomes sans-serif; later ones are
    /// available for lookup but do not displace it.
    pub(crate) fn ensure_registered(&self, data: &Arc<Vec<u8>>) {
        let first;
        {
            let mut registered = self.0.registered_fonts.lock();
            if registered.iter().any(|font| Arc::ptr_eq(font, data)) {
                return;
            }
            registered.push(data.clone());
            first = registered.len() == 1;
        }

        // Faces this load appends start here. Indexing rather than
        // `faces().first()` because on native `load_system_fonts` has already
        // filled the database, so the first face is some system font, not the
        // one being registered now.
        let face_base = self.0.font_system.faces().len();
        self.0.font_system.load_font_binary(data.to_vec());

        if !first {
            // Available for lookup, but it does not displace sans-serif.
            return;
        }

        // Take the family name from the face the load produced rather than
        // hardcoding one: swapping in a subset (or a different font) should
        // not require editing a string here to match.
        match self
            .0
            .font_system
            .faces()
            .get(face_base)
            .and_then(|face| face.families.first())
            .cloned()
        {
            Some((name, _)) => self.0.font_system.set_sans_serif_family(name),
            None => log::error!("the registered font produced no faces; text will not render"),
        }
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

/// Paint the 1x1 tint pixel every glyph's stencil is masked against.
pub(crate) fn paint_tint_region(ctx: &RenderCtx, color: [f32; 4]) -> Option<AtlasRegion> {
    crate::color::paint_tint_region(ctx, color, "Text")
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
    wrap_width: Arc<LiveF32>,
    content: String,
    font_size: f32,
    color: [f32; 4],
) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let mut node = RenderNode::new();

        let max_width = wrap_width.get();
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
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, constraints: Constraints) -> Measured {
        let sizing = Sizing::of(ctx, me);
        let inner = sizing.content_constraints(constraints);

        let Some(font_ctx) = ctx.world().get_resource::<FontCtx>() else {
            return Measured::exact([0.0, 0.0]);
        };
        let Some(content) = ctx.world().get::<TextContent>(me) else {
            return Measured::exact([0.0, 0.0]);
        };
        let layout = shape(font_ctx, &content.0, self.font_size, inner.max_width());

        // Reports no width range, unlike `RichText`. parley hands that widget
        // its min/max-content widths off the layout it already built, whereas
        // suzuri/fontdue has no such API: deriving the pair here would mean
        // two extra full shaping passes per measure, on the widget that has
        // no shape cache at all and is kept as the reference/fallback
        // implementation. A `Text` in a shrinking row therefore will not go
        // below the width it wrapped to; `RichText` will.
        let shaped = [layout.total_width, layout.total_height];
        sizing.measured(constraints, Measured::exact(shaped))
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
    sizing: Sizing,
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
            sizing: Sizing::default(),
            content: content.into(),
            font_size: 16.0,
            color: [0.0, 0.0, 0.0, 1.0],
            enter_fade: None,
            exit_fade: None,
        }
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
            self.sizing,
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
        self.sync_sizing(entity);
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
        let stored_width = world.get::<TextWrapWidth>(child).unwrap().0.get();

        let out = world.get::<matcha_ecs::components::layout::LayoutOutput>(child).unwrap();
        assert_eq!(
            stored_width, out.size[0],
            "TextWrapWidth must hold exactly the width arrange() resolved this entity to"
        );
    }
}
