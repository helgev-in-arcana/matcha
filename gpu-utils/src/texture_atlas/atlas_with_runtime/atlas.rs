use std::sync::{Arc, Weak};

use dashmap::DashMap;
use euclid::Box2D;
use guillotiere::{AllocId, AtlasAllocator, euclid};
use parking_lot::{Mutex, RwLock};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone)]
pub struct AtlasRegion {
    inner: Arc<RegionData>,
}

// We only store the texture id and reference to the atlas,
// to make `Texture` remain valid after `TextureAtlas` resizes or changes,
// except for data loss when the atlas shrinks.
struct RegionData {
    // allocation info
    texture_id: RegionId,
    // interaction with the atlas
    atlas: Weak<RwLock<TextureAtlas>>,
    // It may be useful to store some information about the texture that will not change during atlas resizing
    size: [u32; 2],                    // size of the texture in pixels
    formats: Vec<wgpu::TextureFormat>, // formats of the texture
}

/// Public API to interact with a texture.
/// User code should not need to know about its id, location, or atlas.
impl AtlasRegion {
    pub fn area(&self) -> u32 {
        self.inner.size[0] * self.inner.size[1]
    }

    pub fn size(&self) -> [u32; 2] {
        self.inner.size
    }

    pub fn formats(&self) -> &[wgpu::TextureFormat] {
        &self.inner.formats
    }

    pub fn write_data(&self, queue: &wgpu::Queue, data: &[&[u8]]) -> Result<(), AtlasRegionError> {
        // Check data consistency
        if data.len() != self.inner.formats.len() {
            return Err(AtlasRegionError::DataConsistencyError(
                "Data length does not match formats length".to_string(),
            ));
        }
        for (i, format) in self.inner.formats.iter().enumerate() {
            let bytes_per_pixel = format
                .block_copy_size(None)
                .ok_or(AtlasRegionError::InvalidFormatBlockCopySize)?;
            let expected_size = self.inner.size[0] * self.inner.size[1] * bytes_per_pixel;
            if data[i].len() as u32 != expected_size {
                return Err(AtlasRegionError::DataConsistencyError(format!(
                    "Data size for format {i} does not match expected size"
                )));
            }
        }

        // Get the texture in the atlas and location
        let Some(atlas) = self.inner.atlas.upgrade() else {
            return Err(AtlasRegionError::AtlasGone);
        };
        let atlas = atlas.read();

        let textures = atlas.textures();
        let Some(location) = atlas.get_location(self.inner.texture_id) else {
            return Err(AtlasRegionError::TextureNotFoundInAtlas);
        };

        for (i, texture) in textures.iter().enumerate() {
            let data = data[i];

            let bytes_per_pixel = texture
                .format()
                .block_copy_size(None)
                .ok_or(AtlasRegionError::InvalidFormatBlockCopySize)?;
            let bytes_per_row = self.inner.size[0] * bytes_per_pixel;

            let origin = wgpu::Origin3d {
                x: location.bounds.min.x as u32,
                y: location.bounds.min.y as u32,
                z: location.page_index,
            };

            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
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
                    width: self.inner.size[0],
                    height: self.inner.size[1],
                    depth_or_array_layers: 1,
                },
            );
        }

        Ok(())
    }

    pub fn read_data(&self) -> Result<(), AtlasRegionError> {
        todo!()
    }

    pub fn copy_from_texture(&self) -> Result<(), AtlasRegionError> {
        todo!()
    }

    pub fn copy_to_texture(&self) -> Result<(), AtlasRegionError> {
        todo!()
    }

    pub fn copy_from_buffer(&self) -> Result<(), AtlasRegionError> {
        todo!()
    }

    pub fn copy_to_buffer(&self) -> Result<(), AtlasRegionError> {
        todo!()
    }

    pub fn set_viewport(
        &self,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) -> Result<(), AtlasRegionError> {
        // Get the texture location in the atlas
        let Some(atlas) = self.inner.atlas.upgrade() else {
            return Err(AtlasRegionError::AtlasGone);
        };
        let atlas = atlas.read();
        let Some(location) = atlas.get_location(self.inner.texture_id) else {
            return Err(AtlasRegionError::TextureNotFoundInAtlas);
        };

        // Set the viewport to the texture area
        render_pass.set_viewport(
            location.bounds.min.x as f32,
            location.bounds.min.y as f32,
            location.size()[0] as f32,
            location.size()[1] as f32,
            0.0,
            1.0,
        );

        Ok(())
    }

    pub fn begin_render_pass<'a>(
        &'a self,
        encoder: &'a mut wgpu::CommandEncoder,
    ) -> Result<wgpu::RenderPass<'a>, AtlasRegionError> {
        // Get the texture location in the atlas
        let Some(atlas) = self.inner.atlas.upgrade() else {
            return Err(AtlasRegionError::AtlasGone);
        };
        let atlas = atlas.read();
        let Some(location) = atlas.get_location(self.inner.texture_id) else {
            return Err(AtlasRegionError::TextureNotFoundInAtlas);
        };

        // Create a render pass for the texture area
        let mut render_pass = {
            // Keep the state read guard alive for the duration of the descriptor construction.
            let state_guard = atlas.state.read();
            let layer_texture_views = match &*state_guard {
                TextureAtlasState::Solid(atlas_solid) => &atlas_solid.layer_texture_views,
                TextureAtlasState::Resize(atlas_resize) => {
                    if let Some(views) = &atlas_resize.new_layer_texture_views {
                        views
                    } else {
                        &atlas_resize.old_layer_texture_views
                    }
                }
            };

            // Build color attachments slice referencing per-layer D2 views.
            let color_attachments_vec: Vec<Option<wgpu::RenderPassColorAttachment>> =
                layer_texture_views
                    .iter()
                    .map(|views| {
                        let view = &views[location.page_index as usize];
                        Some(wgpu::RenderPassColorAttachment {
                            view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })
                    })
                    .collect();

            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Texture Atlas Render Pass"),
                color_attachments: color_attachments_vec.as_slice(),
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
        };

        // Set the viewport to the texture area
        render_pass.set_viewport(
            location.bounds.min.x as f32,
            location.bounds.min.y as f32,
            location.size()[0] as f32,
            location.size()[1] as f32,
            0.0,
            1.0,
        );

        Ok(render_pass)
    }

    pub fn uv(&self) -> Result<Box2D<f32, euclid::UnknownUnit>, AtlasRegionError> {
        // Get the texture location in the atlas
        let Some(atlas) = self.inner.atlas.upgrade() else {
            return Err(AtlasRegionError::AtlasGone);
        };
        let atlas = atlas.read();
        let Some(location) = atlas.get_location(self.inner.texture_id) else {
            return Err(AtlasRegionError::TextureNotFoundInAtlas);
        };

        Ok(location.uv)
    }
}

