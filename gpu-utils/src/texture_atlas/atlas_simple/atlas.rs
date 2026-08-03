use std::collections::HashMap;
use std::sync::{Arc, Weak};

use guillotiere::euclid::Box2D;
use guillotiere::{AllocId, AtlasAllocator, Size, euclid};
use log::{trace, warn};
use parking_lot::{Mutex, RwLock};
use thiserror::Error;
use uuid::Uuid;

mod viewport_clear;
use viewport_clear::ViewportClear;

#[derive(Debug, Clone)]
pub struct AtlasRegion {
    inner: Arc<RegionData>,
}

// We only store the texture id and reference to the atlas,
// to make `Texture` remain valid after `TextureAtlas` resizes or changes,
// except for data loss when the atlas shrinks.
struct RegionData {
    // allocation info
    region_id: RegionId,
    atlas_id: TextureAtlasId,
    // interaction with the atlas
    atlas: Weak<TextureAtlas>,
    // It may be useful to store some information about the texture that will not change during atlas resizing
    allocation_size: [u32; 2], // size of the allocated area including margins
    usable_size: [u32; 2],     // size of the usable texture area excluding margins
    atlas_size: [u32; 2],      // size of the atlas when the texture was allocated
    format: wgpu::TextureFormat, // format of the texture
}

impl std::fmt::Debug for RegionData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegionData")
            .field("region_id", &self.region_id)
            .field("atlas_id", &self.atlas_id)
            .field("texture_size", &self.usable_size)
            .field("atlas_size", &self.atlas_size)
            .field("format", &self.format)
            .finish()
    }
}

/// Public API to interact with a texture.
/// User code should not need to know about its id, location, or atlas.
impl AtlasRegion {
    pub fn atlas_id(&self) -> TextureAtlasId {
        trace!(
            "AtlasRegion::atlas_id called for region={:?}",
            self.inner.region_id
        );
        self.inner.atlas_id
    }

    pub fn position_in_atlas(&self) -> Result<(u32, Box2D<f32, euclid::UnknownUnit>), RegionError> {
        trace!(
            "AtlasRegion::position_in_atlas: querying region={:?}",
            self.inner.region_id
        );
        // Get the texture location in the atlas
        let Some(atlas) = self.inner.atlas.upgrade() else {
            warn!("AtlasRegion::position_in_atlas: atlas dropped");
            return Err(RegionError::AtlasGone);
        };
        let Some(location) = atlas.get_location(self.inner.region_id) else {
            warn!("AtlasRegion::position_in_atlas: region not found in atlas");
            return Err(RegionError::TextureNotFoundInAtlas);
        };

        Ok((location.page_index, location.usable_uv_bounds))
    }

    pub fn area(&self) -> u32 {
        self.inner.usable_size[0] * self.inner.usable_size[1]
    }

    pub fn allocation_size(&self) -> [u32; 2] {
        self.inner.allocation_size
    }

    pub fn texture_size(&self) -> [u32; 2] {
        self.inner.usable_size
    }

    pub fn atlas_size(&self) -> [u32; 2] {
        self.inner.atlas_size
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.inner.format
    }

    pub fn atlas_pointer(&self) -> Option<usize> {
        self.inner
            .atlas
            .upgrade()
            .map(|arc| Arc::as_ptr(&arc) as usize)
    }

    pub fn translate_uv(&self, uvs: &[[f32; 2]]) -> Result<Vec<[f32; 2]>, RegionError> {
        trace!(
            "AtlasRegion::translate_uv: translating {} vertices for region={:?}",
            uvs.len(),
            self.inner.region_id
        );
        // Get the texture location in the atlas
        let Some(atlas) = self.inner.atlas.upgrade() else {
            warn!("AtlasRegion::translate_uv: atlas dropped");
            return Err(RegionError::AtlasGone);
        };
        let Some(location) = atlas.get_location(self.inner.region_id) else {
            warn!("AtlasRegion::translate_uv: region not found in atlas");
            return Err(RegionError::TextureNotFoundInAtlas);
        };
        let x_max = location.usable_uv_bounds.max.x;
        let y_max = location.usable_uv_bounds.max.y;
        let x_min = location.usable_uv_bounds.min.x;
        let y_min = location.usable_uv_bounds.min.y;

        // Translate the vertices to the texture area
        let translated_vertices = uvs
            .iter()
            .map(|&[x, y]| {
                [
                    (x_min + (x * (x_max - x_min))).clamp(0.0, 1.0),
                    (y_min + (y * (y_max - y_min))).clamp(0.0, 1.0),
                ]
            })
            .collect::<Vec<_>>();

        Ok(translated_vertices)
    }

