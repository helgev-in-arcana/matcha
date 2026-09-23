//! The upstream rendering contract, independent of Matcha's UI and backend.
//!
//! Deferred extension candidates (not adopted or implemented):
//! - Snapshot-dependency flag: let a source declare that it reads the snapshot;
//!   leave invalidation/regeneration policy to the renderer. The relation to
//!   immutable content IDs and snapshot versions still needs a contract.
//! - Sampler policy: explicit sampling semantics, notably nearest versus linear,
//!   independent of atlas placement. Fields and defaults remain undecided.
//! - Other shader/draw settings beyond sampler policy, such as blend policy:
//!   the useful settings, their scope and their representation remain undecided.
//! These notes do not change the current contract described below.
//!
//! A caller owns and may reuse a complete [`Scene`]. A renderer borrows it only
//! during `render`; CPU callbacks finish before that call returns, GPU work need
//! not. Sources are stored directly, not behind an additional Arc. IDs identify
//! immutable logical content, never GPU placement, revisions or drawing order.
//! A changed animation/background input requires a new ID. Regeneration with an
//! existing ID must reproduce its content, regardless of cache eviction.
//! Source::clone shares the immutable generator allocation (Arc<Prepare> in place
//! of Box<Prepare>) so provider caches and the submitted pool can share definitions.
//! Explicit pool composition deduplicates IDs; direct duplicate insertions remain
//! errors. Closure pointer equality is not a substitute for logical content identity.
//!
//! Phases and objects are composited in array order, using premultiplied linear
//! RGBA and source-over. Vertex positions are object-local, Y-down; transforms
//! map them to viewport UI pixels. UVs are normalized, clamped and linearly filtered.
//! The fixed vertex ABI is intentional: arbitrary byte layouts without shared
//! attribute semantics do not constitute an interoperable rendering interface.
//!
//! Masks have arbitrary meshes and coverage textures (red, linear [0,1]). Only
//! coverage is inherited from their parent, by multiplication; transforms are
//! absolute. Overlapping triangles of one mask take maximum coverage. No depth
//! testing, G-buffer, previous-frame history or implicit UI ancestry is promised.
//!
//! A source is prepared on its first referenced phase, including references
//! through mask ancestors. All preparations in a phase read its *start* image,
//! never earlier objects of that phase. Registration alone requests no work.
//! Definitions must exist even on a warm cache. Caches may ignore pool retention
//! hints, but must not regenerate a source from a later phase's snapshot.
//!
//! GPU callbacks may record copy/compute/render work into their dedicated output.
//! They must initialize it, never mutate the snapshot, retain borrowed handles,
//! destroy resources or submit work themselves. Errors abort the frame. Exposing
//! raw wgpu handles is a trusted extension contract, not a security sandbox.
//! A logical output need not be a fresh allocation: it can be reused after its
//! commands and placement copy have been recorded. Never retain its identity or
//! depend on previous contents. A snapshot can likewise alias the accumulation
//! image if command order puts all generation reads before that phase's writes.
//! PrepareResult reports CPU recording errors; wgpu validation/device errors use
//! wgpu's error model and are not implied absent by an Ok return.

