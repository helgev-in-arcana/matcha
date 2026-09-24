//! Immutable logical resource definitions and their IDs.
//!
//! The pool owns definitions, not GPU allocations. An ID remains stable across
//! relocation and eviction; the producer guarantees identical content on reuse.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{MaskPrepareContext, MeshPrepareContext, PrepareResult, TexturePrepareContext};

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

/// Fixed triangle vertex ABI: three position floats followed by two UV floats.
/// Positions are object-local; UVs are normalized, linearly sampled and clamped.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
}
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
