//! Renderer-owned GPU pages and their CPU allocation metadata.
//!
//! This layer places bytes and texels, without content IDs, cache policy, queue
//! writes or submission. Its owner explicitly releases each lease only after
//! recording/submission order makes reuse safe. Dropping a lease alone does not
//! free an allocation. Cloned GPU handles can keep a page physically alive after
//! removal here; statistics describe managed pages, not completed GPU lifetimes.
//!
//! Empty pages are removed, and vacant registry slots are reused. A stale lease
//! cannot release an allocation in a replacement page: allocator identities are
//! independent of registry slot indices.

mod allocator;

use std::ops::Range;

use allocator::{
    AllocationError, RangeAllocation, RangeAllocator, RangeToken, RectangleAllocation,
    RectangleAllocator, RectangleToken,
};

use crate::SceneError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasConfig {
    /// Preferred page edge; oversized resources receive a larger page unchanged.
    pub texture_edge: u32,
    /// Preferred mesh page capacity, positive and a multiple of four bytes.
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PlacementStats {
    pub texture_pages: usize,
    pub mesh_pages: usize,
    pub reserved_texture_bytes: u64,
    pub reserved_mesh_bytes: u64,
    pub live_texture_bytes: u64,
    /// Allocated ranges include their four-byte size rounding. This differs
    /// from source logical byte counts if a caller requests an unaligned size.
    pub live_mesh_bytes: u64,
}

/// Linear ownership of a placement, not an automatically releasing RAII guard.
/// GPU handles may be cloned; the allocation token cannot be cloned through
/// this API. Release through the owning Placement when ordering permits reuse.
#[must_use = "release the texture lease explicitly when its allocation is no longer needed"]
pub(crate) struct TextureLease {
    pub(crate) texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) origin: [u32; 2],
    pub(crate) size: [u32; 2],
    pub(crate) page_size: [u32; 2],
    page: usize,
    token: RectangleToken,
}

impl TextureLease {
    pub(crate) fn uv(&self) -> [f32; 4] {
        [
            self.origin[0] as f32 / self.page_size[0] as f32,
            self.origin[1] as f32 / self.page_size[1] as f32,
            self.size[0] as f32 / self.page_size[0] as f32,
            self.size[1] as f32 / self.page_size[1] as f32,
        ]
    }
}

#[must_use = "release the buffer lease explicitly when its allocation is no longer needed"]
pub(crate) struct BufferLease {
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) range: Range<u64>,
    page: usize,
    token: RangeToken,
}

struct TexturePage {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    format: wgpu::TextureFormat,
    size: [u32; 2],
    bytes_per_texel: u64,
    capacity_bytes: u64,
    allocator: RectangleAllocator,
}

impl TexturePage {
    fn lease(&self, page: usize, allocation: RectangleAllocation) -> TextureLease {
        TextureLease {
            texture: self.texture.clone(),
            view: self.view.clone(),
            origin: allocation.origin,
            size: allocation.extent,
            page_size: self.size,
            page,
            token: allocation.token,
        }
    }
}

struct BufferPage {
    buffer: wgpu::Buffer,
    allocator: RangeAllocator,
}

impl BufferPage {
    fn lease(&self, page: usize, allocation: RangeAllocation) -> BufferLease {
        BufferLease {
            buffer: self.buffer.clone(),
            range: allocation.range,
            page,
            token: allocation.token,
        }
    }
}

pub(crate) struct Placement {
    config: AtlasConfig,
    textures: Vec<Option<TexturePage>>,
    buffers: Vec<Option<BufferPage>>,
}

fn invalid(message: impl Into<String>) -> SceneError {
    SceneError::Invalid(message.into())
}

fn allocation_error(error: AllocationError) -> SceneError {
    invalid(format!("placement allocation: {error}"))
}

fn aligned_buffer_size(bytes: u64) -> Result<u64, SceneError> {
    if bytes == 0 {
        return Err(invalid("placement buffer size must be nonzero"));
    }
    bytes
        .checked_add(3)
        .map(|size| size & !3)
        .ok_or_else(|| invalid("placement buffer size overflows four-byte alignment"))
}

pub(crate) fn validate_config(
    device: &wgpu::Device,
    config: AtlasConfig,
) -> Result<(), SceneError> {
    let limits = device.limits();
    if config.texture_edge == 0
        || config.texture_edge > limits.max_texture_dimension_2d
        || config.texture_edge > i32::MAX as u32
    {
        return Err(invalid(
            "atlas edge is zero or exceeds texture/allocator limits",
        ));
    }
    if aligned_buffer_size(config.mesh_page_bytes)? > limits.max_buffer_size {
        return Err(invalid("mesh page exceeds the device buffer limit"));
    }
    Ok(())
}

