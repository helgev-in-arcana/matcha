//! Rendering contract types and GPU preparation helpers.
//! Ownership and execution rules are documented at the crate root.

pub use nalgebra::Matrix4;
pub use wgpu;

use std::{
    collections::HashMap,
    error::Error,
    sync::atomic::{AtomicU64, Ordering},
};

// IDs

// An ID identifies immutable logical content, independently of GPU placement.
// Relocation and eviction preserve it; reuse promises identical content.

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

macro_rules! content_id {
    ($($name:ident),+) => {$ (
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(u64);
        impl $name {
            pub fn new() -> Self {
                Self(NEXT_ID.fetch_update(Ordering::Relaxed, Ordering::Relaxed,
                    |v| v.checked_add(1)).expect("Content ID space exhausted, though this is unlikely in real-world scenarios."))
            }
            pub fn get(self) -> u64 { self.0 }
        }
        impl Default for $name { fn default() -> Self { Self::new() } }
    )+};
}

content_id!(MeshId, TextureId, MaskId);

// Render Interface

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

// Ordered drawing data. Phase scheduling belongs to the calling framework;
// renderers execute the supplied order and own no UI hierarchy.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelMaskIndex(pub u32);

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

// Resources

/// Owns logical source definitions, not GPU allocations.
#[derive(Default)]
pub struct ResourcePool {
    meshes: HashMap<MeshId, MeshSource>,
    textures: HashMap<TextureId, TextureSource>,
    masks: HashMap<MaskId, MaskSource>,
}

#[derive(Debug, thiserror::Error)]
#[error("duplicate resource definition {0}")]
pub struct DuplicateResource(pub u64);

impl ResourcePool {
    pub fn share_mesh(&mut self, source: &MeshSource) -> Result<MeshId, DuplicateResource> {
        if let Some(existing) = self.mesh(source.id()) {
            if !source.same_definition(existing) {
                return Err(DuplicateResource(source.id().get()));
            }
            return Ok(source.id());
        }
        self.insert_mesh(source.clone())
    }

    pub fn share_texture(
        &mut self,
        source: &TextureSource,
    ) -> Result<TextureId, DuplicateResource> {
        if let Some(existing) = self.texture(source.id()) {
            if !source.same_definition(existing) {
                return Err(DuplicateResource(source.id().get()));
            }
            return Ok(source.id());
        }
        self.insert_texture(source.clone())
    }

    pub fn share_mask(&mut self, source: &MaskSource) -> Result<MaskId, DuplicateResource> {
        if let Some(existing) = self.mask(source.id()) {
            if !source.same_definition(existing) {
                return Err(DuplicateResource(source.id().get()));
            }
            return Ok(source.id());
        }
        self.insert_mask(source.clone())
    }

    /// Merge definitions contributed by independent resource pools under the
    /// immutable-content-ID contract. Descriptor conflicts fail; closure semantic
    /// equivalence remains the producer's responsibility, not pointer identity.
    /// Public insert_* still rejects duplicates within one pool. This explicit
    /// composition operation materializes one definition per ID. No source
    /// clones/allocations occur on an existing entry.
    /// A closure uses one Arc allocation (replacing Box), not Arc<Source> + Box.
    pub fn import(&mut self, other: &Self) -> Result<(), DuplicateResource> {
        macro_rules! check {
            ($map:ident) => {
                for (id, source) in &other.$map {
                    if self
                        .$map
                        .get(id)
                        .is_some_and(|existing| !source.same_definition(existing))
                    {
                        return Err(DuplicateResource(id.get()));
                    }
                }
            };
        }
        check!(meshes);
        check!(textures);
        check!(masks);
        macro_rules! merge {
            ($map:ident) => {
                for (id, source) in &other.$map {
                    self.$map.entry(*id).or_insert_with(|| source.clone());
                }
            };
        }
        merge!(meshes);
        merge!(textures);
        merge!(masks);
        Ok(())
    }

