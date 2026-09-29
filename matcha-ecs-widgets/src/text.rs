//! `Text` — a leaf widget rendering a shaped, word-wrapped string.
//!
//! Text shaping/rasterisation is powered by the sibling `suzuri` crate. Only
//! its CPU-facing pieces are used: `FontSystem::layout_text` (pure geometry —
//! line wrap, kerning, alignment) for shaping, and `fontdue::Font::rasterize_indexed`
//! (re-exported through suzuri) directly for per-glyph coverage bitmaps.
//! Suzuri's own `CpuRenderer`/`GpuRenderer`/`WgpuRenderer` are deliberately not
//! used: they composite a whole laid-out block into one destination in a
//! single call, which doesn't fit the per-glyph, cross-frame, cross-widget
//! resource sharing this widget needs.
//!
//! Rendering writes Objects and PixelMasks into a Scene.
//! Fontdue glyph bounds are queried during Scene construction; rasterization
//! runs inside MaskSource::prepare only when the renderer needs the content.
//! Colours are GPU-generated TextureSources.
//!
//! The render writer retains a shaped layout keyed by resolved wrap width and a
//! stable tint source. Changing declared content/style replaces the writer.
//! Layout measurement shapes independently of the render writer.

use std::{collections::HashMap, sync::Arc, time::Duration};

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    resource::Resource, world::EntityWorldMut,
};
use matcha_ecs::scene::Draw;
use nalgebra::{Matrix4, Vector3};
use parking_lot::Mutex;
use render_interface::{MaskDescriptor, MaskSource, TextureSource, upload_texture};

use matcha_ecs::{
    components::{
        render::{RenderCtx, RenderItem, RenderOpacity},
        view::{Key, ManualDespawn},
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
    view::Widget,
};

use crate::animation::{Easing, ExitFade, OpacityTween};
use crate::font::{FontData, FontRegistry, font_bytes};
use crate::live::LiveF32;
use crate::sizing::Sizing;

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
/// (writer, every layout pass) and the `RenderItem` writer (reader, every
/// redraw). Deliberately not part of `TextStyle`'s `PartialEq`/`Clone`-based
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
    /// Per-glyph immutable coverage definition, shared across every `Text`
    /// entity/frame that draws the same glyph at the same quantized size
    /// (`suzuri::GlyphId` bundles font+glyph+size). GPU residency is independent.
    stencil_cache: Mutex<HashMap<suzuri::GlyphId, (MaskSource, [f32; 2]), fxhash::FxBuildHasher>>,
    /// Which fonts have been handed to `font_system`, and whether one of them
    /// is its sans-serif family. See [`FontRegistry`] for the locking rule.
    registry: Mutex<FontRegistry>,
}

/// World resource wrapping the shared `suzuri::FontSystem` plus the glyph
/// stencil cache. Lazily inserted on first use (`world_scope` +
/// `get_resource_or_insert_with`), keeping font machinery outside the core.
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
            registry: Mutex::new(FontRegistry::default()),
        }))
    }

    /// Register `data` with suzuri's font system, unless the very same handle
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
    /// The first font to register **successfully** becomes sans-serif; later
    /// ones are available for lookup but do not displace it.
    pub(crate) fn ensure_registered(&self, data: &FontData) {
        // Held across the load, not just across the membership check — see
        // `FontRegistry`'s locking note.
        let mut registry = self.0.registry.lock();
        if registry.contains(data) {
            return;
        }

        // Faces this load appends start here. Indexing rather than
        // `faces().first()` because on native `load_system_fonts` has already
        // filled the database, so the first face is some system font, not the
        // one being registered now.
        let face_base = self.0.font_system.faces().len();
        // suzuri takes ownership, so this is the one copy of the font bytes
        // that `FontData` cannot avoid.
        self.0
            .font_system
            .load_font_binary(font_bytes(data).to_vec());

        // Take the family name from the face the load produced rather than
        // hardcoding one: swapping in a subset (or a different font) should
        // not require editing a string here to match.
        let Some((family, _)) = self
            .0
            .font_system
            .faces()
            .get(face_base)
            .and_then(|face| face.families.first())
            .cloned()
        else {
            // Deliberately not recorded: a font that loaded nothing must not
            // consume the default slot, and re-offering it should retry.
            log::error!("the registered font produced no faces; text will not render");
            return;
        };

        registry.record(data);
        if registry.claim_default_slot() {
            self.0.font_system.set_sans_serif_family(family);
        }
    }

    /// Look up the MaskSource defining
    /// `glyph_id`'s coverage bitmap, plus its pixel size. Returns `None` for
    /// glyphs with no visible bitmap (e.g. space) or a missing font.
    pub(crate) fn glyph_source(&self, glyph_id: suzuri::GlyphId) -> Option<(MaskSource, [f32; 2])> {
        if let Some(cached) = self.0.stencil_cache.lock().get(&glyph_id) {
            return Some(cached.clone());
        }

        let font = self.0.font_system.font(glyph_id.font_id())?;
        let metrics = font.metrics_indexed(glyph_id.glyph_index(), glyph_id.font_size());
        if metrics.width == 0 || metrics.height == 0 {
            return None;
        }

        let region = MaskSource::new(
            MaskDescriptor::new(
                [metrics.width as u32, metrics.height as u32],
                wgpu::TextureFormat::R8Unorm,
            ),
            move |mut c| {
                let (_, bytes) =
                    font.rasterize_indexed(glyph_id.glyph_index(), glyph_id.font_size());
                upload_texture(&mut c.gpu, &c.target, &bytes)
            },
        )
        .with_output_layout(render_interface::PrepareOutputLayout::AnyRegion);

        let entry = (region, [metrics.width as f32, metrics.height as f32]);
        self.0.stencil_cache.lock().insert(glyph_id, entry.clone());
        Some(entry)
    }
}

