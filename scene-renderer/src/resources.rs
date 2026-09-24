//! GPU residency keyed by immutable content IDs. Definitions remain in the borrowed Scene.
//! New entries are provisional until submission. A CPU recording failure discards every
//! entry produced by that recording, leaving previously submitted content intact.
use render_interface::*;
use std::{collections::HashMap, ops::Range};

use crate::{RenderStats, SceneError};
pub(crate) mod placement;
use placement::{AtlasConfig, BufferLease, Placement, PlacementStats, TextureLease};

/// Dedicated storage is a correctness/reference path and a fallback for callers
/// whose workload does not benefit from atlas packing. Neither mode changes IDs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PlacementMode {
    Dedicated,
    #[default]
    Atlas,
}
pub(crate) struct Entry<T> {
    pub(crate) value: T,
    pub(crate) bytes: u64,
    pub(crate) last_used: u64,
    pub(crate) created_frame: u64,
}
pub(crate) struct Mesh {
    pub(crate) desc: MeshDescriptor,
    pub(crate) vertices: wgpu::Buffer,
    pub(crate) indices: Option<wgpu::Buffer>,
    pub(crate) vertex_range: Range<u64>,
    pub(crate) index_range: Range<u64>,
    pub(crate) vertex_lease: Option<BufferLease>,
    pub(crate) index_lease: Option<BufferLease>,
}
pub(crate) struct Image {
    pub(crate) desc: TextureDescriptor,
    pub(crate) texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) uv: [f32; 4],
    pub(crate) texture_lease: Option<TextureLease>,
}
impl Image {
    pub(crate) fn target(&self) -> TextureTarget<'_> {
        TextureTarget {
            desc: &self.desc,
            texture: &self.texture,
            view: &self.view,
        }
    }
    pub(crate) fn bytes(&self) -> u64 {
        u64::from(self.desc.size[0])
            * u64::from(self.desc.size[1])
            * match self.desc.format {
                wgpu::TextureFormat::R8Unorm => 1,
                wgpu::TextureFormat::Rgba16Float => 8,
                _ => 4,
            }
    }
}
pub(crate) fn extent([width, height]: [u32; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}
pub(crate) fn make_image(device: &wgpu::Device, desc: TextureDescriptor) -> Image {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene image"),
        size: extent(desc.size),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: desc.format,
        usage: desc.usages
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    Image {
        desc,
        texture,
        view,
        uv: [0., 0., 1., 1.],
        texture_lease: None,
    }
}
pub(crate) struct ResourceStore {
    pub(crate) meshes: HashMap<MeshId, Entry<Mesh>>,
    pub(crate) textures: HashMap<TextureId, Entry<Image>>,
    pub(crate) masks: HashMap<MaskId, Entry<Image>>,
    device: wgpu::Device,
    frame: u64,
    pub(crate) stats: RenderStats,
    placement: Placement,
    config: AtlasConfig,
    mode: PlacementMode,
}
impl ResourceStore {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let config = AtlasConfig {
            texture_edge: 1024.min(device.limits().max_texture_dimension_2d),
            mesh_page_bytes: (256 * 1024).min(device.limits().max_buffer_size) & !3,
        };
        Self {
            meshes: HashMap::new(),
            textures: HashMap::new(),
            masks: HashMap::new(),
            device: device.clone(),
            frame: 0,
            stats: RenderStats::default(),
            placement: Placement::new(config),
            config,
            mode: PlacementMode::Atlas,
        }
    }
    pub(crate) fn begin(&mut self) {
        self.frame = self
            .frame
            .checked_add(1)
            .expect("renderer frame sequence exhausted");
        self.stats = RenderStats::default();
    }
    pub(crate) fn abort(&mut self) {
        let frame = self.frame;
        let meshes: Vec<_> = self
            .meshes
            .extract_if(|_, e| e.created_frame == frame)
            .map(|(_, e)| e.value)
            .collect();
        for mesh in meshes {
            self.release_mesh(mesh);
        }
        let images: Vec<_> = self
            .textures
            .extract_if(|_, e| e.created_frame == frame)
            .map(|(_, e)| e.value)
            .chain(
                self.masks
                    .extract_if(|_, e| e.created_frame == frame)
                    .map(|(_, e)| e.value),
            )
            .collect();
        for image in images {
            self.release_image(image);
        }
    }
    pub(crate) fn clear(&mut self) {
        self.meshes.clear();
        self.textures.clear();
        self.masks.clear();
        // Clearing all resident owners permits dropping the complete registry.
        // Submitted command buffers still retain their own GPU handles.
        self.placement = Placement::new(self.config);
    }
    pub(crate) fn set_mode(&mut self, mode: PlacementMode) {
        if mode != self.mode {
            self.clear();
            self.mode = mode;
        }
    }
    pub(crate) fn set_config(&mut self, config: AtlasConfig) -> Result<(), SceneError> {
        placement::validate_config(&self.device, config)?;
        self.config = config;
        self.clear();
        Ok(())
    }
    pub(crate) fn placement_stats(&self) -> PlacementStats {
        let mut stats = self.placement.stats();
        for e in self.textures.values().chain(self.masks.values()) {
            if e.value.texture_lease.is_none() {
                stats.texture_pages += 1;
                stats.reserved_texture_bytes += e.bytes;
                stats.live_texture_bytes += e.bytes;
            }
        }
        for e in self.meshes.values() {
            if e.value.vertex_lease.is_none() {
                stats.mesh_pages += 1 + usize::from(e.value.indices.is_some());
                stats.reserved_mesh_bytes += e.bytes;
                stats.live_mesh_bytes += e.bytes;
            }
        }
        stats
    }
    fn release_image(&mut self, image: Image) {
        if let Some(lease) = image.texture_lease {
            self.placement
                .release_texture(lease)
                .expect("resident image owns a live placement lease");
        }
    }
    fn release_mesh(&mut self, mesh: Mesh) {
        for lease in [mesh.vertex_lease, mesh.index_lease].into_iter().flatten() {
            self.placement
                .release_buffer(lease)
                .expect("resident mesh owns a live placement lease");
        }
    }
    pub(crate) fn cache_bytes(&self) -> u64 {
        self.meshes.values().map(|e| e.bytes).sum::<u64>()
            + self.textures.values().map(|e| e.bytes).sum::<u64>()
            + self.masks.values().map(|e| e.bytes).sum::<u64>()
    }
}
impl ResourceStore {
    fn pack_image(
        &mut self,
        image: Image,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<Image, SceneError> {
        if self.mode == PlacementMode::Dedicated {
            return Ok(image);
        }
        let lease = self
            .placement
            .texture(&self.device, image.desc.format, image.desc.size)?;
        let mut source = image.texture.as_image_copy();
        if let Some(old) = &image.texture_lease {
            source.origin = wgpu::Origin3d {
                x: old.origin[0],
                y: old.origin[1],
                z: 0,
            };
        }
        let mut destination = lease.texture.as_image_copy();
        destination.origin = wgpu::Origin3d {
            x: lease.origin[0],
            y: lease.origin[1],
            z: 0,
        };
        encoder.copy_texture_to_texture(source, destination, extent(image.desc.size));
        Ok(Image {
            desc: image.desc,
            texture: lease.texture.clone(),
            view: lease.view.clone(),
            uv: lease.uv(),
            texture_lease: Some(lease),
        })
    }
    fn pack_mesh(
        &mut self,
        mesh: Mesh,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<Mesh, SceneError> {
        if self.mode == PlacementMode::Dedicated {
            return Ok(mesh);
        }
        let vertex_lease = self.placement.buffer(
            &self.device,
            mesh.vertex_range.end - mesh.vertex_range.start,
        )?;
        let index_lease = if mesh.indices.is_some() {
            match self
                .placement
                .buffer(&self.device, mesh.index_range.end - mesh.index_range.start)
            {
                Ok(lease) => Some(lease),
                Err(error) => {
                    self.placement
                        .release_buffer(vertex_lease)
                        .expect("unpublished vertex lease is live");
                    return Err(error);
                }
            }
        } else {
            None
        };
        encoder.copy_buffer_to_buffer(
            &mesh.vertices,
            mesh.vertex_range.start,
            &vertex_lease.buffer,
            vertex_lease.range.start,
            vertex_lease.range.end - vertex_lease.range.start,
        );
        if let (Some(source), Some(lease)) = (&mesh.indices, &index_lease) {
            encoder.copy_buffer_to_buffer(
                source,
                mesh.index_range.start,
                &lease.buffer,
                lease.range.start,
                lease.range.end - lease.range.start,
            );
        }
        Ok(Mesh {
            desc: mesh.desc,
            vertices: vertex_lease.buffer.clone(),
            indices: index_lease.as_ref().map(|lease| lease.buffer.clone()),
            vertex_range: vertex_lease.range.clone(),
            index_range: index_lease
                .as_ref()
                .map_or(0..0, |lease| lease.range.clone()),
            vertex_lease: Some(vertex_lease),
            index_lease,
        })
    }
    pub(crate) fn prepare_mesh(
        &mut self,
        scene: &Scene,
        id: MeshId,
        encoder: &mut wgpu::CommandEncoder,
        snapshot: RenderSnapshot<'_>,
    ) -> Result<(), SceneError> {
        if let Some(entry) = self.meshes.get_mut(&id) {
            entry.last_used = self.frame;
            self.stats.cache_hits += 1;
            return Ok(());
        }
        let source = scene.resources.mesh(id).expect("scene was validated");
        let desc = *source.descriptor();
        let vertices = self.take_output_buffer(
            u64::from(desc.vertex_count) * 20,
            desc.usages | wgpu::BufferUsages::VERTEX,
        );
        let indices = (desc.index_count != 0).then(|| {
            self.take_output_buffer(
                u64::from(desc.index_count) * 4,
                desc.usages | wgpu::BufferUsages::INDEX,
            )
        });
        source
            .prepare(MeshPrepareContext {
                gpu: GpuPrepareContext {
                    device: &self.device,
                    encoder,
                    snapshot,
                },
                target: MeshTarget {
                    desc: &desc,
                    vertices: &vertices,
                    indices: indices.as_ref(),
                },
            })
            .map_err(|source| SceneError::Prepare {
                id: id.get(),
                source,
            })?;
        let bytes = u64::from(desc.vertex_count) * 20 + u64::from(desc.index_count) * 4;
        let vertex_range = 0..vertices.size();
        let index_range = indices.as_ref().map_or(0..0, |b| 0..b.size());
        let mesh = self.pack_mesh(
            Mesh {
                desc,
                vertices,
                indices,
                vertex_range,
                index_range,
                vertex_lease: None,
                index_lease: None,
            },
            encoder,
        )?;
        self.meshes.insert(
            id,
            Entry {
                value: mesh,
                bytes,
                last_used: self.frame,
                created_frame: self.frame,
            },
        );
        self.stats.prepared += 1;
        Ok(())
    }
    pub(crate) fn prepare_texture(
        &mut self,
        scene: &Scene,
        id: TextureId,
        encoder: &mut wgpu::CommandEncoder,
        snapshot: RenderSnapshot<'_>,
    ) -> Result<(), SceneError> {
        if let Some(entry) = self.textures.get_mut(&id) {
            entry.last_used = self.frame;
            self.stats.cache_hits += 1;
            return Ok(());
        }
        let source = scene.resources.texture(id).expect("scene was validated");
        let image = self.take_output_image(*source.descriptor());
        source
            .prepare(TexturePrepareContext {
                gpu: GpuPrepareContext {
                    device: &self.device,
                    encoder,
                    snapshot,
                },
                target: image.target(),
            })
            .map_err(|source| SceneError::Prepare {
                id: id.get(),
                source,
            })?;
        let image = self.pack_image(image, encoder)?;
        self.textures.insert(
            id,
            Entry {
                bytes: image.bytes(),
                value: image,
                last_used: self.frame,
                created_frame: self.frame,
            },
        );
        self.stats.prepared += 1;
        Ok(())
    }
    pub(crate) fn prepare_mask(
        &mut self,
        scene: &Scene,
        id: MaskId,
        encoder: &mut wgpu::CommandEncoder,
        snapshot: RenderSnapshot<'_>,
    ) -> Result<(), SceneError> {
        if let Some(entry) = self.masks.get_mut(&id) {
            entry.last_used = self.frame;
            self.stats.cache_hits += 1;
            return Ok(());
        }
        let source = scene.resources.mask(id).expect("scene was validated");
        let image = self.take_output_image(*source.descriptor());
        source
            .prepare(MaskPrepareContext {
                gpu: GpuPrepareContext {
                    device: &self.device,
                    encoder,
                    snapshot,
                },
                target: image.target(),
            })
            .map_err(|source| SceneError::Prepare {
                id: id.get(),
                source,
            })?;
        let image = self.pack_image(image, encoder)?;
        self.masks.insert(
            id,
            Entry {
                bytes: image.bytes(),
                value: image,
                last_used: self.frame,
                created_frame: self.frame,
            },
        );
        self.stats.prepared += 1;
        Ok(())
    }
    fn take_output_image(&mut self, desc: TextureDescriptor) -> Image {
        self.stats.output_texture_allocations += 1;
        make_image(&self.device, desc)
    }
    fn take_output_buffer(&mut self, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.stats.output_buffer_allocations += 1;
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dedicated source output"),
            size,
            usage: usage | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}
