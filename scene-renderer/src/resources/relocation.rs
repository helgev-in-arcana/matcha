//! Transactional relocation of resident content, without Scene definitions or
//! source callbacks. Copying logical contents changes placement, never IDs.
//!
//! A plan owns a fresh placement registry and replacement values; it only borrows
//! the old residents. The caller submits the recorded copies on the same ordered
//! Queue as earlier draws, then publishes all replacements together. New draws
//! must follow that submission. A failed/discarded plan leaves the old store valid;
//! discard its encoder too, since recorded commands retain GPU handles until drop.
//!
//! Repacking uses descending logical sizes with ID tie-breaking rather than HashMap
//! iteration order. This removes historical holes but is a heuristic, not a promise
//! of fewer pages for every rectangle set. The caller decides when fragmentation
//! merits relocation. The copy budget is checked before GPU allocation or recording.
//! Peak statistics count old plus new managed resident capacity, not driver memory,
//! scratch/working images, or resources retained by already submitted commands.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    ops::Range,
};

use render_interface::{MaskId, MeshId, TextureId};

use super::{
    Entry, Image, Mesh, PlacementMode, extent, make_image,
    placement::{self, AtlasConfig, BufferClass, Placement, PlacementStats},
};
use crate::SceneError;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RelocationStats {
    /// Colour and coverage images copied, not the number of physical pages.
    pub textures: usize,
    pub meshes: usize,
    /// Logical image bytes and mesh ranges actually copied, excluding page slack.
    pub copied_bytes: u64,
    /// Old plus replacement resident capacity while both sets coexist.
    /// This excludes scratch/working buffers and opaque driver allocations.
    /// For a Dedicated no-op this is just the current resident capacity.
    pub peak_managed_bytes: u64,
    pub placement: PlacementStats,
}

pub(crate) struct RelocationPlan {
    pub(super) placement: Placement,
    pub(super) meshes: HashMap<MeshId, Mesh>,
    pub(super) textures: HashMap<TextureId, Image>,
    pub(super) masks: HashMap<MaskId, Image>,
    pub(crate) stats: RelocationStats,
}

