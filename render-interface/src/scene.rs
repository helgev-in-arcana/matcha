//! Ordered drawing data. Phase scheduling belongs to the calling framework;
//! renderers execute the supplied order and own no UI hierarchy.

use std::error::Error;

use crate::{MaskId, Matrix4, MeshId, ResourcePool, TextureId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelMaskIndex(pub u32);

/// Complete drawing data borrowed by a renderer for one call.
///
/// Every referenced resource needs a definition even when its GPU content is
/// cached. An empty scene is valid. Unreferenced definitions request no work.
#[derive(Default)]
pub struct Scene {
    pub resources: ResourcePool,
    pub phases: Vec<Phase>,
    pub pixel_masks: Vec<PixelMask>,
}
/// One ordered group whose generators all read the same phase-start snapshot.
///
/// Sources are prepared on their first referenced phase, including references
/// through mask ancestors. A source reused by later phases keeps that content.
/// Array order is meaningful; phase numbers carry no widget-local semantics.
#[derive(Default, Clone)]
pub struct Phase {
    pub objects: Vec<Object>,
}
/// One triangle mesh composited in array order using premultiplied linear RGBA
/// and source-over. Opacity scales its channels, not an isolated group image.
///
/// Transforms must contain finite values. For `p = transform * [x, y, z, 1]`,
/// the projected position is `[p.x / p.w, p.y / p.w]` in Y-down viewport pixels.
/// Homogeneous `w` is preserved for clipping and perspective interpolation;
/// transformed `z` does not participate in depth testing or depth clipping.
/// Clip coordinates are `[2*p.x/width-p.w, p.w-2*p.y/height, 0, p.w]`.
/// Both triangle windings draw; array order, not vertex depth, determines overlap.
#[derive(Debug, Clone)]
pub struct Object {
    pub mesh: MeshId,
    pub texture: TextureId,
    pub transform: Matrix4<f32>,
    pub mask: Option<PixelMaskIndex>,
    /// Finite value in `0.0..=1.0`, multiplying all premultiplied channels.
    /// Invalid values are a scene error, not implicitly clamped.
    pub opacity: f32,
}
impl Object {
    pub fn new(mesh: MeshId, texture: TextureId, transform: Matrix4<f32>) -> Self {
        Self {
            mesh,
            texture,
            transform,
            mask: None,
            opacity: 1.0,
        }
    }
}
#[derive(Debug, Clone)]
pub struct PixelMask {
    pub mesh: MeshId,
    pub texture: MaskId,
    /// Absolute transform with the same projection rules as [`Object`]. Parent
    /// masks contribute coverage, never an additional transform.
    pub transform: Matrix4<f32>,
    /// Must refer to a strictly preceding element of Scene.pixel_masks.
    pub parent: Option<PixelMaskIndex>,
}

/// A render call's destination is backend-defined; implementations may accept a
/// surface attachment or an offscreen target without putting surfaces in Scene.
///
/// CPU preparation callbacks complete before this call returns, but GPU work
/// need not. A [`crate::PrepareError`] aborts submission of that frame's recorded
/// work and publication of its new cache entries. Existing resident content is
/// preserved. A renderer cannot undo external CPU side effects of a callback.
/// `Ok(())` does not assert GPU completion or absence of errors delivered through
/// wgpu's validation/device notification mechanism.
pub trait Renderer {
    type Target<'a>;
    type Error: Error;
    fn render(&mut self, scene: &Scene, target: Self::Target<'_>) -> Result<(), Self::Error>;
}