    pub fn write_data(&self, queue: &wgpu::Queue, data: &[u8]) -> Result<(), RegionError> {
        trace!(
            "AtlasRegion::write_data: uploading {} bytes to region={:?}",
            data.len(),
            self.inner.region_id
        );
        // Check data consistency
        // todo: `block_copy_size()` may return deferent size from the actual size.
        let bytes_per_pixel = self
            .inner
            .format
            .block_copy_size(None)
            .ok_or(RegionError::InvalidFormatBlockCopySize)?;
        let expected_size = self.inner.usable_size[0] * self.inner.usable_size[1] * bytes_per_pixel;
        if data.len() as u32 != expected_size {
            warn!(
                "AtlasRegion::write_data: data size mismatch (expected {} bytes, got {})",
                expected_size,
                data.len()
            );
            return Err(RegionError::DataConsistencyError(format!(
                "Data size({}byte) does not match expected size({}byte)",
                data.len(),
                expected_size
            )));
        }

        // Get the texture in the atlas and location
        let Some(atlas) = self.inner.atlas.upgrade() else {
            warn!("AtlasRegion::write_data: atlas dropped");
            return Err(RegionError::AtlasGone);
        };
        let texture = atlas.texture();
        let Some(location) = atlas.get_location(self.inner.region_id) else {
            warn!("AtlasRegion::write_data: region not found in atlas");
            return Err(RegionError::TextureNotFoundInAtlas);
        };

        let bytes_per_row = self.inner.usable_size[0] * bytes_per_pixel;

        let origin = wgpu::Origin3d {
            x: location.usable_bounds.min.x as u32,
            y: location.usable_bounds.min.y as u32,
            z: location.page_index,
        };

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: self.inner.usable_size[0],
                height: self.inner.usable_size[1],
                depth_or_array_layers: 1,
            },
        );

        trace!("AtlasRegion::write_data: upload completed");

        Ok(())
    }

    pub fn read_data(&self) -> Result<(), RegionError> {
        todo!()
    }

    pub fn copy_from_texture(&self) -> Result<(), RegionError> {
        todo!()
    }

    pub fn copy_to_texture(&self) -> Result<(), RegionError> {
        todo!()
    }

    pub fn copy_from_buffer(&self) -> Result<(), RegionError> {
        todo!()
    }

    pub fn copy_to_buffer(&self) -> Result<(), RegionError> {
        todo!()
    }

    pub fn set_viewport(&self, render_pass: &mut wgpu::RenderPass<'_>) -> Result<(), RegionError> {
        // Get the texture location in the atlas
        let Some(atlas) = self.inner.atlas.upgrade() else {
            return Err(RegionError::AtlasGone);
        };
        let Some(location) = atlas.get_location(self.inner.region_id) else {
            return Err(RegionError::TextureNotFoundInAtlas);
        };

        // Set the viewport to the texture area
        render_pass.set_viewport(
            location.usable_bounds.min.x as f32,
            location.usable_bounds.min.y as f32,
            location.usable_bounds.width() as f32,
            location.usable_bounds.height() as f32,
            0.0,
            1.0,
        );

        Ok(())
    }

    pub fn begin_render_pass<'a>(
        &'a self,
        encoder: &'a mut wgpu::CommandEncoder,
    ) -> Result<wgpu::RenderPass<'a>, RegionError> {
        // Get the texture location in the atlas
        let Some(atlas) = self.inner.atlas.upgrade() else {
            return Err(RegionError::AtlasGone);
        };
        let Some(location) = atlas.get_location(self.inner.region_id) else {
            return Err(RegionError::TextureNotFoundInAtlas);
        };

        // Clear the allocated region (including the margin) before exposing the render pass to users.
        let view = atlas.layer_texture_view(location.page_index as usize);
        let allocation_bounds = location.allocation_bounds();
        let allocation_width = (allocation_bounds.max.x - allocation_bounds.min.x) as u32;
        let allocation_height = (allocation_bounds.max.y - allocation_bounds.min.y) as u32;
        debug_assert!(allocation_width > 0 && allocation_height > 0);
        debug_assert!(allocation_bounds.min.x >= 0);
        debug_assert!(allocation_bounds.min.y >= 0);

        {
            let mut clear_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Texture Atlas Margin Clear Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            clear_pass.set_viewport(
                allocation_bounds.min.x as f32,
                allocation_bounds.min.y as f32,
                allocation_width as f32,
                allocation_height as f32,
                0.0,
                1.0,
            );
            clear_pass.set_scissor_rect(
                allocation_bounds.min.x as u32,
                allocation_bounds.min.y as u32,
                allocation_width,
                allocation_height,
            );
            atlas.viewport_clear.render(
                &atlas.device(),
                &mut clear_pass,
                atlas.format(),
                [0.0, 0.0, 0.0, 0.0],
            );
        }

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Texture Atlas Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        // Set the viewport to the usable texture area (excluding margins)
        render_pass.set_viewport(
            location.usable_bounds.min.x as f32,
            location.usable_bounds.min.y as f32,
            location.usable_bounds.width() as f32,
            location.usable_bounds.height() as f32,
            0.0,
            1.0,
        );

        Ok(render_pass)
    }

    pub fn uv(&self) -> Result<Box2D<f32, euclid::UnknownUnit>, RegionError> {
        // Get the texture location in the atlas
        let Some(atlas) = self.inner.atlas.upgrade() else {
            return Err(RegionError::AtlasGone);
        };
        let Some(location) = atlas.get_location(self.inner.region_id) else {
            return Err(RegionError::TextureNotFoundInAtlas);
        };

        Ok(location.usable_uv_bounds)
    }

    // pub fn with_data<Init, F>(&self, init: Init, f: F) -> Result<(), TextureError>
    // where
    //     Init: FnOnce(&Texture) -> Result<(), TextureError>,
    //     F: FnOnce(&Texture) -> Result<(), TextureError>,
    // {
    //     todo!()
    // }
}

