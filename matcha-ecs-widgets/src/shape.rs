//! Rounded rectangles.
//!
//! A general drawing primitive, sibling to `color_rect`'s `solid_rect_node` —
//! the scrollbar is simply its first caller, and `Panel`/`Button`/`Checkbox`
//! could adopt it for rounded corners without changes here.
//!
//! **How it is drawn.** `renderer` has no rounded-rect support: `MaskData.kind`
//! reserves a slot for analytic SDF shapes but only coverage masks are
//! implemented. So a rounded rect goes down the same path a glyph does — a flat
//! tint quad from the colour atlas, masked by a CPU-rasterised coverage bitmap
//! in the stencil atlas, composited by `RenderNode::with_stencil`.
//!
//! That indirection is not just expedience, it is the *correct* way to get
//! antialiased edges here. Coverage multiplies all four channels
//! (`renderer_render.wgsl`) and the pipeline blends premultiplied, so a
//! half-covered edge pixel comes out correctly attenuated. Writing an
//! antialiased RGBA bitmap into the colour atlas instead would blend as though
//! every pixel were fully opaque, and the edges would read too bright.

use std::sync::Arc;

use bevy_ecs::{resource::Resource, world::EntityWorldMut};
use gpu_utils::texture_atlas::AtlasRegion;
use nalgebra::Matrix4;
use parking_lot::Mutex;
use fxhash::FxHashMap;
use renderer::RenderNode;

use matcha_ecs::components::render::RenderCtx;

/// Identifies a coverage bitmap. Quantised to whole pixels because that is what
/// is rasterised — keying on raw floats would miss on every sub-pixel wobble
/// and grow the cache without bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CoverageKey {
    w: u32,
    h: u32,
    /// Radius in 1/16 px, quantised for the same reason.
    radius_16th: u32,
}

#[derive(Default)]
struct ShapeCtxInner {
    /// Coverage bitmaps, keyed on shape alone — deliberately independent of
    /// colour, so recolouring reuses the mask.
    coverage: Mutex<FxHashMap<CoverageKey, AtlasRegion>>,
    /// 1x1 tint pixels, keyed on the premultiplied bytes actually uploaded.
    tint: Mutex<FxHashMap<[u8; 4], AtlasRegion>>,
}

/// World resource holding the shape caches. Lazily inserted on first use so the
/// core never has to know it exists, exactly like `FontCtx`/`ImageCtx`. Cheap to
/// `Clone` (an `Arc` handle), so it can be captured straight into a
/// `RenderItem`'s `Send + Sync` builder closure.
///
/// Caching is what makes a rounded rect affordable to redraw. A scrollbar thumb
/// is rebuilt on every frame it moves (its `LayoutOutput` changes, so
/// `invalidate_on_layout_change` fires), but its *shape* is constant for the
/// whole drag — so a shape-keyed cache turns each of those rebuilds into
/// assembling a `RenderNode` from regions that already exist, instead of an
/// atlas allocation plus a rasterisation plus an upload.
#[derive(Resource, Clone, Default)]
pub struct ShapeCtx(Arc<ShapeCtxInner>);

impl ShapeCtx {
    /// Fetch the shared `ShapeCtx`, inserting it on first use.
    pub fn get(entity: &mut EntityWorldMut) -> Self {
        entity.world_scope(|world| world.get_resource_or_insert_with(ShapeCtx::default).clone())
    }

    fn coverage_region(&self, key: CoverageKey, ctx: &RenderCtx) -> Option<AtlasRegion> {
        if let Some(cached) = self.0.coverage.lock().get(&key) {
            return Some(cached.clone());
        }

        let bitmap = rasterize_rounded_rect(key.w, key.h, key.radius_16th as f32 / 16.0);
        let region = match ctx
            .stencil_atlas
            .allocate(ctx.device, ctx.queue, [key.w, key.h])
        {
            Ok(region) => region,
            Err(e) => {
                log::error!("rounded rect coverage allocation failed: {e}");
                return None;
            }
        };
        if let Err(e) = region.write_data(ctx.queue, &bitmap) {
            log::error!("rounded rect coverage upload failed: {e}");
            return None;
        }

        self.0.coverage.lock().insert(key, region.clone());
        Some(region)
    }