// Ensure the texture area will be deallocated when the texture is dropped.
impl Drop for RegionData {
    fn drop(&mut self) {
        if let Some(atlas) = self.atlas.upgrade() {
            match atlas.read().deallocate(self.texture_id) {
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

#[derive(Error, Debug)]
pub enum AtlasRegionError {
    #[error("The texture's atlas has been dropped.")]
    AtlasGone,
    #[error("The texture was not found in the atlas.")]
    TextureNotFoundInAtlas,
    #[error("Data consistency error: {0}")]
    DataConsistencyError(String),
    #[error("Invalid format block copy size.")]
    InvalidFormatBlockCopySize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RegionId {
    texture_uuid: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RegionLocation {
    page_index: u32,
    bounds: euclid::Box2D<i32, euclid::UnknownUnit>,
    uv: euclid::Box2D<f32, euclid::UnknownUnit>,
}

impl RegionLocation {
    fn area(&self) -> u32 {
        self.bounds.area() as u32
    }

    fn size(&self) -> [u32; 2] {
        [
            (self.bounds.max.x - self.bounds.min.x) as u32,
            (self.bounds.max.y - self.bounds.min.y) as u32,
        ]
    }
}

pub struct TextureAtlas {
    formats: Vec<wgpu::TextureFormat>,
    state: RwLock<TextureAtlasState>,
    weak_self: Weak<Self>,
}

enum TextureAtlasState {
    Solid(TextureAtlasSolid),
    Resize(TextureAtlasResize),
}

// Constructor and information methods.
impl TextureAtlas {
    pub fn new(
        device: &wgpu::Device,
        size: wgpu::Extent3d,
        formats: &[wgpu::TextureFormat],
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            formats: formats.to_vec(),
            state: RwLock::new(TextureAtlasState::Solid(TextureAtlasSolid::new(
                device, size, formats,
            ))),
            weak_self: weak_self.clone(),
        })
    }

    pub fn size(&self) -> wgpu::Extent3d {
        match &*self.state.read() {
            TextureAtlasState::Solid(atlas) => atlas.size(),
            TextureAtlasState::Resize(atlas) => atlas.size(),
        }
    }

    pub fn formats(&self) -> &[wgpu::TextureFormat] {
        &self.formats
    }

    pub fn capacity(&self) -> usize {
        match &*self.state.read() {
            TextureAtlasState::Solid(atlas) => atlas.capacity(),
            TextureAtlasState::Resize(atlas) => atlas.capacity(),
        }
    }

    pub fn usage(&self) -> usize {
        match &*self.state.read() {
            TextureAtlasState::Solid(atlas) => atlas.usage(),
            TextureAtlasState::Resize(atlas) => atlas.usage(),
        }
    }

    // todo: we can optimize this performance.
    pub fn max_allocation_size(&self) -> [u32; 2] {
        match &*self.state.read() {
            TextureAtlasState::Solid(atlas) => atlas.max_allocation_size(),
            TextureAtlasState::Resize(atlas) => atlas.max_allocation_size(),
        }
    }
}

/// TextureAtlas allocation and deallocation
impl TextureAtlas {
    pub fn allocate(&self, size: [u32; 2]) -> Result<AtlasRegion, TextureAtlasError> {
        todo!()
    }

    fn deallocate(&self, id: RegionId) -> Result<(), DeallocationErrorTextureNotFound> {
        todo!()
    }
}

// for internal use only
impl TextureAtlas {
    fn get_location(&self, id: RegionId) -> Option<RegionLocation> {
        todo!()
    }

    fn textures(&self) -> &[wgpu::Texture] {
        todo!()
    }

    fn texture_views(&self) -> &[wgpu::TextureView] {
        todo!()
    }
}

struct TextureAtlasSolid {
    textures: Vec<wgpu::Texture>,
    texture_views: Vec<wgpu::TextureView>,
    layer_texture_views: Vec<Vec<wgpu::TextureView>>,
    size: wgpu::Extent3d,

    allocators: Vec<Mutex<AtlasAllocator>>,
    texture_id_to_location: DashMap<RegionId, RegionLocation>,
    texture_id_to_alloc_id: DashMap<RegionId, AllocId>,
    usage: std::sync::atomic::AtomicUsize,
}

impl TextureAtlasSolid {
    fn new(device: &wgpu::Device, size: wgpu::Extent3d, formats: &[wgpu::TextureFormat]) -> Self {
        let (textures, texture_views, layer_texture_views) =
            helper::create_texture_and_view(device, formats, size);
        Self {
            textures,
            texture_views,
            layer_texture_views,
            size,
            allocators: Vec::new(),
            texture_id_to_location: DashMap::new(),
            texture_id_to_alloc_id: DashMap::new(),
            usage: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn size(&self) -> wgpu::Extent3d {
        self.size
    }

    fn capacity(&self) -> usize {
        self.size.width as usize
            * self.size.height as usize
            * self.size.depth_or_array_layers as usize
    }

    fn usage(&self) -> usize {
        self.usage.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn max_allocation_size(&self) -> [u32; 2] {
        let mut max_size = [0; 2];

        for entry in self.texture_id_to_location.iter() {
            let size = entry.value().size();
            max_size[0] = max_size[0].max(size[0]);
            max_size[1] = max_size[1].max(size[1]);
        }
        max_size
    }
}

struct TextureAtlasResize {
    // The new atlas state
    new_textures: Option<Vec<wgpu::Texture>>,
    new_texture_views: Option<Vec<wgpu::TextureView>>,
    new_layer_texture_views: Option<Vec<Vec<wgpu::TextureView>>>,
    new_size: wgpu::Extent3d,

    new_allocators: Vec<Mutex<AtlasAllocator>>,
    new_texture_id_to_location: DashMap<RegionId, RegionLocation>,
    new_texture_id_to_alloc_id: DashMap<RegionId, AllocId>,
    new_usage: usize,

    // The previous atlas state
    old_textures: Vec<wgpu::Texture>,
    old_texture_views: Vec<wgpu::TextureView>,
    old_layer_texture_views: Vec<Vec<wgpu::TextureView>>,
    old_size: wgpu::Extent3d,
    old_allocators: Vec<Mutex<AtlasAllocator>>,
    old_texture_id_to_location: DashMap<RegionId, RegionLocation>,
    old_texture_id_to_alloc_id: DashMap<RegionId, AllocId>,
    old_usage: std::sync::atomic::AtomicUsize,
}

impl TextureAtlasResize {
    fn size(&self) -> wgpu::Extent3d {
        self.new_size
    }

    fn capacity(&self) -> usize {
        self.new_size.width as usize
            * self.new_size.height as usize
            * self.new_size.depth_or_array_layers as usize
    }

    fn usage(&self) -> usize {
        self.new_usage
    }

    fn max_allocation_size(&self) -> [u32; 2] {
        todo!()
    }
}

// helper functions
mod helper {
    use super::*;

    pub fn create_texture_and_view(
        device: &wgpu::Device,
        formats: &[wgpu::TextureFormat],
        page_size: wgpu::Extent3d,
    ) -> (
        Vec<wgpu::Texture>,
        Vec<wgpu::TextureView>,
        Vec<Vec<wgpu::TextureView>>,
    ) {
        let mut textures = Vec::with_capacity(formats.len());
        let mut texture_views = Vec::with_capacity(formats.len());
        let mut layer_texture_views = Vec::with_capacity(formats.len());

        for &format in formats {
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
            let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(&texture_view_label),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                aspect: wgpu::TextureAspect::All,
                ..wgpu::TextureViewDescriptor::default()
            });

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
                    ..wgpu::TextureViewDescriptor::default()
                });
                per_layer_views.push(layer_view);
            }

            textures.push(texture);
            texture_views.push(texture_view);
            layer_texture_views.push(per_layer_views);
        }

        (textures, texture_views, layer_texture_views)
    }

    // Leave unused args to make refactoring easier.
    pub fn copy_texture_data(
        encoder: &mut wgpu::CommandEncoder,
        old_textures: &[wgpu::Texture],
        _old_texture_views: &[wgpu::TextureView],
        new_textures: &[wgpu::Texture],
        _new_texture_views: &[wgpu::TextureView],
        location_map: impl Iterator<Item = (RegionLocation, RegionLocation)>,
    ) {
        for (old_location, new_location) in location_map {
            for (old_texture, new_texture) in old_textures.iter().zip(new_textures.iter()) {
                let old_origin = wgpu::Origin3d {
                    x: old_location.bounds.min.x as u32,
                    y: old_location.bounds.min.y as u32,
                    z: old_location.page_index,
                };

                let new_origin = wgpu::Origin3d {
                    x: new_location.bounds.min.x as u32,
                    y: new_location.bounds.min.y as u32,
                    z: new_location.page_index,
                };

                let size = old_location.size();

                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: old_texture,
                        mip_level: 0,
                        origin: old_origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: new_texture,
                        mip_level: 0,
                        origin: new_origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }
}

// old implementation of the texture atlas

/*

pub struct TextureAtlasOldImpl {
    textures: Vec<wgpu::Texture>,
    texture_views: Vec<wgpu::TextureView>,
    size: wgpu::Extent3d,
    formats: Vec<wgpu::TextureFormat>,

    state: TextureAtlasState,

    weak_self: Weak<RwLock<Self>>,
}

struct TextureAtlasState {
    allocators: Vec<Mutex<AtlasAllocator>>,
    texture_id_to_location: DashMap<TextureId, TextureLocation>,
    texture_id_to_alloc_id: DashMap<TextureId, AllocId>,
    usage: std::sync::atomic::AtomicUsize,
}

/// Constructor and information methods.
impl TextureAtlasOldImpl {
    pub fn new(
        device: &wgpu::Device,
        size: wgpu::Extent3d,
        formats: &[wgpu::TextureFormat],
    ) -> Arc<RwLock<Self>> {
        let (textures, texture_views) = Self::create_texture_and_view(device, formats, size);

        // Initialize the state with an empty allocator and allocation map.
        let state = TextureAtlasState {
            allocators: (0..size.depth_or_array_layers)
                .map(|_| Size::new(size.width as i32, size.height as i32))
                .map(AtlasAllocator::new)
                .map(Mutex::new)
                .collect(),
            texture_id_to_location: DashMap::new(),
            texture_id_to_alloc_id: DashMap::new(),
            usage: std::sync::atomic::AtomicUsize::new(0),
        };

        Arc::new_cyclic(|weak_self| {
            RwLock::new(Self {
                textures,
                texture_views,
                size,
                formats: formats.to_vec(),
                state,
                weak_self: weak_self.clone(),
            })
        })
    }

    pub fn size(&self) -> wgpu::Extent3d {
        self.size
    }

    pub fn formats(&self) -> &[wgpu::TextureFormat] {
        &self.formats
    }

    pub fn capacity(&self) -> usize {
        self.size.width as usize
            * self.size.height as usize
            * self.size.depth_or_array_layers as usize
    }

    pub fn usage(&self) -> usize {
        self.state.usage.load(std::sync::atomic::Ordering::SeqCst)
    }

    // todo: we can optimize this performance.
    pub fn max_allocation_size(&self) -> [u32; 2] {
        let mut max_size = [0; 2];

        for entry in self.state.texture_id_to_location.iter() {
            let size = entry.value().size();
            max_size[0] = max_size[0].max(size[0]);
            max_size[1] = max_size[1].max(size[1]);
        }
        max_size
    }
}

/// TextureAtlas allocation and deallocation
impl TextureAtlasOldImpl {
    /// Allocate a texture in the atlas.
    pub fn allocate(&self, size: [u32; 2]) -> Result<Texture, TextureAtlasError> {
        let size = Size::new(size[0] as i32, size[1] as i32);

        for (page_index, allocator) in self.state.allocators.iter().enumerate() {
            if let Some(alloc) = allocator.lock().allocate(size) {
                let bounds = alloc.rectangle;
                let uvs = euclid::Box2D::new(
                    euclid::Point2D::new(
                        (bounds.min.x as f32) / (self.size.width as f32),
                        (bounds.min.y as f32) / (self.size.height as f32),
                    ),
                    euclid::Point2D::new(
                        (bounds.max.x as f32) / (self.size.width as f32),
                        (bounds.max.y as f32) / (self.size.height as f32),
                    ),
                );
                let location = TextureLocation {
                    page_index: page_index as u32,
                    bounds,
                    uv: uvs,
                };

                // Create a new TextureId and Texture
                let texture_id = TextureId {
                    texture_uuid: Uuid::new_v4(),
                };
                let texture_inner = TextureInner {
                    texture_id,
                    atlas: self.weak_self.clone(),
                    size: [size.width as u32, size.height as u32],
                    formats: self.formats.clone(),
                };
                let texture = Texture {
                    inner: Arc::new(texture_inner),
                };

                // Store the texture location and allocation id in the atlas state.
                self.state
                    .texture_id_to_location
                    .insert(texture_id, location);
                self.state
                    .texture_id_to_alloc_id
                    .insert(texture_id, alloc.id);
                // Update usage
                // use `SeqCst` for safety, just for now. we can optimize this later.
                self.state.usage.fetch_add(
                    location.bounds.area() as usize,
                    std::sync::atomic::Ordering::SeqCst,
                );

                // Return the allocated texture
                return Ok(texture);
            }
        }

        Err(TextureAtlasError::AllocationFailedNotEnoughSpace)
    }

    /// Deallocate a texture from the atlas.
    /// This will be called automatically when the `TextureInner` is dropped.
    fn deallocate(&self, id: TextureId) -> Result<(), DeallocationErrorTextureNotFound> {
        // Find the texture location and remove it from the id-to-location map.
        let (_, location) = self
            .state
            .texture_id_to_location
            .remove(&id)
            .ok_or(DeallocationErrorTextureNotFound)?;

        // Find the allocation id and remove it from the id-to-alloc-id map.
        let (_, alloc_id) = self
            .state
            .texture_id_to_alloc_id
            .remove(&id)
            .ok_or(DeallocationErrorTextureNotFound)?;

        // Deallocate the texture from the allocator.
        self.state.allocators[location.page_index as usize]
            .lock()
            .deallocate(alloc_id);

        // Update usage
        // use `SeqCst` for safety, just for now. we can optimize this later.
        self.state.usage.fetch_sub(
            location.area() as usize,
            std::sync::atomic::Ordering::SeqCst,
        );

        Ok(())
    }
}

/// Resize the atlas to a new size.
impl TextureAtlasOldImpl {
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        new_size: wgpu::Extent3d,
        allow_data_loss: bool,
        new_allocation: Option<[u32; 2]>,
    ) -> Result<Option<Texture>, TextureAtlasError> {
        // mutable reference ensures we can modify the atlas state without threading issues.

        // new allocator and allocation map
        let mut new_allocators = (0..new_size.depth_or_array_layers)
            .map(|_| Size::new(new_size.width as i32, new_size.height as i32))
            .map(AtlasAllocator::new)
            .map(Mutex::new)
            .collect::<Vec<_>>();

        // Re-allocate existing textures
        let (new_texture_id_to_location, new_texture_id_to_alloc_id, mut new_usage) =
            self.reallocate_existing(&mut new_allocators, new_size, allow_data_loss)?;

        // Allocate for the new texture if requested
        let return_texture = if let Some(size) = new_allocation {
            match self.allocate_new(&mut new_allocators, new_size, size)? {
                Some(new_texture_data) => Some(new_texture_data),
                None => {
                    if !allow_data_loss {
                        return Err(TextureAtlasError::ResizeFailedNotEnoughSpace);
                    }
                    None
                }
            }
        } else {
            None
        };

        // Copy data from old textures to new textures
        let (new_textures, new_texture_views) =
            Self::create_texture_and_view(device, &self.formats, new_size);

        let location_map = new_texture_id_to_location.iter().map(|entry| {
            let id = entry.key();
            let new_location = entry.value();
            let old_location = self
                .state
                .texture_id_to_location
                .get(id)
                .expect("New allocation map was constructed from old allocation map, so it should contain all ids");
            (*old_location, *new_location)
        });

        let mut command_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("TextureAtlas Resize Command Encoder"),
        });

        Self::copy_texture_data(
            &mut command_encoder,
            &self.textures,
            &self.texture_views,
            &new_textures,
            &new_texture_views,
            location_map,
        );

        // Submit the command encoder to copy data
        queue.submit(Some(command_encoder.finish()));

        // Update the atlas state
        self.textures = new_textures;
        self.texture_views = new_texture_views;
        self.size = new_size;

        if let Some((ref texture, location, alloc_id)) = return_texture {
            new_texture_id_to_location.insert(texture.inner.texture_id, location);
            new_texture_id_to_alloc_id.insert(texture.inner.texture_id, alloc_id);
            new_usage += location.bounds.area() as usize;
        }

        self.state.allocators = new_allocators;
        self.state.texture_id_to_location = new_texture_id_to_location;
        self.state.texture_id_to_alloc_id = new_texture_id_to_alloc_id;
        self.state.usage = std::sync::atomic::AtomicUsize::new(new_usage);

        Ok(return_texture.map(|(texture, _, _)| texture))
    }

    #[allow(clippy::type_complexity)] // This function is for internal use only, so I think it's not a problem.
    fn reallocate_existing(
        &self,
        new_allocators: &mut [Mutex<AtlasAllocator>],
        new_atlas_size: wgpu::Extent3d,
        allow_data_loss: bool,
    ) -> Result<
        (
            DashMap<TextureId, TextureLocation>,
            DashMap<TextureId, AllocId>,
            usize,
        ),
        TextureAtlasError,
    > {
        let new_texture_id_to_location = DashMap::new();
        let new_texture_id_to_alloc_id = DashMap::new();
        let mut new_usage = 0;

        'for_each_textures: for (texture_id, size) in self
            .state
            .texture_id_to_location
            .iter()
            .map(|entry| (*entry.key(), entry.value().size()))
        {
            let allocate_request = Size::new(size[0] as i32, size[1] as i32);

            for (page_index, allocator) in new_allocators.iter_mut().enumerate() {
                if let Some(alloc) = allocator.lock().allocate(allocate_request) {
                    let bounds = alloc.rectangle;
                    let uvs = euclid::Box2D::new(
                        euclid::Point2D::new(
                            (bounds.min.x as f32) / (new_atlas_size.width as f32),
                            (bounds.min.y as f32) / (new_atlas_size.height as f32),
                        ),
                        euclid::Point2D::new(
                            (bounds.max.x as f32) / (new_atlas_size.width as f32),
                            (bounds.max.y as f32) / (new_atlas_size.height as f32),
                        ),
                    );

                    let location = TextureLocation {
                        page_index: page_index as u32,
                        bounds,
                        uv: uvs,
                    };

                    new_texture_id_to_location.insert(texture_id, location);
                    new_texture_id_to_alloc_id.insert(texture_id, alloc.id);
                    new_usage += location.bounds.area() as usize;
                    continue 'for_each_textures;
                }
            }

            // If we reach here, it means we couldn't allocate the texture in any page.
            if !allow_data_loss {
                return Err(TextureAtlasError::ResizeFailedNotEnoughSpace);
            }
        }

        Ok((
            new_texture_id_to_location,
            new_texture_id_to_alloc_id,
            new_usage,
        ))
    }

    fn allocate_new(
        &self,
        new_allocators: &mut [Mutex<AtlasAllocator>],
        new_atlas_size: wgpu::Extent3d,
        size: [u32; 2],
    ) -> Result<Option<(Texture, TextureLocation, AllocId)>, TextureAtlasError> {
        let allocate_request = Size::new(size[0] as i32, size[1] as i32);

        for (page_index, allocator) in new_allocators.iter_mut().enumerate() {
            if let Some(alloc) = allocator.lock().allocate(allocate_request) {
                let bounds = alloc.rectangle;
                let uvs = euclid::Box2D::new(
                    euclid::Point2D::new(
                        (bounds.min.x as f32) / (new_atlas_size.width as f32),
                        (bounds.min.y as f32) / (new_atlas_size.height as f32),
                    ),
                    euclid::Point2D::new(
                        (bounds.max.x as f32) / (new_atlas_size.width as f32),
                        (bounds.max.y as f32) / (new_atlas_size.height as f32),
                    ),
                );

                let location = TextureLocation {
                    page_index: page_index as u32,
                    bounds,
                    uv: uvs,
                };

                // Create a new TextureId and Texture
                let texture_id = TextureId {
                    texture_uuid: Uuid::new_v4(),
                };
                let texture = Texture {
                    inner: Arc::new(TextureInner {
                        texture_id,
                        atlas: self.weak_self.clone(),
                        size,
                        formats: self.formats.clone(),
                    }),
                };

                return Ok(Some((texture, location, alloc.id)));
            }
        }

        Ok(None)
    }
}


// helper functions
impl TextureAtlasOldImpl {
    fn create_texture_and_view(
        device: &wgpu::Device,
        formats: &[wgpu::TextureFormat],
        page_size: wgpu::Extent3d,
    ) -> (Vec<wgpu::Texture>, Vec<wgpu::TextureView>) {
        let mut textures = Vec::with_capacity(formats.len());
        let mut texture_views = Vec::with_capacity(formats.len());

        for &format in formats {
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
            let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(&texture_view_label),
                ..wgpu::TextureViewDescriptor::default()
            });
            textures.push(texture);
            texture_views.push(texture_view);
        }

        (textures, texture_views)
    }

    // Leave unused args to make refactoring easier.
    fn copy_texture_data(
        encoder: &mut wgpu::CommandEncoder,
        old_textures: &[wgpu::Texture],
        _old_texture_views: &[wgpu::TextureView],
        new_textures: &[wgpu::Texture],
        _new_texture_views: &[wgpu::TextureView],
        location_map: impl Iterator<Item = (TextureLocation, TextureLocation)>,
    ) {
        for (old_location, new_location) in location_map {
            for (old_texture, new_texture) in old_textures.iter().zip(new_textures.iter()) {
                let old_origin = wgpu::Origin3d {
                    x: old_location.bounds.min.x as u32,
                    y: old_location.bounds.min.y as u32,
                    z: old_location.page_index,
                };

                let new_origin = wgpu::Origin3d {
                    x: new_location.bounds.min.x as u32,
                    y: new_location.bounds.min.y as u32,
                    z: new_location.page_index,
                };

                let size = old_location.size();

                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: old_texture,
                        mip_level: 0,
                        origin: old_origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: new_texture,
                        mip_level: 0,
                        origin: new_origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }
}

*/

/// `DeallocationErrorTextureNotFound` only be used in this file.
struct DeallocationErrorTextureNotFound;

#[derive(Error, Debug)]
pub enum TextureAtlasError {
    #[error("Allocation failed because there was not enough space in the atlas.")]
    AllocationFailedNotEnoughSpace,
    #[error("Resizing the atlas failed because there was not enough space for all the textures.")]
    ResizeFailedNotEnoughSpace,
}