fn insert_page<T>(pages: &mut Vec<Option<T>>, page: T) -> usize {
    if let Some(index) = pages.iter().position(Option::is_none) {
        pages[index] = Some(page);
        index
    } else {
        pages.push(Some(page));
        pages.len() - 1
    }
}

fn remove_page<T>(pages: &mut Vec<Option<T>>, index: usize) {
    pages[index] = None;
    while pages.last().is_some_and(Option::is_none) {
        pages.pop();
    }
}

impl Placement {
    /// Config is checked against the allocation device before any GPU creation.
    /// One Placement must stay on a single device, as enforced by ResourceStore.
    pub(crate) fn new(config: AtlasConfig) -> Self {
        Self {
            config,
            textures: Vec::new(),
            buffers: Vec::new(),
        }
    }

    pub(crate) fn texture(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: [u32; 2],
    ) -> Result<TextureLease, SceneError> {
        validate_config(device, self.config)?;
        if size.iter().any(|&dimension| {
            dimension == 0
                || dimension > device.limits().max_texture_dimension_2d
                || dimension > i32::MAX as u32
        }) {
            return Err(invalid(
                "texture placement size is zero or exceeds device limits",
            ));
        }
        // The caller checks sampled-format capabilities and semantics. This
        // layer only requires uncompressed texels for packing and accounting.
        if format.block_dimensions() != (1, 1) || format.is_depth_stencil_format() {
            return Err(invalid(
                "texture placement requires an uncompressed colour format",
            ));
        }
        let bytes_per_texel = u64::from(
            format
                .block_copy_size(None)
                .ok_or_else(|| invalid("texture placement format has no colour copy size"))?,
        );
        for (index, page) in self.textures.iter_mut().enumerate() {
            let Some(page) = page else { continue };
            if page.format != format {
                continue;
            }
            match page.allocator.allocate(size) {
                Ok(allocation) => return Ok(page.lease(index, allocation)),
                Err(AllocationError::OutOfSpace) => {}
                Err(error) => return Err(allocation_error(error)),
            }
        }
        // A wide, one-pixel-high image must not reserve an entire normal page
        // height. Oversized images occupy an exact-size dedicated page.
        let page_size = if size.iter().any(|&edge| edge > self.config.texture_edge) {
            size
        } else {
            [self.config.texture_edge; 2]
        };
        let capacity_bytes = u64::from(page_size[0])
            .checked_mul(u64::from(page_size[1]))
            .and_then(|area| area.checked_mul(bytes_per_texel))
            .ok_or_else(|| invalid("texture page capacity overflows byte accounting"))?;
        let mut allocator = RectangleAllocator::new(page_size).map_err(allocation_error)?;
        let allocation = allocator.allocate(size).map_err(allocation_error)?;
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
        let page = TexturePage {
            texture,
            view,
            format,
            size: page_size,
            bytes_per_texel,
            capacity_bytes,
            allocator,
        };
        let index = insert_page(&mut self.textures, page);
        Ok(self.textures[index]
            .as_ref()
            .expect("the newly inserted texture page occupies this index")
            .lease(index, allocation))
    }

    pub(crate) fn buffer(
        &mut self,
        device: &wgpu::Device,
        size: u64,
    ) -> Result<BufferLease, SceneError> {
        validate_config(device, self.config)?;
        let size = aligned_buffer_size(size)?;
        if size > device.limits().max_buffer_size {
            return Err(invalid("mesh allocation exceeds the device buffer limit"));
        }
        for (index, page) in self.buffers.iter_mut().enumerate() {
            let Some(page) = page else { continue };
            match page.allocator.allocate(size, 4) {
                Ok(allocation) => return Ok(page.lease(index, allocation)),
                Err(AllocationError::OutOfSpace) => {}
                Err(error) => return Err(allocation_error(error)),
            }
        }
        let capacity = aligned_buffer_size(self.config.mesh_page_bytes)?.max(size);
        let mut allocator = RangeAllocator::new(capacity).map_err(allocation_error)?;
        let allocation = allocator.allocate(size, 4).map_err(allocation_error)?;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene resident mesh page"),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::INDEX
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let index = insert_page(&mut self.buffers, BufferPage { buffer, allocator });
        Ok(self.buffers[index]
            .as_ref()
            .expect("the newly inserted buffer page occupies this index")
            .lease(index, allocation))
    }

