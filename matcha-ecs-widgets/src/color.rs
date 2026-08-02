//! Turning a linear `[f32; 4]` colour into the bytes the colour atlas stores,
//! and painting a tint pixel with them.
//!
//! Three copies of this used to exist — in `text`, `rich_text` and `shape` —
//! and they had already drifted: `shape` premultiplies and the other two did
//! not, so the fix for that lived in one of the three and could not reach the
//! others. One home is the point of this module.

use gpu_utils::texture_atlas::AtlasRegion;
use matcha_ecs::components::render::RenderCtx;

/// Gamma-encode a linear colour component into the sRGB space the colour atlas
/// stores.
///
/// A render pass targeting an `Rgba8UnormSrgb` texture does this itself.
/// `AtlasRegion::write_data` is a raw byte copy and does not, so anything
/// uploading bytes by hand owes the conversion.
pub(crate) fn linear_to_srgb_u8(c: f32) -> u8 {
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
/// colour written straight comes out too bright. Opaque fills are unaffected,
/// which is why the two text widgets got away without this for as long as they
/// did — the alpha they paint with is almost always 1.
pub(crate) fn premultiplied_srgb_bytes(color: [f32; 4]) -> [u8; 4] {
    let a = color[3].clamp(0.0, 1.0);
    [
        linear_to_srgb_u8(color[0] * a),
        linear_to_srgb_u8(color[1] * a),
        linear_to_srgb_u8(color[2] * a),
        (a * 255.0).round() as u8,
    ]
}

/// Paint a 1x1 solid-colour region into `ctx.texture_atlas`, stretched by UV
/// clamping over whatever it is drawn onto — a glyph's stencil tint, a flat
/// box, a decoration rule.
///
/// Written directly via `write_data` rather than through a render pass: a real
/// GPU render pass whose viewport is scoped to a 1x1 (or otherwise very small)
/// atlas region was found to rasterise incorrectly — a soft, mispositioned
/// blob instead of a flat fill, reproduced in isolation with a hand-built 4x4
/// case and unrelated to glyphs or stencils. Root cause never identified (see
/// `ECS_IMPLEMENTATION_PLAN.md` §8); `write_data` sidesteps it and is the
/// simpler upload for a flat fill regardless.
///
/// `what` names the caller in the log line, since an atlas failure here is
/// otherwise indistinguishable between widgets.
pub(crate) fn paint_tint_region(
    ctx: &RenderCtx,
    color: [f32; 4],
    what: &str,
) -> Option<AtlasRegion> {
    let region = match ctx.texture_atlas.allocate(ctx.device, ctx.queue, [1, 1]) {
        Ok(region) => region,
        Err(e) => {
            log::error!("{what} tint region allocation failed: {e}");
            return None;
        }
    };

    if let Err(e) = region.write_data(ctx.queue, &premultiplied_srgb_bytes(color)) {
        log::error!("{what} tint upload failed: {e}");
        return None;
    }

    Some(region)
}