    pub fn mesh_ids(&self) -> impl Iterator<Item = MeshId> + '_ {
        self.meshes.keys().copied()
    }

    pub fn texture_ids(&self) -> impl Iterator<Item = TextureId> + '_ {
        self.textures.keys().copied()
    }

    pub fn mask_ids(&self) -> impl Iterator<Item = MaskId> + '_ {
        self.masks.keys().copied()
    }

    pub fn len(&self) -> usize {
        self.meshes.len() + self.textures.len() + self.masks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

macro_rules! pool {
    ($insert:ident, $get:ident, $retain:ident, $map:ident, $id:ident, $source:ident) => {
        pub fn $insert(&mut self, source: $source) -> Result<$id, DuplicateResource> {
            let id = source.id();
            match self.$map.entry(id) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(source);
                    Ok(id)
                }
                std::collections::hash_map::Entry::Occupied(_) => Err(DuplicateResource(id.get())),
            }
        }
        pub fn $get(&self, id: $id) -> Option<&$source> {
            self.$map.get(&id)
        }
        pub fn $retain(&mut self, mut keep: impl FnMut($id) -> bool) {
            self.$map.retain(|id, _| keep(*id));
        }
    };
}

impl ResourcePool {
    pool!(insert_mesh, mesh, retain_meshes, meshes, MeshId, MeshSource);
    pool!(
        insert_texture,
        texture,
        retain_textures,
        textures,
        TextureId,
        TextureSource
    );
    pool!(insert_mask, mask, retain_masks, masks, MaskId, MaskSource);
}

macro_rules! source {
    ($name:ident, $id:ident, $desc:ident, $ctx:ident, $prepare:ident) => {
        pub type $prepare = dyn for<'a> Fn($ctx<'a>) -> PrepareResult + Send + Sync + 'static;
        #[derive(Clone)]
        pub struct $name {
            id: $id,
            desc: $desc,
            prepare: std::sync::Arc<$prepare>,
        }
        impl $name {
            pub fn new(
                desc: $desc,
                prepare: impl for<'a> Fn($ctx<'a>) -> PrepareResult + Send + Sync + 'static,
            ) -> Self {
                Self::with_id($id::new(), desc, prepare)
            }
            /// Re-submit an existing immutable definition. Caller guarantees the
            /// descriptor and generated content are identical for this ID.
            pub fn with_id(
                id: $id,
                desc: $desc,
                prepare: impl for<'a> Fn($ctx<'a>) -> PrepareResult + Send + Sync + 'static,
            ) -> Self {
                Self {
                    id,
                    desc,
                    prepare: std::sync::Arc::new(prepare),
                }
            }
            pub fn id(&self) -> $id {
                self.id
            }
            pub fn descriptor(&self) -> &$desc {
                &self.desc
            }
            pub fn prepare(&self, ctx: $ctx<'_>) -> PrepareResult {
                (self.prepare)(ctx)
            }
            fn same_definition(&self, other: &Self) -> bool {
                self.id == other.id && self.desc == other.desc
            }
        }
    };
}

source!(
    MeshSource,
    MeshId,
    MeshDescriptor,
    MeshPrepareContext,
    MeshPrepare
);

source!(
    TextureSource,
    TextureId,
    TextureDescriptor,
    TexturePrepareContext,
    TexturePrepare
);

source!(
    MaskSource,
    MaskId,
    MaskDescriptor,
    MaskPrepareContext,
    MaskPrepare
);

