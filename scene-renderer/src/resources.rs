//! GPU residency keyed by immutable content IDs. Definitions remain in the borrowed Scene.
//! New entries are provisional until submission. A CPU recording failure discards every
//! entry produced by that recording, leaving previously submitted content intact.
use render_interface::*;
use std::{collections::HashMap, ops::Range};

use crate::{RenderStats, SceneError};
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
}
pub(crate) struct Image {
    pub(crate) desc: TextureDescriptor,
    pub(crate) texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) uv: [f32; 4],
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
    }
}
pub(crate) struct ResourceStore {
    pub(crate) meshes: HashMap<MeshId, Entry<Mesh>>,
    pub(crate) textures: HashMap<TextureId, Entry<Image>>,
    pub(crate) masks: HashMap<MaskId, Entry<Image>>,
    device: wgpu::Device,
    frame: u64,
    pub(crate) stats: RenderStats,
}
impl ResourceStore {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            meshes: HashMap::new(),
            textures: HashMap::new(),
            masks: HashMap::new(),
            device: device.clone(),
            frame: 0,
            stats: RenderStats::default(),
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
        self.meshes.retain(|_, e| e.created_frame != self.frame);
        self.textures.retain(|_, e| e.created_frame != self.frame);
        self.masks.retain(|_, e| e.created_frame != self.frame);
    }
    pub(crate) fn clear(&mut self) {
        self.meshes.clear();
        self.textures.clear();
        self.masks.clear();
    }
    pub(crate) fn cache_bytes(&self) -> u64 {
        self.meshes.values().map(|e| e.bytes).sum::<u64>()
            + self.textures.values().map(|e| e.bytes).sum::<u64>()
            + self.masks.values().map(|e| e.bytes).sum::<u64>()
    }
}
impl ResourceStore {
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
        self.meshes.insert(
            id,
            Entry {
                value: Mesh {
                    desc,
                    vertices,
                    indices,
                    vertex_range,
                    index_range,
                },
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