    pub(crate) fn release_texture(&mut self, lease: TextureLease) -> Result<(), SceneError> {
        let page = self
            .textures
            .get_mut(lease.page)
            .and_then(Option::as_mut)
            .ok_or_else(|| invalid("texture lease refers to a released page"))?;
        page.allocator.free(lease.token).map_err(allocation_error)?;
        if page.allocator.stats().allocations == 0 {
            remove_page(&mut self.textures, lease.page);
        }
        Ok(())
    }

    pub(crate) fn release_buffer(&mut self, lease: BufferLease) -> Result<(), SceneError> {
        let page = self
            .buffers
            .get_mut(lease.page)
            .and_then(Option::as_mut)
            .ok_or_else(|| invalid("buffer lease refers to a released page"))?;
        page.allocator.free(lease.token).map_err(allocation_error)?;
        if page.allocator.stats().allocations == 0 {
            remove_page(&mut self.buffers, lease.page);
        }
        Ok(())
    }

    pub(crate) fn stats(&self) -> PlacementStats {
        let mut stats = PlacementStats::default();
        for page in self.textures.iter().flatten() {
            stats.texture_pages += 1;
            stats.reserved_texture_bytes = stats
                .reserved_texture_bytes
                .saturating_add(page.capacity_bytes);
            stats.live_texture_bytes = stats
                .live_texture_bytes
                .saturating_add(page.allocator.stats().used_area * page.bytes_per_texel);
        }
        for page in self.buffers.iter().flatten() {
            let page_stats = page.allocator.stats();
            stats.mesh_pages += 1;
            stats.reserved_mesh_bytes = stats
                .reserved_mesh_bytes
                .saturating_add(page_stats.capacity_bytes);
            stats.live_mesh_bytes = stats.live_mesh_bytes.saturating_add(page_stats.used_bytes);
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> wgpu::Device {
        futures::executor::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::NOOP,
                backend_options: wgpu::BackendOptions {
                    noop: wgpu::NoopBackendOptions { enable: true },
                    ..Default::default()
                },
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance
                .request_adapter(&Default::default())
                .await
                .expect("noop adapter exists without a GPU");
            adapter
                .request_device(&Default::default())
                .await
                .expect("noop device creation succeeds")
                .0
        })
    }

    fn config() -> AtlasConfig {
        AtlasConfig {
            texture_edge: 8,
            mesh_page_bytes: 32,
        }
    }

    #[test]
    fn texture_pages_pack_by_format_and_track_live_vs_reserved_bytes() {
        let device = device();
        let mut placement = Placement::new(config());
        let a = placement
            .texture(&device, wgpu::TextureFormat::Rgba8Unorm, [4, 8])
            .expect("fits");
        let b = placement
            .texture(&device, wgpu::TextureFormat::Rgba8Unorm, [4, 8])
            .expect("fits beside a");
        let mask = placement
            .texture(&device, wgpu::TextureFormat::R8Unorm, [2, 2])
            .expect("separate format");
        assert_eq!(a.texture, b.texture);
        assert_ne!(a.texture, mask.texture);
        assert_ne!(a.origin, b.origin);
        assert_eq!(a.uv()[2..], [0.5, 1.0]);
        let stats = placement.stats();
        assert_eq!(stats.texture_pages, 2);
        assert_eq!(stats.reserved_texture_bytes, 320);
        assert_eq!(stats.live_texture_bytes, 260);
        placement.release_texture(a).expect("live allocation");
        assert_eq!(placement.stats().reserved_texture_bytes, 320);
        assert_eq!(placement.stats().live_texture_bytes, 132);
        placement.release_texture(b).expect("last RGBA allocation");
        assert_eq!(placement.stats().reserved_texture_bytes, 64);
        placement
            .release_texture(mask)
            .expect("last mask allocation");
        assert_eq!(placement.stats(), PlacementStats::default());
        assert!(placement.textures.is_empty());
    }

    #[test]
    fn oversized_thin_images_do_not_inherit_the_normal_page_height() {
        let device = device();
        let mut placement = Placement::new(config());
        let lease = placement
            .texture(&device, wgpu::TextureFormat::Rgba8Unorm, [65, 1])
            .expect("valid thin image");
        assert_eq!(lease.page_size, [65, 1]);
        assert_eq!(placement.stats().reserved_texture_bytes, 65 * 4);
        assert_eq!(placement.stats().live_texture_bytes, 65 * 4);
        placement.release_texture(lease).expect("live allocation");
        assert_eq!(placement.stats(), PlacementStats::default());
    }