/// Logical triangle-list output, independent of its eventual GPU placement.
///
/// Referenced meshes must have at least one vertex. For non-indexed drawing,
/// `vertex_count` must be a multiple of three; otherwise `index_count` must be a
/// multiple of three and every generated index must be below `vertex_count`.
/// An empty draw is represented by omitting its Object, not an empty mesh.
/// Constructors do not validate these requirements; the renderer validates the
/// descriptor before preparation, while valid index contents are the producer's
/// responsibility (they may be generated entirely on the GPU).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshDescriptor {
    pub vertex_count: u32,
    /// Zero selects non-indexed drawing; otherwise indices are u32.
    pub index_count: u32,
    /// Additional buffer usages, e.g. STORAGE for compute generation.
    /// Outputs already guarantee COPY_SRC | COPY_DST and respectively VERTEX
    /// or INDEX. Mappable outputs are not supported. Device limits and requested
    /// usage support are checked by the renderer before calling the generator.
    pub usages: wgpu::BufferUsages,
    /// Optional conservative local AABB, including every generated vertex.
    /// A renderer may ignore it. Incorrectly narrow bounds violate the content
    /// contract. None preserves full generality for procedural geometry.
    pub bounds: Option<[[f32; 3]; 2]>,
    /// Every point of the projected surface is covered at most once. This
    /// permits coincident object/mask meshes to sample coverage directly by UV.
    /// Leave false for arbitrary overlapping triangle soups.
    pub non_overlapping: bool,
}

impl MeshDescriptor {
    pub fn triangles(vertex_count: u32, index_count: u32) -> Self {
        Self {
            vertex_count,
            index_count,
            usages: wgpu::BufferUsages::empty(),
            bounds: None,
            non_overlapping: false,
        }
    }
}

/// Fixed triangle vertex ABI: three position floats followed by two UV floats.
/// Positions are object-local; UVs are normalized, linearly sampled and clamped.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureDescriptor {
    /// Nonzero logical extent; exactly one 2D layer, mip and sample.
    pub size: [u32; 2],
    /// Colour is premultiplied in linear space. sRGB formats encode those
    /// premultiplied RGB values; linear formats store them directly.
    /// Supported formats are backend capabilities, not arbitrary format
    /// conversion requests; unsupported descriptors fail before preparation.
    pub format: wgpu::TextureFormat,
    /// Additional usages beyond TEXTURE_BINDING | COPY_SRC | COPY_DST.
    /// The renderer checks format/device support before calling the generator.
    pub usages: wgpu::TextureUsages,
}

impl TextureDescriptor {
    pub fn new(size: [u32; 2], format: wgpu::TextureFormat) -> Self {
        Self {
            size,
            format,
            usages: wgpu::TextureUsages::empty(),
        }
    }
}

/// A coverage image. Its sampled red channel represents linear coverage [0, 1];
/// other channels do not affect masking. It has the same output usage guarantees.
pub type MaskDescriptor = TextureDescriptor;

// Borrowed GPU recording contexts; submission and resident placement belong
// to the renderer, while the source initializes its complete logical output.

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

// Optional byte-upload conveniences for preparation callbacks. They record
// staging copies into the supplied encoder and never submit work themselves.

/// Record a byte upload without Queue access; submit remains renderer-owned.
/// Empty data is a no-op. Nonempty data must fit and be copy-aligned; this helper
/// may write a prefix, but the generator must initialize its complete output.
pub fn upload_buffer(
    gpu: &mut GpuPrepareContext<'_>,
    target: &wgpu::Buffer,
    bytes: &[u8],
) -> PrepareResult {
    use wgpu::util::DeviceExt;
    let size = buffer_upload_size(target.size(), bytes.len())?;
    if size == 0 {
        return Ok(());
    }
    if !target.usage().contains(wgpu::BufferUsages::COPY_DST) {
        return Err("buffer upload target lacks COPY_DST".into());
    }
    if size > gpu.device.limits().max_buffer_size {
        return Err("buffer upload exceeds device buffer limit".into());
    }
    let staging = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("source upload"),
            contents: bytes,
            usage: wgpu::BufferUsages::COPY_SRC,
        });
    gpu.encoder
        .copy_buffer_to_buffer(&staging, 0, target, 0, size);
    Ok(())
}