/// Shape `content` fresh (no caching — see module docs) at `font_size`,
/// word-wrapping at `max_width`. Returns an empty layout if no matching font
/// is found rather than panicking.
pub(crate) fn shape(
    font_ctx: &FontCtx,
    content: &str,
    font_size: f32,
    max_width: f32,
) -> suzuri::text::TextLayout<()> {
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
pub(crate) fn solid_source(ctx: &RenderCtx, color: [f32; 4]) -> Option<TextureSource> {
    crate::color::solid_source(ctx, color, "Text")
}

/// Emit `layout`'s glyphs as Objects, each a
/// tint-texture quad masked by its shared coverage definition, tinted
/// uniformly by `tint_source` (see `solid_source`). Shared by `Text`'s
/// own render item and by any other widget (e.g. `Button`'s label) that needs
/// to draw a shaped single-style glyph run without duplicating the
/// suzuri-shaping/stencil-cache glue.
pub(crate) fn draw_glyph_run(
    scene: &mut Draw<'_>,
    font_ctx: &FontCtx,
    layout: &suzuri::text::TextLayout<()>,
    tint: &TextureSource,
    offset: Matrix4<f32>,
) {
    for line in &layout.lines {
        for glyph in &line.glyphs {
            if let Some((mask, size)) = font_ctx.glyph_source(glyph.glyph_id) {
                let transform =
                    offset * Matrix4::new_translation(&Vector3::new(glyph.x, glyph.y, 0.));
                matcha_ecs::scene::push_quad(scene, tint, size, transform, Some(&mask));
            }
        }
    }
}

/// Build a writer retaining its shaped layout at the live wrap width (reading
/// the live wrap width from `wrap_width`) and draws each glyph as a
/// tint-texture quad masked by its cached stencil coverage bitmap.
fn text_render_item(
    font_ctx: FontCtx,
    wrap_width: Arc<LiveF32>,
    content: String,
    font_size: f32,
    color: [f32; 4],
) -> RenderItem {
    let cached = parking_lot::Mutex::new(None);
    let tint = parking_lot::Mutex::new(None);
    RenderItem::new(move |ctx: &RenderCtx, draw| {
        let max_width = wrap_width.get();
        let mut cached = cached.lock();
        if cached.as_ref().is_none_or(|(width, _)| *width != max_width) {
            *cached = Some((max_width, shape(&font_ctx, &content, font_size, max_width)));
        }
        let layout = &cached.as_ref().expect("shaped width").1;

        let mut tint = tint.lock();
        let tint_source = tint.get_or_insert_with(|| solid_source(ctx, color).expect("solid tint"));

        draw_glyph_run(draw, &font_ctx, &layout, &tint_source, Matrix4::identity());
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
        // two extra full shaping passes per measure. A `Text` in a shrinking
        // row therefore will not go below the width it wrapped to; `RichText` will.
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
    /// Enter and exit fades use the same opacity transitions as `ColorRect`.
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
        let font_ctx =
            entity.world_scope(|world| world.get_resource_or_insert_with(FontCtx::new).clone());
        let wrap_width = entity
            .get::<TextWrapWidth>()
            .expect("bundle() inserted TextWrapWidth")
            .0
            .clone();
        text_render_item(
            font_ctx,
            wrap_width,
            self.content.clone(),
            self.font_size,
            self.color,
        )
    }
}

impl Widget for Text {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        // `after_spawn` builds the RenderItem because it can access FontCtx
        // in the world; bundle() only supplies the entity's initial components.
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
    //! public-API integration tests in `tests/text.rs`, its
    //! write-through from `TextStyle::arrange` can only be checked from
    //! inside this crate.
    use bevy_ecs::world::World;
    use matcha_ecs::{
        components::view::ViewChildren,
        layout::{Constraints, layout_root},
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

        let out = world
            .get::<matcha_ecs::components::layout::LayoutOutput>(child)
            .unwrap();
        assert_eq!(
            stored_width, out.size[0],
            "TextWrapWidth must hold exactly the width arrange() resolved this entity to"
        );
    }
}
