//! Rasterising box shapes, and caching what comes out.
//!
//! This is the "how" layer under [`crate::box_style`]'s "what": one rasteriser
//! covering every filled, ringed or blurred rounded rectangle the box model can
//! ask for, plus the two caches that make redrawing one affordable.
//!
//! **How a shape is drawn.** `renderer` has no rounded-rect support:
//! `MaskData.kind` reserves a slot for analytic SDF shapes but only coverage
//! masks are implemented. So a rounded rect goes down the same path a glyph
//! does — a flat tint quad from the colour atlas, masked by a CPU-rasterised
//! coverage bitmap in the stencil atlas, composited by
//! `RenderNode::with_stencil`.
//!
//! That indirection is not just expedience, it is the *correct* way to get
//! antialiased edges here. Coverage multiplies all four channels
//! (`renderer_render.wgsl`) and the pipeline blends premultiplied, so a
//! half-covered edge pixel comes out correctly attenuated. Writing an
//! antialiased RGBA bitmap into the colour atlas instead would blend as though
//! every pixel were fully opaque, and the edges would read too bright.
//!
//! **Why the two caches are split the way they are.** Coverage is keyed on
//! shape alone and tint on colour alone, so recolouring reuses the mask and
//! resizing reuses the colour. That is what makes a hover transition affordable:
//! it changes only which 1x1 texel the quad samples.

use std::sync::Arc;

use bevy_ecs::{resource::Resource, world::EntityWorldMut};
use fxhash::FxHashMap;
use gpu_utils::texture_atlas::AtlasRegion;
use parking_lot::Mutex;

use matcha_ecs::components::render::RenderCtx;

use crate::color::premultiplied_srgb_bytes;

/// Identifies a coverage bitmap. Quantised to whole pixels because that is what
/// is rasterised — keying on raw floats would miss on every sub-pixel wobble
/// and grow the cache without bound.
///
/// One key covers all three shapes the box model needs, because they are the
/// same SDF read differently: a filled rounded rect (`border` all zero), a
/// border ring (`border` non-zero — the outer shape minus the inset one), and
/// either of those blurred for a shadow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoverageKey {
    pub w: u32,
    pub h: u32,
    /// Per-corner radius in 1/16 px, quantised for the same reason:
    /// top-left, top-right, bottom-right, bottom-left.
    pub radius_16th: [u32; 4],
    /// Per-side border width in 1/16 px (top, right, bottom, left). All zero
    /// means a filled shape rather than a ring.
    pub border_16th: [u32; 4],
    /// Gaussian sigma in 1/16 px. Zero means a hard, merely antialiased edge.
    pub blur_16th: u32,
    /// How far in from the bitmap's edge the shape stops, in 1/16 px. Zero
    /// means it fills the bitmap.
    ///
    /// This exists for shadows: a blur needs empty margin around the shape to
    /// fade out into, and a shape that ran to the bitmap's edge would be cut
    /// off square by it.
    pub inset_16th: u32,
}

impl CoverageKey {
    /// A filled rounded rect.
    pub fn filled(w: u32, h: u32, radius: [f32; 4]) -> Self {
        Self {
            w,
            h,
            radius_16th: radius.map(quantize),
            border_16th: [0; 4],
            blur_16th: 0,
            inset_16th: 0,
        }
    }

    /// The ring left between the outer shape and one inset by `border`
    /// (top, right, bottom, left).
    pub fn ring(w: u32, h: u32, radius: [f32; 4], border: [f32; 4]) -> Self {
        Self {
            border_16th: border.map(quantize),
            ..Self::filled(w, h, radius)
        }
    }

    pub fn blurred(mut self, sigma: f32) -> Self {
        self.blur_16th = quantize(sigma);
        self
    }

    /// Leave `margin` px of empty bitmap on every side, for a blur to fade out
    /// into.
    pub fn inset(mut self, margin: f32) -> Self {
        self.inset_16th = quantize(margin);
        self
    }
}

fn quantize(v: f32) -> u32 {
    (v.max(0.0) * 16.0).round() as u32
}