pub use nalgebra::Matrix4;
use std::{
    collections::HashMap,
    error::Error,
    sync::atomic::{AtomicU64, Ordering},
};
pub use wgpu;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
macro_rules! content_id {
    ($($name:ident),+) => {$ (
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(u64);
        impl $name {
            pub fn new() -> Self {
                Self(NEXT_ID.fetch_update(Ordering::Relaxed, Ordering::Relaxed,
                    |v| v.checked_add(1)).expect("content ID space exhausted"))
            }
            pub fn get(self) -> u64 { self.0 }
        }
        impl Default for $name { fn default() -> Self { Self::new() } }
    )+};
}
content_id!(MeshId, TextureId, MaskId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelMaskIndex(pub u32);

#[derive(Default)]
pub struct Scene {
    pub resources: ResourcePool,
    pub phases: Vec<Phase>,
    pub pixel_masks: Vec<PixelMask>,
}
#[derive(Default, Clone)]
pub struct Phase {
    pub objects: Vec<Object>,
}
#[derive(Debug, Clone)]
pub struct Object {
    pub mesh: MeshId,
    pub texture: TextureId,
    pub transform: Matrix4<f32>,
    pub mask: Option<PixelMaskIndex>,
    /// Draw-time opacity multiplies all four premultiplied channels.
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
    pub transform: Matrix4<f32>,
    /// Must refer to a strictly preceding element of Scene.pixel_masks.
    pub parent: Option<PixelMaskIndex>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshDescriptor {
    pub vertex_count: u32,
    /// Zero selects non-indexed drawing; otherwise indices are u32.
    pub index_count: u32,
    /// Additional buffer usages, e.g. STORAGE for compute generation.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureDescriptor {
    pub size: [u32; 2],
    /// One 2D layer, one mip and one sample. Colour is premultiplied;
    /// sRGB formats store encoded RGB, linear formats store linear RGB.
    pub format: wgpu::TextureFormat,
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
pub type MaskDescriptor = TextureDescriptor;
pub type PrepareError = Box<dyn Error + Send + Sync + 'static>;
pub type PrepareResult = Result<(), PrepareError>;

#[derive(Clone, Copy)]
pub struct RenderSnapshot<'a> {
    pub color_texture: &'a wgpu::Texture,
    pub color_view: &'a wgpu::TextureView,
    pub size: [u32; 2],
    pub format: wgpu::TextureFormat,
}
pub struct GpuPrepareContext<'a> {
    pub device: &'a wgpu::Device,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub snapshot: RenderSnapshot<'a>,
}
pub struct MeshTarget<'a> {
    pub desc: &'a MeshDescriptor,
    pub vertices: &'a wgpu::Buffer,
    pub indices: Option<&'a wgpu::Buffer>,
}
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

#[derive(Default)]
pub struct ResourcePool {
    meshes: HashMap<MeshId, MeshSource>,
    textures: HashMap<TextureId, TextureSource>,
    masks: HashMap<MaskId, MaskSource>,
}
#[derive(Debug, thiserror::Error)]
#[error("duplicate resource definition {0}")]
pub struct DuplicateResource(pub u64);
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
    /// Merge references contributed by independently cached scenes under the
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
    pub fn len(&self) -> usize {
        self.meshes.len() + self.textures.len() + self.masks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A render call's destination is backend-defined; implementations may accept a
/// surface attachment or an offscreen target without putting surfaces in Scene.
pub trait Renderer {
    type Target<'a>;
    type Error: Error;
    fn render(&mut self, scene: &Scene, target: Self::Target<'_>) -> Result<(), Self::Error>;
}

/// Record a byte upload without Queue access; submit remains renderer-owned.
pub fn upload_buffer(
    gpu: &mut GpuPrepareContext<'_>,
    target: &wgpu::Buffer,
    bytes: &[u8],
) -> PrepareResult {
    use wgpu::util::DeviceExt;
    if bytes.len() as u64 > target.size() || bytes.len() % 4 != 0 {
        return Err("invalid buffer upload length/alignment".into());
    }
    let staging = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("source upload"),
            contents: bytes,
            usage: wgpu::BufferUsages::COPY_SRC,
        });
    gpu.encoder
        .copy_buffer_to_buffer(&staging, 0, target, 0, bytes.len() as u64);
    Ok(())
}
/// Tightly packed rows, padded here to WebGPU's copy alignment.
pub fn upload_texture(
    gpu: &mut GpuPrepareContext<'_>,
    target: &TextureTarget<'_>,
    bytes: &[u8],
) -> PrepareResult {
    use wgpu::util::DeviceExt;
    let bpp = match target.desc.format {
        wgpu::TextureFormat::R8Unorm => 1,
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => 4,
        wgpu::TextureFormat::Rgba16Float => 8,
        _ => return Err("unsupported upload format".into()),
    };
    let [w, h] = target.desc.size;
    let row = w.checked_mul(bpp).ok_or("row size overflow")?;
    if bytes.len() as u64 != u64::from(row) * u64::from(h) {
        return Err("invalid texture upload length".into());
    }
    let padded = row.checked_add(255).ok_or("row padding overflow")? / 256 * 256;
    let mut data = vec![0; padded as usize * h as usize];
    for y in 0..h as usize {
        data[y * padded as usize..y * padded as usize + row as usize]
            .copy_from_slice(&bytes[y * row as usize..(y + 1) * row as usize]);
    }
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
                bytes_per_row: Some(padded),
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