    fn tint_region(&self, color: [f32; 4], ctx: &RenderCtx) -> Option<AtlasRegion> {
        let bytes = premultiplied_srgb_bytes(color);
        if let Some(cached) = self.0.tint.lock().get(&bytes) {
            return Some(cached.clone());
        }

        // Written with `write_data` rather than a render pass: a pass scoped to
        // a 1x1 viewport rasterises incorrectly (see `text::paint_tint_region`
        // for the full history), and a raw byte copy is simpler anyway.
        let region = match ctx.texture_atlas.allocate(ctx.device, ctx.queue, [1, 1]) {
            Ok(region) => region,
            Err(e) => {
                log::error!("rounded rect tint allocation failed: {e}");
                return None;
            }
        };
        if let Err(e) = region.write_data(ctx.queue, &bytes) {
            log::error!("rounded rect tint upload failed: {e}");
            return None;
        }

        self.0.tint.lock().insert(bytes, region.clone());
        Some(region)
    }
}

/// Gamma-encode a linear colour component into the sRGB space the colour atlas
/// stores. `write_data` is a raw byte copy, so unlike a render pass targeting
/// an `Rgba8UnormSrgb` texture it does no conversion of its own.
fn linear_to_srgb_u8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let encoded = if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

/// Encode `color` as the bytes to upload for a tint pixel.
///
/// **Premultiplied**, and multiplied in *linear* space before the sRGB encode.
/// The pipeline blends with `PREMULTIPLIED_ALPHA_BLENDING`, so a translucent
/// colour written straight would come out too bright — invisible for opaque
/// text (which is why `text::paint_tint_region` has never shown it) but very
/// visible on a translucent scrollbar thumb.
fn premultiplied_srgb_bytes(color: [f32; 4]) -> [u8; 4] {
    let a = color[3].clamp(0.0, 1.0);
    [
        linear_to_srgb_u8(color[0] * a),
        linear_to_srgb_u8(color[1] * a),
        linear_to_srgb_u8(color[2] * a),
        (a * 255.0).round() as u8,
    ]
}

/// Rasterise an antialiased coverage bitmap for a `w`x`h` rounded rectangle,
/// one byte per pixel (the stencil atlas is `R8Unorm`).
///
/// Coverage comes from the rounded-rect signed distance field, taken at each
/// pixel's centre and turned into a one-pixel-wide ramp across the boundary.
/// `radius` is clamped to half the shorter side, past which the shape is a
/// stadium and a larger radius means nothing.
pub fn rasterize_rounded_rect(w: u32, h: u32, radius: f32) -> Vec<u8> {
    let mut out = vec![0u8; (w as usize) * (h as usize)];
    if w == 0 || h == 0 {
        return out;
    }

    let (fw, fh) = (w as f32, h as f32);
    let (hx, hy) = (fw / 2.0, fh / 2.0);
    let r = radius.clamp(0.0, hx.min(hy));

    for y in 0..h {
        for x in 0..w {
            // Pixel centre, relative to the rectangle's centre.
            let px = (x as f32 + 0.5) - hx;
            let py = (y as f32 + 0.5) - hy;

            // Standard rounded-box SDF: distance to the shrunk box, less the
            // radius. Negative inside, positive outside.
            let qx = px.abs() - (hx - r);
            let qy = py.abs() - (hy - r);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let inside = qx.max(qy).min(0.0);
            let d = outside + inside - r;

            // A pixel whose centre is half a pixel inside the boundary is fully
            // covered; half a pixel outside, not at all.
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            out[(y as usize) * (w as usize) + (x as usize)] = (coverage * 255.0).round() as u8;
        }
    }
    out
}