/// Tightly packed rows, padded here to WebGPU's copy alignment.
/// Supported formats are R8Unorm, Rgba8Unorm, Rgba8UnormSrgb and Rgba16Float.
/// Zero dimensions, invalid byte counts and unrepresentable staging sizes fail
/// before any GPU commands are recorded; no conversion or rescaling is performed.
pub fn upload_texture(
    gpu: &mut GpuPrepareContext<'_>,
    target: &TextureTarget<'_>,
    bytes: &[u8],
) -> PrepareResult {
    use wgpu::util::DeviceExt;
    let layout = texture_upload_layout(
        target.desc,
        bytes.len(),
        gpu.device.limits().max_buffer_size,
    )?;
    let [w, h] = target.desc.size;
    if target.texture.size()
        != (wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        })
        || target.texture.format() != target.desc.format
        || target.texture.dimension() != wgpu::TextureDimension::D2
        || target.texture.mip_level_count() != 1
        || target.texture.sample_count() != 1
        || !target
            .texture
            .usage()
            .contains(wgpu::TextureUsages::COPY_DST)
    {
        return Err("texture upload target does not match its descriptor or lacks COPY_DST".into());
    }
    let data = padded_texture_data(bytes, &layout)?;
    let staging = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("texture source upload"),
            contents: &data,
            usage: wgpu::BufferUsages::COPY_SRC,
        });
    gpu.encoder.copy_buffer_to_texture(
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(layout.padded_row),
                rows_per_image: Some(h),
            },
        },
        target.texture.as_image_copy(),
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}

fn buffer_upload_size(target_size: u64, byte_len: usize) -> Result<u64, PrepareError> {
    let size = u64::try_from(byte_len).map_err(|_| "buffer upload length overflow")?;
    if size > target_size || size % wgpu::COPY_BUFFER_ALIGNMENT != 0 {
        return Err("invalid buffer upload length/alignment".into());
    }
    Ok(size)
}

struct TextureUploadLayout {
    row: usize,
    padded_row: u32,
    staging_size: usize,
}

fn texture_upload_layout(
    desc: &TextureDescriptor,
    byte_len: usize,
    max_buffer_size: u64,
) -> Result<TextureUploadLayout, PrepareError> {
    let bpp = match desc.format {
        wgpu::TextureFormat::R8Unorm => 1,
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => 4,
        wgpu::TextureFormat::Rgba16Float => 8,
        _ => return Err("unsupported upload format".into()),
    };
    let [w, h] = desc.size;
    if w == 0 || h == 0 {
        return Err("texture upload dimensions must be nonzero".into());
    }
    let row = w.checked_mul(bpp).ok_or("row size overflow")?;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_row = row
        .checked_next_multiple_of(alignment)
        .ok_or("row padding overflow")?;
    let byte_len = u64::try_from(byte_len).map_err(|_| "texture upload length overflow")?;
    if byte_len != u64::from(row) * u64::from(h) {
        return Err("invalid texture upload length".into());
    }
    let staging_size = u64::from(padded_row) * u64::from(h);
    if staging_size > max_buffer_size {
        return Err("texture upload exceeds device buffer limit".into());
    }
    // Vec allocations cannot exceed isize::MAX even on a 64-bit host. Check
    // before narrowing to usize; padded rows can overflow a 32-bit allocation
    // even when the original tightly packed byte slice fits that address space.
    let staging_size = usize::try_from(staging_size)
        .ok()
        .filter(|size| *size <= isize::MAX as usize)
        .ok_or("texture staging allocation size overflow")?;
    Ok(TextureUploadLayout {
        row: usize::try_from(row).map_err(|_| "texture row size overflow")?,
        padded_row,
        staging_size,
    })
}

