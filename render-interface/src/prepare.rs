//! Borrowed GPU recording contexts; submission and resident placement belong
//! to the renderer, while the source initializes its complete logical output.

use std::error::Error;

use crate::{MeshDescriptor, TextureDescriptor};

/// A CPU-side failure to record valid generation work.
///
/// Returning an error aborts the frame's submission and new cache publication;
/// callbacks should avoid external side effects because those cannot be undone.
/// This is distinct from wgpu validation/device errors and GPU completion.
pub type PrepareError = Box<dyn Error + Send + Sync + 'static>;
pub type PrepareResult = Result<(), PrepareError>;

/// Read-only image at the start of the source's first referenced phase.
///
/// The full single-mip, single-sample 2D colour image supports TEXTURE_BINDING
/// and COPY_SRC. Its actual size and format are reported here; a generator must
/// not assume they match its own output. No storage or writable usage is promised.
/// The renderer may alias its accumulation image if all generation reads precede
/// that phase's drawing writes. It must not substitute a later-phase snapshot.
#[derive(Clone, Copy)]
pub struct RenderSnapshot<'a> {
    pub color_texture: &'a wgpu::Texture,
    pub color_view: &'a wgpu::TextureView,
    pub size: [u32; 2],
    pub format: wgpu::TextureFormat,
}
/// Borrowed recording access for a resource generator.
///
/// Callbacks must not retain output/snapshot handles (including clones) or the
/// encoder, submit commands, destroy borrowed resources, or mutate the snapshot.
/// Device clones and private pipelines/work resources may be retained, with
/// caches scoped to device identity. Callbacks may record
/// copy, compute and render work and create private intermediates. Every declared
/// output byte/texel must be initialized; prior output contents are unspecified.
/// The output is a logical resource at offset/origin zero, never an atlas region.
/// Its identity and fresh allocation are not guaranteed. A renderer may copy it
/// into resident storage and reuse it later in the same ordered command stream.
/// These are trusted extension rules: raw wgpu access is not a sandbox.
pub struct GpuPrepareContext<'a> {
    pub device: &'a wgpu::Device,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub snapshot: RenderSnapshot<'a>,
}
pub struct MeshTarget<'a> {
    pub desc: &'a MeshDescriptor,
    /// Exactly `vertex_count * size_of::<Vertex>()` bytes at offset zero;
    /// usages include COPY_SRC | COPY_DST | VERTEX and descriptor additions.
    pub vertices: &'a wgpu::Buffer,
    /// Present exactly when `index_count != 0`, with `index_count * 4` bytes at
    /// offset zero and COPY_SRC | COPY_DST | INDEX plus descriptor additions.
    pub indices: Option<&'a wgpu::Buffer>,
}
/// Full logical 2D image matching `desc`, not a view into a resident atlas.
/// Its usages include TEXTURE_BINDING | COPY_SRC | COPY_DST and descriptor additions.
pub struct TextureTarget<'a> {
    pub desc: &'a TextureDescriptor,
    pub texture: &'a wgpu::Texture,
    pub view: &'a wgpu::TextureView,
}
pub struct MeshPrepareContext<'a> {
    pub gpu: GpuPrepareContext<'a>,
    pub target: MeshTarget<'a>,
}
pub struct TexturePrepareContext<'a> {
    pub gpu: GpuPrepareContext<'a>,
    pub target: TextureTarget<'a>,
}
pub struct MaskPrepareContext<'a> {
    pub gpu: GpuPrepareContext<'a>,
    pub target: TextureTarget<'a>,
}