    #[test]
    fn buffer_rounding_reuse_and_oversized_pages_preserve_requested_data_range() {
        let device = device();
        let mut placement = Placement::new(config());
        let a = placement.buffer(&device, 5).expect("round to 8");
        let b = placement.buffer(&device, 4).expect("same page");
        assert_eq!(a.range, 0..8);
        assert_eq!(b.range, 8..12);
        assert_eq!(a.buffer, b.buffer);
        placement.release_buffer(a).expect("first allocation");
        let c = placement.buffer(&device, 8).expect("reuses first interval");
        assert_eq!(c.range, 0..8);
        let big = placement.buffer(&device, 65).expect("oversized page");
        assert_eq!(big.range, 0..68);
        assert_eq!(big.buffer.size(), 68);
        assert_eq!(placement.stats().reserved_mesh_bytes, 100);
        assert_eq!(placement.stats().live_mesh_bytes, 80);
        placement.release_buffer(b).expect("live allocation");
        placement.release_buffer(c).expect("live allocation");
        placement.release_buffer(big).expect("live allocation");
        assert_eq!(placement.stats(), PlacementStats::default());
        assert!(placement.buffers.is_empty());
    }

    #[test]
    fn oversized_texture_uses_actual_dimensions_without_rescaling() {
        let device = device();
        let mut placement = Placement::new(config());
        let image = placement
            .texture(&device, wgpu::TextureFormat::Rgba16Float, [13, 3])
            .expect("dedicated sized page");
        assert_eq!(image.size, [13, 3]);
        assert_eq!(image.page_size, [13, 3]);
        assert_eq!(placement.stats().reserved_texture_bytes, 13 * 3 * 8);
        assert_eq!(placement.stats().live_texture_bytes, 13 * 3 * 8);
        placement.release_texture(image).expect("live image");
    }

    #[test]
    fn page_registry_churn_reuses_holes_without_historical_growth() {
        let device = device();
        let mut placement = Placement::new(config());
        let first = placement
            .texture(&device, wgpu::TextureFormat::R8Unorm, [8, 8])
            .expect("page zero");
        let keeper = placement
            .texture(&device, wgpu::TextureFormat::R8Unorm, [8, 8])
            .expect("page one");
        placement.release_texture(first).expect("creates hole");
        for _ in 0..128 {
            let image = placement
                .texture(&device, wgpu::TextureFormat::R8Unorm, [8, 8])
                .expect("reuses hole");
            assert_eq!(image.page, 0);
            assert_eq!(placement.textures.len(), 2);
            placement
                .release_texture(image)
                .expect("releases page zero");
        }
        placement
            .release_texture(keeper)
            .expect("releases page one and trailing hole");
        assert!(placement.textures.is_empty());
    }

    #[test]
    fn stale_page_tokens_cannot_release_replacements() {
        let device = device();
        let mut placement = Placement::new(config());
        let old = placement.buffer(&device, 32).expect("first page");
        // Fabrication is confined to this child test module. The parent store
        // cannot clone leases or access their allocation tokens.
        let stale = BufferLease {
            buffer: old.buffer.clone(),
            range: old.range.clone(),
            page: old.page,
            token: old.token,
        };
        placement.release_buffer(old).expect("release first page");
        let new = placement
            .buffer(&device, 32)
            .expect("replacement at same registry index");
        assert_eq!(stale.page, new.page);
        assert!(placement.release_buffer(stale).is_err());
        assert_eq!(placement.stats().live_mesh_bytes, 32);
        placement
            .release_buffer(new)
            .expect("replacement is still live");
    }

    #[test]
    fn invalid_dimensions_and_configuration_do_not_create_pages() {
        let device = device();
        let mut placement = Placement::new(config());
        assert!(placement.buffer(&device, 0).is_err());
        assert!(placement.buffer(&device, u64::MAX).is_err());
        assert!(
            placement
                .texture(&device, wgpu::TextureFormat::R8Unorm, [0, 1])
                .is_err()
        );
        assert!(
            placement
                .texture(&device, wgpu::TextureFormat::R8Unorm, [u32::MAX, 1])
                .is_err()
        );
        assert_eq!(placement.stats(), PlacementStats::default());
        assert!(
            validate_config(
                &device,
                AtlasConfig {
                    texture_edge: 0,
                    ..config()
                }
            )
            .is_err()
        );
        assert!(
            validate_config(
                &device,
                AtlasConfig {
                    mesh_page_bytes: 0,
                    ..config()
                }
            )
            .is_err()
        );
        assert!(
            validate_config(
                &device,
                AtlasConfig {
                    mesh_page_bytes: u64::MAX,
                    ..config()
                }
            )
            .is_err()
        );
    }
}
