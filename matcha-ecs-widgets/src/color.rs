//! Turning a linear `[f32; 4]` colour into the bytes the colour atlas stores,
//! and painting a tint pixel with them.
//!
//! Three copies of this used to exist — in `text`, `rich_text` and `shape` —
//! and they had already drifted: `shape` premultiplies and the other two did
//! not, so the fix for that lived in one of the three and could not reach the
//! others. One home is the point of this module.

use matcha_paint::Bitmap;
use matcha_ecs::components::render::RenderCtx;

/// Gamma-encode a linear colour component into the sRGB space the colour atlas
/// stores.
///
/// A render pass targeting an `Rgba8UnormSrgb` texture does this itself.
/// `Bitmap::write_data` is a raw byte copy and does not, so anything
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

/// Define a shared CPU tint pixel. GPU upload is deferred to the renderer's
/// source cache; no atlas allocation or GPU work occurs in a widget builder.
pub(crate) fn paint_tint_region(
    ctx: &RenderCtx,
    color: [f32; 4],
    what: &str,
) -> Option<Bitmap> {
    let _ = (ctx, what);
    Bitmap::rgba([1, 1], premultiplied_srgb_bytes(color).to_vec()).ok()
}