impl RelocationPlan {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build(
        device: &wgpu::Device,
        meshes: &HashMap<MeshId, Entry<Mesh>>,
        textures: &HashMap<TextureId, Entry<Image>>,
        masks: &HashMap<MaskId, Entry<Image>>,
        config: AtlasConfig,
        mode: PlacementMode,
        max_copy_bytes: u64,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<Self, SceneError> {
        placement::validate_config(device, config)?;
        let mut copied_bytes = 0;
        for entry in meshes.values() {
            copied_bytes =
                add_copy_bytes(copied_bytes, mesh_copy_bytes(&entry.value)?, max_copy_bytes)?;
        }
        for entry in textures.values().chain(masks.values()) {
            copied_bytes = add_copy_bytes(copied_bytes, entry.value.bytes(), max_copy_bytes)?;
        }
        let old = resident_stats(
            meshes.values().map(|entry| &entry.value),
            textures
                .values()
                .chain(masks.values())
                .map(|entry| &entry.value),
        )?;

        let mut plan = Self {
            placement: Placement::new(config),
            meshes: HashMap::with_capacity(meshes.len()),
            textures: HashMap::with_capacity(textures.len()),
            masks: HashMap::with_capacity(masks.len()),
            stats: RelocationStats::default(),
        };
        let mut ordered_meshes: Vec<_> = meshes.iter().collect();
        ordered_meshes.sort_unstable_by_key(|(id, entry)| (Reverse(entry.bytes), id.get()));
        for (&id, entry) in ordered_meshes {
            let mesh = copy_mesh(device, &mut plan.placement, mode, &entry.value, encoder)?;
            plan.meshes.insert(id, mesh);
        }
        // Both image kinds share the same format-specific placement pool.
        let mut ordered_images: Vec<_> = textures
            .iter()
            .map(|(&id, entry)| (ImageId::Texture(id), &entry.value))
            .chain(
                masks
                    .iter()
                    .map(|(&id, entry)| (ImageId::Mask(id), &entry.value)),
            )
            .collect();
        ordered_images.sort_unstable_by_key(|(id, image)| {
            let [width, height] = image.desc.size;
            (Reverse(u64::from(width) * u64::from(height)), id.order())
        });
        for (id, old_image) in ordered_images {
            let image = copy_image(device, &mut plan.placement, mode, old_image, encoder)?;
            match id {
                ImageId::Texture(id) => {
                    plan.textures.insert(id, image);
                }
                ImageId::Mask(id) => {
                    plan.masks.insert(id, image);
                }
            }
        }
        let new = resident_stats(
            plan.meshes.values(),
            plan.textures.values().chain(plan.masks.values()),
        )?;
        let peak_managed_bytes = [
            old.reserved_texture_bytes,
            old.reserved_mesh_bytes,
            new.reserved_texture_bytes,
            new.reserved_mesh_bytes,
        ]
        .into_iter()
        .try_fold(0u64, |sum, bytes| {
            sum.checked_add(bytes)
                .ok_or_else(|| invalid("relocation peak byte count overflow"))
        })?;
        plan.stats = RelocationStats {
            textures: textures.len() + masks.len(),
            meshes: meshes.len(),
            copied_bytes,
            peak_managed_bytes,
            placement: new,
        };
        Ok(plan)
    }
}

enum ImageId {
    Texture(TextureId),
    Mask(MaskId),
}
impl ImageId {
    fn order(&self) -> u64 {
        match self {
            Self::Texture(id) => id.get(),
            Self::Mask(id) => id.get(),
        }
    }
}

fn copy_image(
    device: &wgpu::Device,
    placement: &mut Placement,
    mode: PlacementMode,
    source: &Image,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<Image, SceneError> {
    let image = match mode {
        PlacementMode::Dedicated => make_image(device, source.desc),
        PlacementMode::Atlas => {
            let lease = placement.texture_with_usage(
                device,
                source.desc.format,
                source.desc.size,
                source.texture.usage(),
            )?;
            Image {
                desc: source.desc,
                texture: lease.texture.clone(),
                view: lease.view.clone(),
                uv: lease.uv(),
                texture_lease: Some(lease),
            }
        }
    };
    let mut from = source.texture.as_image_copy();
    let mut to = image.texture.as_image_copy();
    if let Some(lease) = &source.texture_lease {
        from.origin = wgpu::Origin3d {
            x: lease.origin[0],
            y: lease.origin[1],
            z: 0,
        };
    }
    if let Some(lease) = &image.texture_lease {
        to.origin = wgpu::Origin3d {
            x: lease.origin[0],
            y: lease.origin[1],
            z: 0,
        };
    }
    encoder.copy_texture_to_texture(from, to, extent(source.desc.size));
    Ok(image)
}

fn copy_mesh(
    device: &wgpu::Device,
    placement: &mut Placement,
    mode: PlacementMode,
    source: &Mesh,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<Mesh, SceneError> {
    let vertex_bytes = range_bytes(&source.vertex_range)?;
    let index_bytes = range_bytes(&source.index_range)?;
    let (vertices, vertex_range, vertex_lease) = match mode {
        PlacementMode::Dedicated => (
            buffer(
                device,
                vertex_bytes,
                source.desc.usages | wgpu::BufferUsages::VERTEX,
            ),
            0..vertex_bytes,
            None,
        ),
        PlacementMode::Atlas => {
            let class = source
                .vertex_lease
                .as_ref()
                .map_or(BufferClass::Resident, |lease| lease.class);
            let lease = placement.buffer_with_usage(
                device,
                vertex_bytes,
                source.vertices.usage(),
                class,
            )?;
            (lease.buffer.clone(), lease.range.clone(), Some(lease))
        }
    };
    let (indices, index_range, index_lease) = if let Some(source_indices) = &source.indices {
        match mode {
            PlacementMode::Dedicated => (
                Some(buffer(
                    device,
                    index_bytes,
                    source.desc.usages | wgpu::BufferUsages::INDEX,
                )),
                0..index_bytes,
                None,
            ),
            PlacementMode::Atlas => {
                let class = source
                    .index_lease
                    .as_ref()
                    .map_or(BufferClass::Resident, |lease| lease.class);
                let lease = placement.buffer_with_usage(
                    device,
                    index_bytes,
                    source_indices.usage(),
                    class,
                )?;
                (Some(lease.buffer.clone()), lease.range.clone(), Some(lease))
            }
        }
    } else {
        (None, 0..0, None)
    };
    encoder.copy_buffer_to_buffer(
        &source.vertices,
        source.vertex_range.start,
        &vertices,
        vertex_range.start,
        vertex_bytes,
    );
    if let (Some(from), Some(to)) = (&source.indices, &indices) {
        encoder.copy_buffer_to_buffer(
            from,
            source.index_range.start,
            to,
            index_range.start,
            index_bytes,
        );
    }
    Ok(Mesh {
        desc: source.desc,
        vertices,
        indices,
        vertex_range,
        index_range,
        vertex_lease,
        index_lease,
    })
}

fn buffer(device: &wgpu::Device, size: u64, usages: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("relocated dedicated mesh"),
        size,
        usage: usages | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn range_bytes(range: &Range<u64>) -> Result<u64, SceneError> {
    range
        .end
        .checked_sub(range.start)
        .ok_or_else(|| invalid("invalid resident mesh range"))
}
fn mesh_copy_bytes(mesh: &Mesh) -> Result<u64, SceneError> {
    let indices = if mesh.indices.is_some() {
        range_bytes(&mesh.index_range)?
    } else {
        0
    };
    range_bytes(&mesh.vertex_range)?
        .checked_add(indices)
        .ok_or_else(|| invalid("mesh relocation byte count overflow"))
}
fn add_copy_bytes(current: u64, bytes: u64, limit: u64) -> Result<u64, SceneError> {
    current
        .checked_add(bytes)
        .filter(|total| *total <= limit)
        .ok_or_else(|| invalid("relocation exceeds the copy byte limit"))
}

/// Count each actual page once, even when many residents share its GPU handle.
/// Empty managed pages are released by Placement, so live handles cover its pages.
#[allow(clippy::mutable_key_type)] // wgpu Hash/Eq use stable handle identity, not mutable device state.
fn resident_stats<'a>(
    meshes: impl Iterator<Item = &'a Mesh>,
    images: impl Iterator<Item = &'a Image>,
) -> Result<PlacementStats, SceneError> {
    let mut stats = PlacementStats::default();
    let mut textures = HashSet::new();
    let mut buffers = HashSet::new();
    for image in images {
        stats.live_texture_bytes = checked_sum(stats.live_texture_bytes, image.bytes())?;
        if textures.insert(&image.texture) {
            stats.texture_pages += 1;
            let bytes_per_texel = image
                .texture
                .format()
                .block_copy_size(None)
                .ok_or_else(|| invalid("resident texture has no copy byte size"))?;
            let bytes = u64::from(image.texture.width())
                .checked_mul(u64::from(image.texture.height()))
                .and_then(|area| area.checked_mul(u64::from(bytes_per_texel)))
                .ok_or_else(|| invalid("resident texture byte count overflow"))?;
            stats.reserved_texture_bytes = checked_sum(stats.reserved_texture_bytes, bytes)?;
        }
    }
    for mesh in meshes {
        stats.live_mesh_bytes = checked_sum(stats.live_mesh_bytes, mesh_copy_bytes(mesh)?)?;
        for buffer in std::iter::once(&mesh.vertices).chain(mesh.indices.as_ref()) {
            if buffers.insert(buffer) {
                stats.mesh_pages += 1;
                stats.reserved_mesh_bytes = checked_sum(stats.reserved_mesh_bytes, buffer.size())?;
            }
        }
    }
    Ok(stats)
}
fn checked_sum(left: u64, right: u64) -> Result<u64, SceneError> {
    left.checked_add(right)
        .ok_or_else(|| invalid("resident byte count overflow"))
}
fn invalid(message: &str) -> SceneError {
    SceneError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_limit_includes_the_whole_plan_and_checks_overflow() {
        assert_eq!(
            add_copy_bytes(0, 0, 0).expect("empty plan needs no budget"),
            0
        );
        let first = add_copy_bytes(0, 120, 132).expect("vertex range fits");
        assert_eq!(
            add_copy_bytes(first, 12, 132).expect("indices fit exactly"),
            132
        );
        assert!(add_copy_bytes(first, 13, 132).is_err());
        assert!(add_copy_bytes(u64::MAX, 1, u64::MAX).is_err());
    }
}