fn dequantize(v: u32) -> f32 {
    v as f32 / 16.0
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
/// Caching is what makes a decorated box affordable to redraw. A scrollbar thumb
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

    /// Fetch (rasterising and uploading on a miss) the coverage bitmap for
    /// `key`, as a region of the stencil atlas.
    pub fn coverage_region(&self, key: CoverageKey, ctx: &RenderCtx) -> Option<AtlasRegion> {
        if key.w == 0 || key.h == 0 {
            return None;
        }
        if let Some(cached) = self.0.coverage.lock().get(&key) {
            return Some(cached.clone());
        }

        let bitmap = rasterize_box(key);
        let region = match ctx
            .stencil_atlas
            .allocate(ctx.device, ctx.queue, [key.w, key.h])
        {
            Ok(region) => region,
            Err(e) => {
                log::error!("box coverage allocation failed: {e}");
                return None;
            }
        };
        if let Err(e) = region.write_data(ctx.queue, &bitmap) {
            log::error!("box coverage upload failed: {e}");
            return None;
        }

        self.0.coverage.lock().insert(key, region.clone());
        Some(region)
    }

    /// Fetch (allocating and uploading on a miss) a 1x1 region of the colour
    /// atlas holding `color`, premultiplied.
    ///
    /// One texel is enough at any size: the shader clamps a sample into the
    /// region's own texel centre, so stretching it over a whole quad samples
    /// that one texel everywhere. Same trick the core's `ClipMask` uses, and
    /// it is why recolouring a box costs no rasterisation at all.
    pub fn tint_region(&self, color: [f32; 4], ctx: &RenderCtx) -> Option<AtlasRegion> {
        // Keyed on the bytes actually uploaded, so two colours that encode
        // identically share a texel.
        let bytes = premultiplied_srgb_bytes(color);
        if let Some(cached) = self.0.tint.lock().get(&bytes) {
            return Some(cached.clone());
        }

        let region = crate::color::paint_tint_region(ctx, color, "box")?;
        self.0.tint.lock().insert(bytes, region.clone());
        Some(region)
    }
}

