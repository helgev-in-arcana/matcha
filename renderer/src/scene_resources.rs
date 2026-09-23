//! Backend-owned resident placement. Generators receive isolated logical outputs;
//! copies into these arenas are recorded *after* their commands, without CPU
//! readback. IDs never encode page/offset. Allocation leases free on eviction or
//! failed-frame rollback. Future writes follow earlier draws on the same Queue;
//! no queue.write_texture bypasses that ordering when an interval is reused.
//!
//! Pages are weakly indexed: empty pages disappear instead of becoming an
//! unbounded high-water cache. wgpu command buffers retain the actual GPU handles
//! of submitted work even after the CPU allocation lease is dropped.
use guillotiere::{AllocId, AtlasAllocator, Size};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Weak},
};

#[derive(Clone, Copy, Debug)]
pub struct AtlasConfig {
    pub texture_edge: u32,
    pub mesh_page_bytes: u64,
}
impl Default for AtlasConfig {
    fn default() -> Self {
        Self {
            texture_edge: 1024,
            mesh_page_bytes: 256 * 1024,
        }
    }
}
#[derive(Default, Clone, Copy, Debug)]
pub struct PlacementStats {
    pub texture_pages: usize,
    pub texture_bytes: u64,
    pub mesh_pages: usize,
    pub mesh_bytes: u64,
}

pub struct TexturePage {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    allocator: Mutex<AtlasAllocator>,
    size: [u32; 2],
}
pub struct TextureSlot {
    pub page: Arc<TexturePage>,
    pub origin: [u32; 2],
    pub size: [u32; 2],
    id: AllocId,
}
impl TextureSlot {
    pub fn uv(&self) -> [f32; 4] {
        [
            self.origin[0] as f32 / self.page.size[0] as f32,
            self.origin[1] as f32 / self.page.size[1] as f32,
            self.size[0] as f32 / self.page.size[0] as f32,
            self.size[1] as f32 / self.page.size[1] as f32,
        ]
    }
}
impl Drop for TextureSlot {
    fn drop(&mut self) {
        self.page.allocator.lock().deallocate(self.id);
    }
}
pub struct BufferPage {
    pub buffer: wgpu::Buffer,
    free: Mutex<Vec<Range<u64>>>,
}
pub struct BufferSlot {
    pub page: Arc<BufferPage>,
    pub range: Range<u64>,
}
impl Drop for BufferSlot {
    fn drop(&mut self) {
        let mut free = self.page.free.lock();
        free.push(self.range.clone());
        free.sort_unstable_by_key(|r| r.start);
        let mut i = 0;
        while i + 1 < free.len() {
            if free[i].end == free[i + 1].start {
                free[i].end = free[i + 1].end;
                free.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }
}
pub struct Arenas {
    config: AtlasConfig,
    textures: HashMap<wgpu::TextureFormat, Vec<Weak<TexturePage>>>,
    buffers: Vec<Weak<BufferPage>>,
}
impl Arenas {
    pub fn new(config: AtlasConfig) -> Self {
        Self {
            config,
            textures: HashMap::new(),
            buffers: Vec::new(),
        }
    }
    pub fn texture(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: [u32; 2],
    ) -> TextureSlot {
        let pages = self.textures.entry(format).or_default();
        pages.retain(|p| p.strong_count() > 0);
        for weak in pages.iter() {
            if let Some(page) = weak.upgrade() {
                let allocation = page
                    .allocator
                    .lock()
                    .allocate(Size::new(size[0] as i32, size[1] as i32));
                if let Some(a) = allocation {
                    return TextureSlot {
                        page,
                        origin: [a.rectangle.min.x as u32, a.rectangle.min.y as u32],
                        size,
                        id: a.id,
                    };
                }
            }
        }
        // Oversized resources use an appropriately sized page, not a hard atlas
        // dimension rejection. No implicit rescaling or format conversion.
        let page_size = [
            self.config.texture_edge.max(size[0]),
            self.config.texture_edge.max(size[1]),
        ];
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene resident texture page"),
            size: wgpu::Extent3d {
                width: page_size[0],
                height: page_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut allocator =
            AtlasAllocator::new(Size::new(page_size[0] as i32, page_size[1] as i32));
        let a = allocator
            .allocate(Size::new(size[0] as i32, size[1] as i32))
            .expect("new page contains requested dimensions");
        let page = Arc::new(TexturePage {
            texture,
            view,
            allocator: Mutex::new(allocator),
            size: page_size,
        });
        pages.push(Arc::downgrade(&page));
        TextureSlot {
            page,
            origin: [a.rectangle.min.x as u32, a.rectangle.min.y as u32],
            size,
            id: a.id,
        }
    }
    pub fn buffer(&mut self, device: &wgpu::Device, size: u64) -> BufferSlot {
        let size = size.next_multiple_of(4);
        self.buffers.retain(|p| p.strong_count() > 0);
        for weak in &self.buffers {
            if let Some(page) = weak.upgrade() {
                let range = {
                    let mut free = page.free.lock();
                    let found = free.iter().position(|r| r.end - r.start >= size);
                    found.map(|i| {
                        let range = free[i].start..free[i].start + size;
                        free[i].start += size;
                        if free[i].is_empty() {
                            free.remove(i);
                        }
                        range
                    })
                };
                if let Some(range) = range {
                    return BufferSlot { page, range };
                }
            }
        }
        let capacity = self.config.mesh_page_bytes.max(size).next_multiple_of(4);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene resident mesh page"),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::INDEX
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let page = Arc::new(BufferPage {
            buffer,
            free: Mutex::new(if size < capacity {
                vec![size..capacity]
            } else {
                Vec::new()
            }),
        });
        self.buffers.push(Arc::downgrade(&page));
        BufferSlot {
            page,
            range: 0..size,
        }
    }
    pub fn stats(&self) -> PlacementStats {
        let mut stats = PlacementStats::default();
        for (format, pages) in &self.textures {
            for page in pages.iter().filter_map(Weak::upgrade) {
                stats.texture_pages += 1;
                stats.texture_bytes += u64::from(page.size[0])
                    * u64::from(page.size[1])
                    * u64::from(
                        format
                            .block_copy_size(None)
                            .expect("uncompressed resident formats"),
                    );
            }
        }
        for page in self.buffers.iter().filter_map(Weak::upgrade) {
            stats.mesh_pages += 1;
            stats.mesh_bytes += page.buffer.size();
        }
        stats
    }
}