// Ensure the texture area will be deallocated when the texture is dropped.
impl Drop for RegionData {
    fn drop(&mut self) {
        if let Some(atlas) = self.atlas.upgrade() {
            match atlas.deallocate(self.region_id) {
                Ok(_) => {
                    // Successfully deallocated
                }
                Err(DeallocationErrorTextureNotFound) => {
                    // We do not need to handle this error because this means the texture was already deallocated.
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RegionId {
    texture_uuid: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RegionLocation {
    page_index: u32,
    margin: u32,
    allocation_bounds: euclid::Box2D<i32, euclid::UnknownUnit>,
    usable_bounds: euclid::Box2D<i32, euclid::UnknownUnit>,
    usable_uv_bounds: euclid::Box2D<f32, euclid::UnknownUnit>,
}

impl RegionLocation {
    fn new(
        allocation_bounds: Box2D<i32, euclid::UnknownUnit>,
        atlas_size: [u32; 2],
        page_index: usize,
        margin: u32,
    ) -> Self {
        let bounds = if margin == 0 {
            allocation_bounds
        } else {
            euclid::Box2D::new(
                euclid::Point2D::new(
                    allocation_bounds.min.x + margin as i32,
                    allocation_bounds.min.y + margin as i32,
                ),
                euclid::Point2D::new(
                    allocation_bounds.max.x - margin as i32,
                    allocation_bounds.max.y - margin as i32,
                ),
            )
        };

        debug_assert!(bounds.min.x >= allocation_bounds.min.x);
        debug_assert!(bounds.min.y >= allocation_bounds.min.y);
        debug_assert!(bounds.max.x <= allocation_bounds.max.x);
        debug_assert!(bounds.max.y <= allocation_bounds.max.y);
        debug_assert!(bounds.max.x > bounds.min.x);
        debug_assert!(bounds.max.y > bounds.min.y);

        // Normalize the usable bounds to UV space.
        let uv = euclid::Box2D::new(
            euclid::Point2D::new(
                bounds.min.x as f32 / atlas_size[0] as f32,
                bounds.min.y as f32 / atlas_size[1] as f32,
            ),
            euclid::Point2D::new(
                bounds.max.x as f32 / atlas_size[0] as f32,
                bounds.max.y as f32 / atlas_size[1] as f32,
            ),
        );
        Self {
            page_index: page_index as u32,
            margin,
            allocation_bounds,
            usable_bounds: bounds,
            usable_uv_bounds: uv,
        }
    }

    fn area(&self) -> u32 {
        self.usable_bounds.area() as u32
    }

    fn allocation_area(&self) -> u32 {
        self.allocation_bounds.area() as u32
    }

    fn allocation_bounds(&self) -> euclid::Box2D<i32, euclid::UnknownUnit> {
        self.allocation_bounds
    }

    fn size(&self) -> [u32; 2] {
        [
            (self.usable_bounds.max.x - self.usable_bounds.min.x) as u32,
            (self.usable_bounds.max.y - self.usable_bounds.min.y) as u32,
        ]
    }
}

static ATLAS_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureAtlasId {
    id: usize,
}

impl TextureAtlasId {
    fn new() -> Self {
        let id = ATLAS_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { id }
    }
}

pub struct TextureAtlas {
    id: TextureAtlasId,
    format: wgpu::TextureFormat,
    state: Mutex<TextureAtlasState>,
    resources: RwLock<TextureAtlasResources>,
    device: RwLock<wgpu::Device>,
    viewport_clear: ViewportClear,
    margin: u32,
    weak_self: Weak<Self>,
}

struct TextureAtlasResources {
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    layer_texture_views: Vec<wgpu::TextureView>,
    size: wgpu::Extent3d,
}

struct TextureAtlasState {
    allocators: Vec<AtlasAllocator>,
    texture_id_to_location: HashMap<RegionId, RegionLocation>,
    texture_id_to_alloc_id: HashMap<RegionId, AllocId>,
    usage: usize,
}

/// Constructor and information methods.
impl TextureAtlas {
    pub const DEFAULT_MARGIN_PX: u32 = 1;

    pub fn new(
        device: &wgpu::Device,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        margin: u32,
    ) -> Arc<Self> {
        let (texture, texture_view, layer_texture_views) =
            Self::create_texture_and_view(device, format, size);

        // Initialize the state with an empty allocator and allocation map.
        let state = TextureAtlasState {
            allocators: (0..size.depth_or_array_layers)
                .map(|_| Size::new(size.width as i32, size.height as i32))
                .map(AtlasAllocator::new)
                .collect(),
            texture_id_to_location: HashMap::new(),
            texture_id_to_alloc_id: HashMap::new(),
            usage: 0,
        };

        let resources = TextureAtlasResources {
            texture,
            texture_view,
            layer_texture_views,
            size,
        };

        Arc::new_cyclic(|weak_self| Self {
            id: TextureAtlasId::new(),
            format,
            state: Mutex::new(state),
            resources: RwLock::new(resources),
            device: RwLock::new(device.clone()),
            viewport_clear: ViewportClear::default(),
            margin,
            weak_self: weak_self.clone(),
        })
    }
}

impl TextureAtlas {
    pub fn size(&self) -> wgpu::Extent3d {
        self.resources.read().size
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    fn device(&self) -> wgpu::Device {
        self.device.read().clone()
    }

    pub fn margin(&self) -> u32 {
        self.margin
    }

    pub fn capacity(&self) -> usize {
        let resources = self.resources.read();
        resources.size.width as usize
            * resources.size.height as usize
            * resources.size.depth_or_array_layers as usize
    }

    pub fn usage(&self) -> usize {
        self.state.lock().usage
    }

    // todo: we can optimize this performance.
    pub fn max_allocation_size(&self) -> [u32; 2] {
        let mut max_size = [0; 2];

        let state = self.state.lock();
        for location in state.texture_id_to_location.values() {
            let size = location.size();
            max_size[0] = max_size[0].max(size[0]);
            max_size[1] = max_size[1].max(size[1]);
        }
        max_size
    }
}

/// TextureAtlas allocation and deallocation
impl TextureAtlas {
    /// Allocate a texture in the atlas.
    pub fn allocate(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        requested_size: [u32; 2],
    ) -> Result<AtlasRegion, TextureAtlasError> {
        // Check if size is smaller than the atlas size
        if requested_size[0] == 0 || requested_size[1] == 0 {
            return Err(TextureAtlasError::AllocationFailedInvalidSize {
                requested: requested_size,
            });
        }
        let atlas_size = self.size();
        if requested_size[0] + self.margin * 2 > atlas_size.width
            || requested_size[1] + self.margin * 2 > atlas_size.height
        {
            return Err(TextureAtlasError::AllocationFailedTooLarge {
                requested: requested_size,
                available: [atlas_size.width, atlas_size.height],
                margin_needed: self.margin * 2,
            });
        }

        let doubled_margin =
            self.margin
                .checked_mul(2)
                .ok_or(TextureAtlasError::AllocationFailedInvalidSize {
                    requested: requested_size,
                })?;

        let allocation_width = requested_size[0].checked_add(doubled_margin).ok_or(
            TextureAtlasError::AllocationFailedInvalidSize {
                requested: requested_size,
            },
        )?;
        let allocation_height = requested_size[1].checked_add(doubled_margin).ok_or(
            TextureAtlasError::AllocationFailedInvalidSize {
                requested: requested_size,
            },
        )?;

        if allocation_width > i32::MAX as u32 || allocation_height > i32::MAX as u32 {
            return Err(TextureAtlasError::AllocationFailedInvalidSize {
                requested: requested_size,
            });
        }

        let allocation_size = Size::new(allocation_width as i32, allocation_height as i32);

        if let Some(region) =
            self.try_allocate(allocation_size, [atlas_size.width, atlas_size.height])
        {
            return Ok(region);
        }

        self.add_one_page(device, queue);

        let updated_size = self.size();
        self.try_allocate(allocation_size, [updated_size.width, updated_size.height])
            .ok_or(TextureAtlasError::AllocationFailedNotEnoughSpace)
    }

    /// Deallocate a texture from the atlas.
    /// This will be called automatically when the `TextureInner` is dropped.
    fn deallocate(&self, id: RegionId) -> Result<(), DeallocationErrorTextureNotFound> {
        let mut state = self.state.lock();

        // Find the texture location and remove it from the id-to-location map.
        let location = state
            .texture_id_to_location
            .remove(&id)
            .ok_or(DeallocationErrorTextureNotFound)?;

        // Find the allocation id and remove it from the id-to-alloc-id map.
        let alloc_id = state
            .texture_id_to_alloc_id
            .remove(&id)
            .ok_or(DeallocationErrorTextureNotFound)?;

        // Deallocate the texture from the allocator.
        state.allocators[location.page_index as usize].deallocate(alloc_id);

        // Update usage
        state.usage -= location.allocation_area() as usize;

        Ok(())
    }

    fn try_allocate(&self, allocation_size: Size, atlas_size: [u32; 2]) -> Option<AtlasRegion> {
        let mut state = self.state.lock();

        for (page_index, allocator) in state.allocators.iter_mut().enumerate() {
            if let Some(alloc) = allocator.allocate(allocation_size) {
                let location =
                    RegionLocation::new(alloc.rectangle, atlas_size, page_index, self.margin);

                let texture_id = RegionId {
                    texture_uuid: Uuid::new_v4(),
                };
                let texture_inner = RegionData {
                    region_id: texture_id,
                    atlas_id: self.id,
                    atlas: self.weak_self.clone(),
                    allocation_size: [
                        location.allocation_bounds.width() as u32,
                        location.allocation_bounds.height() as u32,
                    ],
                    usable_size: [
                        location.usable_bounds.width() as u32,
                        location.usable_bounds.height() as u32,
                    ],
                    atlas_size,
                    format: self.format,
                };
                let texture = AtlasRegion {
                    inner: Arc::new(texture_inner),
                };

                state.texture_id_to_location.insert(texture_id, location);
                state.texture_id_to_alloc_id.insert(texture_id, alloc.id);
                state.usage += location.allocation_area() as usize;

                return Some(texture);
            }
        }

        None
    }
}

/// Resize the atlas to a new size.
impl TextureAtlas {
    fn add_one_page(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut resources = self.resources.write();
        let previous_size = resources.size;
        let new_size = wgpu::Extent3d {
            width: previous_size.width,
            height: previous_size.height,
            depth_or_array_layers: previous_size.depth_or_array_layers + 1,
        };

        let (new_texture, new_texture_view, new_layer_texture_views) =
            Self::create_texture_and_view(device, self.format, new_size);

        {
            let mut state = self.state.lock();
            state.allocators.push(AtlasAllocator::new(Size::new(
                new_size.width as i32,
                new_size.height as i32,
            )));
        }

        let old_texture = resources.texture.clone();

        // Copy existing texture data to the new textures.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("TextureAtlas Resize Encoder"),
        });

        // Copy existing pages into the new texture
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &old_texture,
                mip_level: 0,
                aspect: wgpu::TextureAspect::All,
                origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
            },
            wgpu::TexelCopyTextureInfo {
                texture: &new_texture,
                mip_level: 0,
                aspect: wgpu::TextureAspect::All,
                origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
            },
            wgpu::Extent3d {
                width: previous_size.width,
                height: previous_size.height,
                depth_or_array_layers: previous_size.depth_or_array_layers,
            },
        );

        // Clear only the newly added layer to ensure it is initialized and transparent.
        // This prevents uninitialized memory in the new layer and keeps existing pages intact.
        let new_layer_index = new_size.depth_or_array_layers - 1;
        if let Some(view) = new_layer_texture_views.get(new_layer_index as usize) {
            let _clear_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("TextureAtlas Init New Layer Clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }

        queue.submit(Some(encoder.finish()));

        resources.texture = new_texture;
        resources.texture_view = new_texture_view;
        resources.layer_texture_views = new_layer_texture_views;
        resources.size = new_size;
    }
}

impl TextureAtlas {
    fn get_location(&self, id: RegionId) -> Option<RegionLocation> {
        self.state.lock().texture_id_to_location.get(&id).copied()
    }

    pub fn texture(&self) -> wgpu::Texture {
        self.resources.read().texture.clone()
    }

    pub fn texture_view(&self) -> wgpu::TextureView {
        self.resources.read().texture_view.clone()
    }

    fn layer_texture_view(&self, index: usize) -> wgpu::TextureView {
        self.resources.read().layer_texture_views[index].clone()
    }
}

// helper functions
impl TextureAtlas {
    fn create_texture_and_view(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        page_size: wgpu::Extent3d,
    ) -> (wgpu::Texture, wgpu::TextureView, Vec<wgpu::TextureView>) {
        let texture_label = format!("texture_atlas_texture_{format:?}");
        let texture_view_label = format!("texture_atlas_texture_view_{format:?}");

        let texture_descriptor = wgpu::TextureDescriptor {
            label: Some(&texture_label),
            size: page_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        };
        let texture = device.create_texture(&texture_descriptor);

        // D2Array view for sampling all layers
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(&texture_view_label),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            aspect: wgpu::TextureAspect::All,
            ..Default::default()
        });

        // Per-layer D2 views for render attachments (one per array layer)
        let mut per_layer_views = Vec::with_capacity(page_size.depth_or_array_layers as usize);
        for layer in 0..page_size.depth_or_array_layers {
            let layer_view = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(&format!("texture_atlas_layer_view_{format:?}_{layer}")),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: 0,
                mip_level_count: Some(1),
                base_array_layer: layer,
                array_layer_count: Some(1),
                aspect: wgpu::TextureAspect::All,
                ..Default::default()
            });
            per_layer_views.push(layer_view);
        }

        (texture, texture_view, per_layer_views)
    }
}

/// `DeallocationErrorTextureNotFound` only be used in this file.
struct DeallocationErrorTextureNotFound;

#[derive(Error, Debug)]
pub enum RegionError {
    #[error("The texture's atlas has been dropped.")]
    AtlasGone,
    #[error("The texture was not found in the atlas.")]
    TextureNotFoundInAtlas,
    #[error("Data consistency error: {0}")]
    DataConsistencyError(String),
    #[error("Invalid format block copy size.")]
    InvalidFormatBlockCopySize,
}

#[derive(Error, Debug)]
pub enum TextureAtlasError {
    #[error("Allocation failed because there was not enough space in the atlas.")]
    AllocationFailedNotEnoughSpace,
    #[error("Resizing the atlas failed because there was not enough space for all the textures.")]
    ResizeFailedNotEnoughSpace,
    #[error(
        "Allocation failed because the requested size is too large for the atlas. requested: {requested:?} available: {available:?}"
    )]
    AllocationFailedTooLarge {
        requested: [u32; 2],
        available: [u32; 2],
        margin_needed: u32,
    },
    #[error("Allocation failed because the requested size is invalid. requested: {requested:?}")]
    AllocationFailedInvalidSize { requested: [u32; 2] },
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::panic::AssertUnwindSafe;
    use std::sync::Arc;

    async fn setup_atlas(
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
        margin: u32,
    ) -> (wgpu::Device, wgpu::Queue, Arc<TextureAtlas>) {
        let (_, _, device, queue) = crate::wgpu_utils::noop_wgpu().await;
        let atlas = TextureAtlas::new(&device, size, format, margin);
        (device, queue, atlas)
    }

    fn allocation_area(region: &AtlasRegion) -> usize {
        (region.allocation_size()[0] * region.allocation_size()[1]) as usize
    }

    #[tokio::test]
    async fn use_tokio_test_macro_to_await_to_get_wgpu_device() {
        let (_, _, device, queue) = crate::wgpu_utils::noop_wgpu().await;

        let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("noop"),
        });
        queue.submit(Some(encoder.finish()));
        queue.on_submitted_work_done(|| {});
    }

    #[tokio::test]
    async fn atlas_new_initializes_resources_and_metadata() {
        let size = wgpu::Extent3d {
            width: 32,
            height: 16,
            depth_or_array_layers: 2,
        };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let margin = 2;
        let (_device, _queue, atlas) = setup_atlas(size, format, margin).await;

        assert_eq!(atlas.size(), size);
        assert_eq!(atlas.format(), format);
        assert_eq!(atlas.margin(), margin);
        assert_eq!(
            atlas.capacity(),
            (size.width * size.height * size.depth_or_array_layers) as usize
        );
        assert_eq!(atlas.usage(), 0);

        let texture = atlas.texture();
        assert_eq!(texture.size(), size);
        assert_eq!(texture.mip_level_count(), 1);
        assert_eq!(texture.dimension(), wgpu::TextureDimension::D2);
        let usage = texture.usage();
        assert!(usage.contains(wgpu::TextureUsages::TEXTURE_BINDING));
        assert!(usage.contains(wgpu::TextureUsages::COPY_SRC));
        assert!(usage.contains(wgpu::TextureUsages::COPY_DST));
        assert!(usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT));
    }

    #[tokio::test]
    async fn atlas_texture_and_views_have_expected_dimensions_and_usages() {
        let size = wgpu::Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 3,
        };
        let (device, _queue, atlas) = setup_atlas(
            size,
            wgpu::TextureFormat::Rgba8Unorm,
            TextureAtlas::DEFAULT_MARGIN_PX,
        )
        .await;

        let array_view = atlas.texture_view();
        assert_eq!(
            array_view.texture().size().depth_or_array_layers,
            size.depth_or_array_layers
        );

        for layer in 0..size.depth_or_array_layers {
            let view = atlas.layer_texture_view(layer as usize);
            assert_eq!(
                view.texture().size().depth_or_array_layers,
                size.depth_or_array_layers
            );
        }

        drop(device);
    }

    #[tokio::test]
    async fn allocate_rejects_zero_dimension() {
        let size = wgpu::Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 1,
        };
        let (device, queue, atlas) = setup_atlas(size, wgpu::TextureFormat::Rgba8Unorm, 0).await;

        let err = atlas.allocate(&device, &queue, [0, 4]).unwrap_err();
        assert!(matches!(
            err,
            TextureAtlasError::AllocationFailedInvalidSize { requested } if requested == [0, 4]
        ));

        let err = atlas.allocate(&device, &queue, [4, 0]).unwrap_err();
        assert!(matches!(
            err,
            TextureAtlasError::AllocationFailedInvalidSize { requested } if requested == [4, 0]
        ));
    }

    #[tokio::test]
    async fn allocate_rejects_too_large_including_margins() {
        let margin = 2;
        let size = wgpu::Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        };
        let (device, queue, atlas) =
            setup_atlas(size, wgpu::TextureFormat::Rgba8Unorm, margin).await;

        let err = atlas.allocate(&device, &queue, [7, 7]).unwrap_err();
        match err {
            TextureAtlasError::AllocationFailedTooLarge {
                requested,
                available,
                margin_needed,
            } => {
                assert_eq!(requested, [7, 7]);
                assert_eq!(available, [size.width, size.height]);
                assert_eq!(margin_needed, margin * 2);
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[tokio::test]
    async fn allocate_success_and_region_exposes_expected_properties() {
        let margin = 1;
        let size = wgpu::Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 1,
        };
        let requested = [6, 4];
        let (device, queue, atlas) =
            setup_atlas(size, wgpu::TextureFormat::Rgba8Unorm, margin).await;
        let region = atlas.allocate(&device, &queue, requested).unwrap();

        assert_eq!(region.texture_size(), requested);
        assert_eq!(
            region.allocation_size(),
            [requested[0] + margin * 2, requested[1] + margin * 2]
        );
        assert_eq!(region.atlas_size(), [size.width, size.height]);
        assert_eq!(region.format(), atlas.format());
        assert_eq!(region.area(), requested[0] * requested[1]);

        let atlas_ptr = Arc::as_ptr(&atlas) as usize;
        assert_eq!(region.atlas_pointer(), Some(atlas_ptr));

        let (page_index, usable_uv) = region.position_in_atlas().unwrap();
        assert_eq!(page_index, 0);
        assert!(usable_uv.min.x >= 0.0 && usable_uv.min.y >= 0.0);
        assert!(usable_uv.max.x <= 1.0 && usable_uv.max.y <= 1.0);
        assert_eq!(region.uv().unwrap(), usable_uv);
    }

    #[tokio::test]
    async fn allocate_triggers_growth_by_adding_one_page() {
        let size = wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        };
        let (device, queue, atlas) = setup_atlas(size, wgpu::TextureFormat::Rgba8Unorm, 0).await;

        let _region_full = atlas.allocate(&device, &queue, [4, 4]).unwrap();
        let previous_capacity = atlas.capacity();
        let region = atlas.allocate(&device, &queue, [2, 2]).unwrap();

        let (page_index, _) = region.position_in_atlas().unwrap();
        assert_eq!(page_index, 1);
        assert_eq!(atlas.size().depth_or_array_layers, 2);
        assert_eq!(atlas.capacity(), previous_capacity * 2);
    }

    #[tokio::test]
    async fn usage_tracks_allocation_and_drop_deallocation() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        )
        .await;

        let (area_a, area_b);
        {
            let region_a = atlas.allocate(&device, &queue, [4, 4]).unwrap();
            let region_b = atlas.allocate(&device, &queue, [2, 6]).unwrap();
            area_a = allocation_area(&region_a);
            area_b = allocation_area(&region_b);
            assert_eq!(atlas.usage(), area_a + area_b);
        }

        assert_eq!(atlas.usage(), 0);
    }

    #[tokio::test]
    async fn max_allocation_size_reflects_largest_live_region() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 32,
                height: 32,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        )
        .await;

        let region_small = atlas.allocate(&device, &queue, [3, 5]).unwrap();
        let region_large = atlas.allocate(&device, &queue, [12, 8]).unwrap();
        assert_eq!(atlas.max_allocation_size(), [12, 8]);

        drop(region_large);
        assert_eq!(atlas.max_allocation_size(), [3, 5]);

        drop(region_small);
        assert_eq!(atlas.max_allocation_size(), [0, 0]);
    }

    #[tokio::test]
    async fn position_in_atlas_returns_atlas_gone_when_atlas_dropped() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [4, 4]).unwrap();

        drop(atlas);
        let err = region.position_in_atlas().unwrap_err();
        assert!(matches!(err, RegionError::AtlasGone));
    }

    #[tokio::test]
    async fn atlas_pointer_returns_none_after_atlas_drop() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [4, 4]).unwrap();

        assert!(region.atlas_pointer().is_some());
        drop(atlas);
        assert!(region.atlas_pointer().is_none());
    }

    #[tokio::test]
    async fn uv_matches_position_in_atlas_usable_uv_bounds() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [4, 4]).unwrap();

        let (_, usable_uv) = region.position_in_atlas().unwrap();
        assert_eq!(region.uv().unwrap(), usable_uv);
    }

    #[tokio::test]
    async fn translate_uv_maps_corners_and_clamps() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [4, 4]).unwrap();

        let (_, usable_uv) = region.position_in_atlas().unwrap();
        let translated = region
            .translate_uv(&[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [-0.5, 1.5]])
            .unwrap();

        assert_eq!(translated[0], [usable_uv.min.x, usable_uv.min.y]);
        assert_eq!(translated[1], [usable_uv.max.x, usable_uv.min.y]);
        assert_eq!(translated[2], [usable_uv.min.x, usable_uv.max.y]);
        assert_eq!(translated[3], [usable_uv.max.x, usable_uv.max.y]);
        assert!((0.0..=1.0).contains(&translated[4][0]));
        assert!((0.0..=1.0).contains(&translated[4][1]));
    }

    #[tokio::test]
    async fn write_data_succeeds_on_consistent_size() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            0,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [4, 2]).unwrap();
        let bytes_per_pixel = region.format().block_copy_size(None).unwrap();
        let byte_count =
            (region.texture_size()[0] * region.texture_size()[1] * bytes_per_pixel) as usize;
        let data = vec![255u8; byte_count];

        region.write_data(&queue, &data).unwrap();
    }

    #[tokio::test]
    async fn write_data_fails_on_data_size_mismatch() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            0,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [4, 2]).unwrap();
        let err = region.write_data(&queue, &[0u8; 3]).unwrap_err();
        assert!(matches!(err, RegionError::DataConsistencyError(_)));
    }

    #[tokio::test]
    async fn write_data_fails_on_invalid_format_block_size() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Depth24Plus,
            0,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [2, 2]).unwrap();
        let err = region.write_data(&queue, &[0]).unwrap_err();
        assert!(matches!(err, RegionError::InvalidFormatBlockCopySize));
    }

    #[tokio::test]
    async fn write_data_fails_with_atlas_gone_when_atlas_dropped() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            0,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [2, 2]).unwrap();
        let bytes_per_pixel = region.format().block_copy_size(None).unwrap();
        let expected_bytes =
            (region.texture_size()[0] * region.texture_size()[1] * bytes_per_pixel) as usize;
        let payload = vec![0u8; expected_bytes];
        drop(atlas);

        let err = region.write_data(&queue, &payload).unwrap_err();
        assert!(matches!(err, RegionError::AtlasGone));
    }

    #[test]
    fn read_and_copy_operations_placeholders() {
        // NOTE:
        // These methods are currently unimplemented (todo!) and are expected to panic.
        // If they are implemented to return a Result (Err) in the future, this test must be updated/replaced.
        // To make the intent explicit, we assert they panic by using catch_unwind.
        let (device, queue, atlas) = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(setup_atlas(
                wgpu::Extent3d {
                    width: 4,
                    height: 4,
                    depth_or_array_layers: 1,
                },
                wgpu::TextureFormat::Rgba8Unorm,
                0,
            ));
        let region = atlas.allocate(&device, &queue, [2, 2]).unwrap();

        let read = std::panic::catch_unwind(AssertUnwindSafe(|| region.read_data()));
        assert!(read.is_err());
        let copy_tex = std::panic::catch_unwind(AssertUnwindSafe(|| region.copy_from_texture()));
        assert!(copy_tex.is_err());
        let copy_to_tex = std::panic::catch_unwind(AssertUnwindSafe(|| region.copy_to_texture()));
        assert!(copy_to_tex.is_err());
        let copy_buf = std::panic::catch_unwind(AssertUnwindSafe(|| region.copy_from_buffer()));
        assert!(copy_buf.is_err());
        let copy_to_buf = std::panic::catch_unwind(AssertUnwindSafe(|| region.copy_to_buffer()));
        assert!(copy_to_buf.is_err());
    }

    // Skeleton for when read/copy APIs return Result<_, RegionError> in the future.
    // After implementation, remove #[ignore] and assert specific Err variants.
    #[ignore]
    #[tokio::test]
    async fn read_and_copy_operations_return_error_when_implemented() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            0,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [2, 2]).unwrap();

        // FIXME: After implementation, check concrete Err variants
        let _ = region; // placate lint
        // e.g., assert!(matches!(region.read_data(), Err(RegionError::...)));
    }

    #[tokio::test]
    async fn set_viewport_sets_usable_bounds() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            0,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [2, 2]).unwrap();
        let view = atlas.texture_view();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            region.set_viewport(&mut pass).unwrap();
        }
        queue.submit(Some(encoder.finish()));
    }

    #[tokio::test]
    async fn set_viewport_errors_when_atlas_gone() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            0,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [2, 2]).unwrap();
        let view = atlas.texture_view();

        drop(atlas);
        let view_clone = view.clone();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("atlas-gone"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view_clone,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let err = region.set_viewport(&mut pass).unwrap_err();
            assert!(matches!(err, RegionError::AtlasGone));
        }
    }

    #[tokio::test]
    async fn begin_render_pass_clears_allocation_and_sets_viewport() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            0,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [2, 2]).unwrap();
        if !device.features().contains(wgpu::Features::IMMEDIATES) {
            return;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let _pass = region.begin_render_pass(&mut encoder).unwrap();
        }
        queue.submit(Some(encoder.finish()));
    }

    // Verify allocation succeeds at equality boundary (requested + 2*margin == atlas_size)
    #[tokio::test]
    async fn allocate_accepts_size_equal_to_atlas_including_margins() {
        let margin = 1;
        let size = wgpu::Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 1,
        };
        let (device, queue, atlas) =
            setup_atlas(size, wgpu::TextureFormat::Rgba8Unorm, margin).await;

        // 14 + 2*1 == 16 -> exactly fits
        let region = atlas.allocate(&device, &queue, [14, 14]).unwrap();
        assert_eq!(region.texture_size(), [14, 14]);
        assert_eq!(region.allocation_size(), [16, 16]);
        let (page_index, _uv) = region.position_in_atlas().unwrap();
        assert_eq!(page_index, 0);
    }

    // Verify translate_uv maps the midpoint linearly
    #[tokio::test]
    async fn translate_uv_maps_midpoint_linearly() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [6, 4]).unwrap();

        let (_, uv) = region.position_in_atlas().unwrap();
        let mid = region.translate_uv(&[[0.5, 0.5]]).unwrap();
        let expected = [(uv.min.x + uv.max.x) * 0.5, (uv.min.y + uv.max.y) * 0.5];
        let got = mid[0];
        let eps = 1e-6;
        assert!(
            (got[0] - expected[0]).abs() < eps,
            "x midpoint mismatch: got={}, expected={}",
            got[0],
            expected[0]
        );
        assert!(
            (got[1] - expected[1]).abs() < eps,
            "y midpoint mismatch: got={}, expected={}",
            got[1],
            expected[1]
        );
    }

    // Verify usage accumulates across multiple pages
    #[tokio::test]
    async fn usage_accumulates_across_pages() {
        let size = wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        };
        let (device, queue, atlas) = setup_atlas(size, wgpu::TextureFormat::Rgba8Unorm, 0).await;

        let r0 = atlas.allocate(&device, &queue, [4, 4]).unwrap(); // fills page 0
        let usage_after_r0 = atlas.usage();
        assert_eq!(usage_after_r0, allocation_area(&r0));

        let r1 = atlas.allocate(&device, &queue, [1, 1]).unwrap(); // goes to page 1
        let usage_after_r1 = atlas.usage();
        assert_eq!(atlas.size().depth_or_array_layers, 2);
        assert_eq!(usage_after_r1, allocation_area(&r0) + allocation_area(&r1));

        drop(r0);
        assert_eq!(atlas.usage(), allocation_area(&r1));

        drop(r1);
        assert_eq!(atlas.usage(), 0);
    }

    #[tokio::test]
    async fn begin_render_pass_errors_when_atlas_gone() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            0,
        )
        .await;
        let region = atlas.allocate(&device, &queue, [2, 2]).unwrap();

        drop(atlas);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        assert!(matches!(
            region.begin_render_pass(&mut encoder).unwrap_err(),
            RegionError::AtlasGone
        ));
    }

    #[tokio::test]
    async fn layer_texture_view_index_range_matches_page_count() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            0,
        )
        .await;
        atlas.layer_texture_view(0);

        let _region_full = atlas.allocate(&device, &queue, [4, 4]).unwrap();
        let _ = atlas.allocate(&device, &queue, [2, 2]).unwrap();
        assert_eq!(atlas.size().depth_or_array_layers, 2);
        atlas.layer_texture_view(0);
        atlas.layer_texture_view(1);
    }

    #[tokio::test]
    async fn allocate_rejects_overflow_when_applying_margins() {
        let (device, queue, atlas) = setup_atlas(
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        )
        .await;

        {
            let mut resources = atlas.resources.write();
            resources.size.width = u32::MAX;
        }

        let err = atlas
            .allocate(&device, &queue, [i32::MAX as u32, 1])
            .unwrap_err();
        assert!(matches!(
            err,
            TextureAtlasError::AllocationFailedInvalidSize { requested } if requested == [i32::MAX as u32, 1]
        ));
    }
}