/// Signed distance to a rounded box centred at the origin with half-extents
/// `half` and per-corner radii `radius` (tl, tr, br, bl). Negative inside.
fn rounded_box_sdf(p: [f32; 2], half: [f32; 2], radius: [f32; 4]) -> f32 {
    // Pick the radius belonging to the quadrant the point is in. Y is down, so
    // positive y is the bottom half.
    let r = match (p[0] >= 0.0, p[1] >= 0.0) {
        (false, false) => radius[0],
        (true, false) => radius[1],
        (true, true) => radius[2],
        (false, true) => radius[3],
    };
    let r = r.clamp(0.0, half[0].min(half[1]));

    let qx = p[0].abs() - (half[0] - r);
    let qy = p[1].abs() - (half[1] - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let inside = qx.max(qy).min(0.0);
    outside + inside - r
}

/// Turn a signed distance into coverage: fully covered half a pixel inside the
/// boundary, not at all half a pixel outside.
fn coverage_of(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// Rasterise `key` into a one-byte-per-pixel coverage bitmap (the stencil atlas
/// is `R8Unorm`).
///
/// A ring is the outer shape's coverage *minus* the inset shape's, which keeps
/// both of its edges antialiased and costs one extra SDF evaluation per pixel —
/// far simpler than tracing an annulus directly, and exact because the inner
/// shape is contained in the outer one.
///
/// The inner corner radius is CSS's `outer − adjacent border`, floored at zero.
/// CSS resolves that per axis, giving an elliptical inner curve; this SDF is
/// axis-uniform, so the larger of the two adjacent border widths is used. The
/// difference is sub-pixel unless two adjacent borders differ sharply.
pub fn rasterize_box(key: CoverageKey) -> Vec<u8> {
    let (w, h) = (key.w, key.h);
    let mut out = vec![0u8; (w as usize) * (h as usize)];
    if w == 0 || h == 0 {
        return out;
    }

    let inset = dequantize(key.inset_16th);
    let half = [
        (w as f32 / 2.0 - inset).max(0.0),
        (h as f32 / 2.0 - inset).max(0.0),
    ];
    let radius = key.radius_16th.map(dequantize);
    let border = key.border_16th.map(dequantize);
    let ring = border.iter().any(|&b| b > 0.0);

    // The inner shape as an offset rectangle: inset per side, so its centre
    // moves whenever opposite borders differ.
    let (top, right, bottom, left) = (border[0], border[1], border[2], border[3]);
    let inner_half = [
        (half[0] - (left + right) / 2.0).max(0.0),
        (half[1] - (top + bottom) / 2.0).max(0.0),
    ];
    let inner_centre = [(right - left) / -2.0, (bottom - top) / -2.0];
    let inner_radius = [
        (radius[0] - left.max(top)).max(0.0),
        (radius[1] - right.max(top)).max(0.0),
        (radius[2] - right.max(bottom)).max(0.0),
        (radius[3] - left.max(bottom)).max(0.0),
    ];

    for y in 0..h {
        for x in 0..w {
            let px = (x as f32 + 0.5) - w as f32 / 2.0;
            let py = (y as f32 + 0.5) - h as f32 / 2.0;

            let mut c = coverage_of(rounded_box_sdf([px, py], half, radius));
            if ring {
                let inner = coverage_of(rounded_box_sdf(
                    [px - inner_centre[0], py - inner_centre[1]],
                    inner_half,
                    inner_radius,
                ));
                c = (c - inner).clamp(0.0, 1.0);
            }
            out[(y as usize) * (w as usize) + (x as usize)] = (c * 255.0).round() as u8;
        }
    }

    let sigma = dequantize(key.blur_16th);
    if sigma > 0.0 {
        blur(&mut out, w as usize, h as usize, sigma);
    }
    out
}

/// Three box-blur passes, which approximate a Gaussian closely enough that the
/// difference is invisible in a shadow — and run in O(pixels) rather than
/// O(pixels · radius).
///
/// The box width matching a Gaussian of `sigma` over three passes is
/// `sigma * sqrt(12/3 + 1)`, i.e. a radius of about `1.12 * sigma`.
fn blur(data: &mut [u8], w: usize, h: usize, sigma: f32) {
    let radius = (sigma * 1.12).round() as usize;
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let mut scratch = vec![0u8; data.len()];
    for _ in 0..3 {
        // Horizontal, then the same pass again on the transpose, which is the
        // vertical one. Separability is what keeps this linear.
        blur_rows(data, &mut scratch, w, h, radius);
        transpose(&scratch, data, w, h);
        blur_rows(data, &mut scratch, h, w, radius);
        transpose(&scratch, data, h, w);
    }
}

/// One horizontal box-blur pass, via a running sum. Edge pixels are extended
/// rather than treated as zero, so a shape touching the bitmap's border does
/// not develop a dark seam there.
fn blur_rows(src: &[u8], dst: &mut [u8], w: usize, h: usize, radius: usize) {
    let window = (radius * 2 + 1) as u32;
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let at = |i: isize| row[i.clamp(0, w as isize - 1) as usize] as u32;

        let mut sum: u32 = (0..=radius as isize).map(|i| at(i)).sum::<u32>()
            + row[0] as u32 * radius as u32;

        for x in 0..w {
            dst[y * w + x] = (sum / window) as u8;
            sum += at(x as isize + radius as isize + 1);
            sum -= at(x as isize - radius as isize);
        }
    }
}

fn transpose(src: &[u8], dst: &mut [u8], w: usize, h: usize) {
    for y in 0..h {
        for x in 0..w {
            dst[x * h + y] = src[y * w + x];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::linear_to_srgb_u8;

    fn at(bitmap: &[u8], w: u32, x: u32, y: u32) -> u8 {
        bitmap[(y as usize) * (w as usize) + (x as usize)]
    }

    fn filled(w: u32, h: u32, radius: f32) -> Vec<u8> {
        rasterize_box(CoverageKey::filled(w, h, [radius; 4]))
    }

    #[test]
    fn a_zero_radius_rect_is_fully_covered_including_its_corners() {
        let b = filled(8, 5, 0.0);
        assert!(
            b.iter().all(|&v| v == 255),
            "a square rect should have no partial pixels: {b:?}"
        );
    }

    #[test]
    fn the_centre_is_always_fully_covered() {
        for radius in [0.0, 1.0, 4.0, 10.0, 1000.0] {
            let b = filled(20, 20, radius);
            assert_eq!(at(&b, 20, 10, 10), 255, "radius {radius}");
        }
    }

    #[test]
    fn corners_are_cut_away_when_a_radius_is_given() {
        let b = filled(20, 20, 8.0);
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
        assert_eq!(filled(40, 10, 5.0), filled(40, 10, 500.0));
    }

    #[test]
    fn the_boundary_is_antialiased_rather_than_hard() {
        let b = filled(20, 20, 8.0);
        assert!(
            b.iter().any(|&v| v > 0 && v < 255),
            "a rounded corner should produce partially covered pixels"
        );
    }

    #[test]
    fn a_degenerate_size_produces_an_empty_bitmap() {
        assert!(filled(0, 10, 2.0).is_empty());
        assert!(filled(10, 0, 2.0).is_empty());
    }

    #[test]
    fn per_corner_radii_round_only_the_corners_asked_for() {
        // Top-left only.
        let b = rasterize_box(CoverageKey::filled(20, 20, [8.0, 0.0, 0.0, 0.0]));
        assert_eq!(at(&b, 20, 0, 0), 0, "the rounded corner is cut away");
        for (x, y) in [(19, 0), (19, 19), (0, 19)] {
            assert_eq!(at(&b, 20, x, y), 255, "corner ({x}, {y}) stays square");
        }
    }

    #[test]
    fn a_ring_is_hollow_and_as_thick_as_its_border() {
        let b = rasterize_box(CoverageKey::ring(20, 20, [0.0; 4], [3.0; 4]));
        assert_eq!(at(&b, 20, 10, 10), 0, "the middle is not painted");
        assert_eq!(at(&b, 20, 10, 0), 255, "the outermost row is");
        assert_eq!(at(&b, 20, 10, 2), 255, "and so is the last border row");
        assert_eq!(at(&b, 20, 10, 4), 0, "one row past the border is not");
    }

    #[test]
    fn a_one_sided_border_paints_only_that_side() {
        // CSS `border-bottom` — the case a divider is made of.
        let b = rasterize_box(CoverageKey::ring(20, 20, [0.0; 4], [0.0, 0.0, 4.0, 0.0]));
        assert_eq!(at(&b, 20, 10, 19), 255, "the bottom edge is painted");
        assert_eq!(at(&b, 20, 10, 0), 0, "the top edge is not");
        assert_eq!(at(&b, 20, 0, 10), 0, "nor either side");
    }

    #[test]
    fn an_inset_shape_leaves_the_margin_empty() {
        // What a shadow needs: room inside the bitmap for the blur to fade out
        // into, rather than a shape cut off square by the bitmap's edge.
        let b = rasterize_box(CoverageKey::filled(40, 40, [0.0; 4]).inset(8.0));
        assert_eq!(at(&b, 40, 0, 20), 0, "the margin is untouched");
        assert_eq!(at(&b, 40, 9, 20), 255, "the shape starts after it");
        assert_eq!(at(&b, 40, 20, 20), 255);
    }

    #[test]
    fn blurring_softens_the_edge_without_moving_it() {
        let sharp = rasterize_box(CoverageKey::filled(60, 60, [0.0; 4]).inset(15.0));
        let soft = rasterize_box(CoverageKey::filled(60, 60, [0.0; 4]).inset(15.0).blurred(4.0));

        assert_eq!(at(&sharp, 60, 5, 30), 0, "well outside the sharp shape");
        assert!(
            at(&soft, 60, 12, 30) > 0,
            "a blur should spread coverage into the margin"
        );
        assert!(
            at(&soft, 60, 15, 30) < 255,
            "and soften the boundary itself"
        );
        assert_eq!(at(&soft, 60, 30, 30), 255, "the middle stays solid");

        // The blur redistributes coverage rather than eating it: symmetric
        // spreading leaves the total roughly where it was.
        let total = |b: &[u8]| b.iter().map(|&v| v as u64).sum::<u64>();
        let (a, b) = (total(&sharp), total(&soft));
        assert!(
            b * 10 > a * 8 && a * 10 > b * 8,
            "blurring should conserve coverage, not create or destroy it: {a} -> {b}"
        );
    }

    #[test]
    fn a_tint_is_premultiplied_in_linear_space() {
        // Opaque white stays white.
        assert_eq!(
            premultiplied_srgb_bytes([1.0, 1.0, 1.0, 1.0]),
            [255, 255, 255, 255]
        );
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