/// A rounded rectangle of `size`, filled with `color`.
///
/// Returns an empty node for a degenerate size or if the atlas is exhausted, so
/// a caller never has to branch on "is there anything to draw".
pub fn rounded_rect_node(
    ctx: &RenderCtx,
    shape: &ShapeCtx,
    size: [f32; 2],
    radius: f32,
    color: [f32; 4],
) -> RenderNode {
    let node = RenderNode::new();
    if size[0] < 0.5 || size[1] < 0.5 || color[3] <= 0.0 {
        return node;
    }

    // Draw at exactly the size rasterised, so the coverage bitmap maps one
    // texel per pixel and the antialiased edge is not resampled soft.
    let key = CoverageKey {
        w: size[0].round().max(1.0) as u32,
        h: size[1].round().max(1.0) as u32,
        radius_16th: (radius.max(0.0) * 16.0).round() as u32,
    };
    let drawn = [key.w as f32, key.h as f32];

    let (Some(tint), Some(coverage)) = (
        shape.tint_region(color, ctx),
        shape.coverage_region(key, ctx),
    ) else {
        return node;
    };

    node.with_texture(tint, drawn, Matrix4::identity())
        .with_stencil(coverage, drawn, Matrix4::identity())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(bitmap: &[u8], w: u32, x: u32, y: u32) -> u8 {
        bitmap[(y as usize) * (w as usize) + (x as usize)]
    }

    #[test]
    fn a_zero_radius_rect_is_fully_covered_including_its_corners() {
        let b = rasterize_rounded_rect(8, 5, 0.0);
        assert!(
            b.iter().all(|&v| v == 255),
            "a square rect should have no partial pixels: {b:?}"
        );
    }

    #[test]
    fn the_centre_is_always_fully_covered() {
        for radius in [0.0, 1.0, 4.0, 10.0, 1000.0] {
            let b = rasterize_rounded_rect(20, 20, radius);
            assert_eq!(at(&b, 20, 10, 10), 255, "radius {radius}");
        }
    }

    #[test]
    fn corners_are_cut_away_when_a_radius_is_given() {
        let b = rasterize_rounded_rect(20, 20, 8.0);
        for (x, y) in [(0, 0), (19, 0), (0, 19), (19, 19)] {
            assert_eq!(at(&b, 20, x, y), 0, "corner ({x}, {y}) should be empty");
        }
        // The edge midpoints are untouched by the corner rounding.
        assert_eq!(at(&b, 20, 10, 0), 255);
        assert_eq!(at(&b, 20, 0, 10), 255);
    }

    #[test]
    fn the_radius_is_clamped_to_half_the_shorter_side() {
        // Beyond half the height the shape is a stadium; asking for more must
        // change nothing rather than turning the SDF inside out.
        let stadium = rasterize_rounded_rect(40, 10, 5.0);
        let over = rasterize_rounded_rect(40, 10, 500.0);
        assert_eq!(stadium, over);
    }

    #[test]
    fn the_boundary_is_antialiased_rather_than_hard() {
        let b = rasterize_rounded_rect(20, 20, 8.0);
        assert!(
            b.iter().any(|&v| v > 0 && v < 255),
            "a rounded corner should produce partially covered pixels"
        );
    }

    #[test]
    fn a_degenerate_size_produces_an_empty_bitmap() {
        assert!(rasterize_rounded_rect(0, 10, 2.0).is_empty());
        assert!(rasterize_rounded_rect(10, 0, 2.0).is_empty());
    }

    #[test]
    fn a_tint_is_premultiplied_in_linear_space() {
        // Opaque white stays white.
        assert_eq!(premultiplied_srgb_bytes([1.0, 1.0, 1.0, 1.0]), [255, 255, 255, 255]);
        // Fully transparent contributes nothing on any channel.
        assert_eq!(premultiplied_srgb_bytes([1.0, 1.0, 1.0, 0.0]), [0, 0, 0, 0]);
        // Half alpha halves the *linear* colour, which is sRGB ~0.5^(1/2.2).
        let half = premultiplied_srgb_bytes([1.0, 1.0, 1.0, 0.5]);
        assert_eq!(half[3], 128);
        assert_eq!(half[0], linear_to_srgb_u8(0.5));
        // Straight alpha would have left this at 255 — the bug this avoids.
        assert!(half[0] < 255);
    }
}