fn padded_texture_data(
    bytes: &[u8],
    layout: &TextureUploadLayout,
) -> Result<Vec<u8>, PrepareError> {
    let mut data = Vec::new();
    data.try_reserve_exact(layout.staging_size)?;
    data.resize(layout.staging_size, 0);
    for (source, target) in bytes
        .chunks_exact(layout.row)
        .zip(data.chunks_exact_mut(layout.padded_row as usize))
    {
        target[..layout.row].copy_from_slice(source);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_upload_rejects_unaligned_or_oversized_data_but_allows_empty() {
        assert_eq!(buffer_upload_size(0, 0).expect("empty upload is valid"), 0);
        assert_eq!(buffer_upload_size(8, 4).expect("aligned prefix fits"), 4);
        assert!(buffer_upload_size(8, 3).is_err());
        assert!(buffer_upload_size(8, 12).is_err());
    }

    #[test]
    fn texture_rows_preserve_pixels_and_zero_the_padding() {
        let desc = TextureDescriptor::new([3, 2], wgpu::TextureFormat::Rgba8Unorm);
        let bytes: Vec<u8> = (0..24).collect();
        let layout = texture_upload_layout(&desc, bytes.len(), 512)
            .expect("two short rows fit the staging buffer");
        let padded = padded_texture_data(&bytes, &layout).expect("small CPU allocation succeeds");
        assert_eq!(padded.len(), 512);
        assert_eq!(&padded[..12], &bytes[..12]);
        assert_eq!(&padded[256..268], &bytes[12..]);
        assert!(padded[12..256].iter().all(|b| *b == 0));
        assert!(padded[268..].iter().all(|b| *b == 0));
    }

    #[test]
    fn aligned_rows_need_no_extra_padding_for_each_supported_format() {
        for (format, width) in [
            (wgpu::TextureFormat::R8Unorm, 256),
            (wgpu::TextureFormat::Rgba8Unorm, 64),
            (wgpu::TextureFormat::Rgba8UnormSrgb, 64),
            (wgpu::TextureFormat::Rgba16Float, 32),
        ] {
            let desc = TextureDescriptor::new([width, 2], format);
            let bytes = vec![19; 512];
            let layout = texture_upload_layout(&desc, bytes.len(), 512)
                .expect("aligned rows occupy exactly their packed byte count");
            assert_eq!(
                padded_texture_data(&bytes, &layout).expect("small CPU allocation succeeds"),
                bytes
            );
        }
    }

    #[test]
    fn empty_extents_bad_lengths_and_unsupported_formats_fail_on_cpu() {
        for size in [[0, 1], [1, 0], [0, 0]] {
            let desc = TextureDescriptor::new(size, wgpu::TextureFormat::R8Unorm);
            assert!(texture_upload_layout(&desc, 0, 1024).is_err());
        }
        let desc = TextureDescriptor::new([2, 2], wgpu::TextureFormat::R8Unorm);
        assert!(texture_upload_layout(&desc, 3, 1024).is_err());
        assert!(texture_upload_layout(&desc, 5, 1024).is_err());
        let unsupported = TextureDescriptor::new([1, 1], wgpu::TextureFormat::Depth32Float);
        assert!(texture_upload_layout(&unsupported, 4, 1024).is_err());
    }

    #[test]
    fn row_overflow_and_padded_device_limit_fail_without_allocating() {
        let large = TextureDescriptor::new([u32::MAX, 1], wgpu::TextureFormat::Rgba16Float);
        assert!(texture_upload_layout(&large, 0, u64::MAX).is_err());
        let padding = TextureDescriptor::new([u32::MAX, 1], wgpu::TextureFormat::R8Unorm);
        assert!(texture_upload_layout(&padding, 0, u64::MAX).is_err());
        let tiny = TextureDescriptor::new([1, 2], wgpu::TextureFormat::R8Unorm);
        // Two source bytes still require two 256-byte staging rows.
        assert!(texture_upload_layout(&tiny, 2, 511).is_err());
        assert!(texture_upload_layout(&tiny, 2, 512).is_ok());
    }

    #[test]
    fn staging_larger_than_addressable_allocation_fails_without_allocating() {
        // Width one keeps the logical bytes representable on 32-bit hosts while
        // padding overflows their address space. On 64-bit hosts the wide image
        // fits usize, but its staging allocation exceeds Vec's isize::MAX bound.
        let size = if usize::BITS == 32 {
            [1, u32::MAX]
        } else {
            [0x8000_0100, u32::MAX]
        };
        let desc = TextureDescriptor::new(size, wgpu::TextureFormat::R8Unorm);
        let byte_len = usize::try_from(u64::from(size[0]) * u64::from(size[1]))
            .expect("the declared packed byte count fits this architecture");
        let error = texture_upload_layout(&desc, byte_len, u64::MAX)
            .err()
            .expect("oversized CPU staging must be rejected before allocation");
        assert!(error.to_string().contains("allocation size overflow"));
    }

    #[test]
    fn duplicate_registration_is_rejected_without_replacing_original() {
        let mut pool = ResourcePool::default();
        let desc = TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm);
        let first = TextureSource::new(desc, |_| Ok(()));
        let id = first.id();
        pool.insert_texture(first).expect("first definition");
        let different = TextureDescriptor::new([2, 2], wgpu::TextureFormat::Rgba8Unorm);
        assert!(
            pool.insert_texture(TextureSource::with_id(id, different, |_| Ok(())))
                .is_err()
        );
        assert_eq!(
            *pool.texture(id).expect("original remains").descriptor(),
            desc
        );
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn ids_are_unique_across_resource_types_and_concurrent_allocation() {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    (0..500)
                        .flat_map(|_| {
                            [
                                MeshId::new().get(),
                                TextureId::new().get(),
                                MaskId::new().get(),
                            ]
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let ids: std::collections::HashSet<_> = handles
            .into_iter()
            .flat_map(|h| h.join().expect("ID allocator thread"))
            .collect();
        assert_eq!(ids.len(), 6000);
    }

    #[test]
    fn composing_definitions_uses_content_identity_not_closure_pointer_identity() {
        let desc = TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm);
        let id = TextureId::new();
        let mut a = ResourcePool::default();
        let mut b = ResourcePool::default();
        a.insert_texture(TextureSource::with_id(id, desc, |_| Ok(())))
            .expect("first definition");
        b.insert_texture(TextureSource::with_id(id, desc, |_| Ok(())))
            .expect("same content reconstructed independently");
        a.import(&b)
            .expect("caller promises same logical content for this ID");
        assert_eq!(a.len(), 1);
        assert!(
            a.insert_texture(TextureSource::with_id(id, desc, |_| Ok(())))
                .is_err(),
            "direct duplicate submissions remain errors"
        );
    }

    #[test]
    fn conflicting_import_leaves_all_resource_types_unchanged() {
        let mut destination = ResourcePool::default();
        let mut incoming = ResourcePool::default();
        let desc = TextureDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm);
        let mask = MaskSource::new(desc, |_| panic!("registration must not prepare resources"));
        let mask_id = mask.id();
        destination.insert_mask(mask).expect("original mask");
        incoming
            .insert_mask(MaskSource::with_id(
                mask_id,
                TextureDescriptor::new([2, 2], desc.format),
                |_| panic!("registration must not prepare resources"),
            ))
            .expect("conflicting definition in a separate pool");
        let mesh_id = incoming
            .insert_mesh(MeshSource::new(MeshDescriptor::triangles(3, 0), |_| {
                panic!("registration must not prepare resources")
            }))
            .expect("new mesh");

        assert!(destination.import(&incoming).is_err());
        assert_eq!(destination.len(), 1);
        assert!(destination.mesh(mesh_id).is_none());
        assert_eq!(
            *destination
                .mask(mask_id)
                .expect("original mask")
                .descriptor(),
            desc
        );
    }

    #[test]
    fn sharing_and_retention_do_not_prepare_or_invalidate_provider_definitions() {
        let source = MeshSource::new(MeshDescriptor::triangles(3, 0), |_| {
            panic!("pool operations must not prepare resources")
        });
        let mut pool = ResourcePool::default();
        assert_eq!(pool.share_mesh(&source).expect("first share"), source.id());
        pool.share_mesh(&source).expect("repeated share");
        assert_eq!(pool.mesh_ids().collect::<Vec<_>>(), vec![source.id()]);
        pool.retain_meshes(|_| false);
        assert!(pool.is_empty());
        pool.share_mesh(&source)
            .expect("provider definition remains reusable");
        assert_eq!(pool.len(), 1);
    }
}
