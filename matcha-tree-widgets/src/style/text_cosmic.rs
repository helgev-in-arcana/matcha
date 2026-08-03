use crate::style::Style;
use cosmic_text::{Attrs, Buffer, Color, FontSystem, Metrics, Shaping, SwashCache};
use fxhash::FxHasher;
use gpu_utils::texture_atlas::atlas_simple::atlas::AtlasRegion;
use matcha_core::tree_app::context::UiContext;
use parking_lot::Mutex;
use renderer::widgets_renderer::texture_copy::{
    RenderData as TexRenderData, TargetData as TexTargetData, TextureCopy,
};

use std::hash::{Hash, Hasher};

struct FontContext {
    font_system: Mutex<FontSystem>,
    swash_cache: Mutex<SwashCache>,
}

impl Default for FontContext {
    fn default() -> Self {
        Self {
            font_system: Mutex::new(FontSystem::new()),
            swash_cache: Mutex::new(SwashCache::new()),
        }
    }
}

pub struct TextCosmic<'a> {
    pub texts: Vec<TextElement<'a>>,
    pub color: Color,
    pub metrics: Metrics,
    pub max_size: [Option<f32>; 2],
    pub buffer: Mutex<Option<Buffer>>,
    // cache in memory stores rendered RGBA bytes and a key used to detect invalidation
    pub cache_in_memory: Mutex<Option<CacheInMemory>>,
    // optional cached GPU texture for reuse (staging -> atlas fallback)
    pub cache_in_texture: Mutex<Option<wgpu::Texture>>,
    font_context: FontContext,
}

#[derive(Clone)]
pub struct TextElement<'a> {
    pub text: String,
    pub attrs: Attrs<'a>,
}

pub struct CacheInMemory {
    pub key: u64,
    pub size: [u32; 2],
    /// ! y-axis heads up
    pub text_offset: [i32; 2],
    pub data: Vec<u8>,
}

impl<'a> Clone for TextCosmic<'a> {
    fn clone(&self) -> Self {
        Self {
            texts: self.texts.clone(),
            color: self.color,
            metrics: self.metrics,
            max_size: self.max_size,
            buffer: Mutex::new(None),
            cache_in_memory: Mutex::new(None),
            cache_in_texture: Mutex::new(None),
            font_context: FontContext::default(),
        }
    }
}

impl<'a> TextCosmic<'a> {
    pub fn new(
        texts: Vec<TextElement<'a>>,
        color: Color,
        metrics: Metrics,
        max_size: [Option<f32>; 2],
    ) -> Self {
        Self {
            texts,
            color,
            metrics,
            max_size,
            buffer: Mutex::new(None),
            cache_in_memory: Mutex::new(None),
            cache_in_texture: Mutex::new(None),
            font_context: FontContext::default(),
        }
    }
}

impl TextCosmic<'_> {
    fn set_buffer(font_system: &mut FontSystem, metrics: Metrics) -> Buffer {
        Buffer::new(font_system, metrics)
    }

    fn make_cache_key(&self) -> u64 {
        // Compute a hash over texts, metrics, max_size and color.
        // Note: Attrs isn't hashed in detail here; if attribute changes must be detected,
        // include attributes serialization into the key (left as an improvement).
        let mut hasher = FxHasher::default();
        for t in &self.texts {
            t.text.hash(&mut hasher);
            // include a crude fingerprint of attrs by Debug formatting (safe fallback)
            format!("{:?}", t.attrs).hash(&mut hasher);
        }
        // Hash metrics via Debug (Metrics doesn't implement Hash)
        format!("{:?}", self.metrics).hash(&mut hasher);
        // max_size
        (self.max_size[0].unwrap_or(f32::NAN).to_bits()).hash(&mut hasher);
        (self.max_size[1].unwrap_or(f32::NAN).to_bits()).hash(&mut hasher);
        // color
        self.color.r().hash(&mut hasher);
        self.color.g().hash(&mut hasher);
        self.color.b().hash(&mut hasher);
        self.color.a().hash(&mut hasher);

        hasher.finish()
    }

    fn render_to_memory(
        texts: &[TextElement],
        color: Color,
        buffer: &mut Buffer,
        font_system: &mut FontSystem,
        swash_cache: &mut SwashCache,
        max_size: [Option<f32>; 2],
        key: u64,
    ) -> CacheInMemory {
        // ! y-axis heads down

        let mut buffer = buffer.borrow_with(font_system);

        buffer.set_size(
            max_size[0].unwrap_or(f32::MAX),
            max_size[1].unwrap_or(f32::MAX),
        );

        for text in texts {
            buffer.set_text(text.text.as_str(), text.attrs.clone(), Shaping::Advanced);
        }

        buffer.shape_until_scroll(true);

        let mut x_max = 0;
        let mut y_min = 0;
        let mut y_max = 0;

        for line in buffer.layout_runs() {
            if line.glyphs.is_empty() {
                continue;
            }

            x_max = x_max.max(line.line_w.ceil() as i32);
            y_min = y_min.min((line.line_y - line.line_top).floor() as i32);
            y_max = y_max.max(line.line_y.ceil() as i32);
        }

        let x_min = 0;
        let y_min = y_min;
        let width = (x_max - x_min).max(0) as usize;
        let height = (y_max - y_min).max(0) as usize;
        let size = [width, height];

        let mut data_rgba = vec![0u8; size[0].saturating_mul(size[1]).saturating_mul(4)];
        let data_offset = [x_min, y_min];

        if size[0] == 0 || size[1] == 0 {
            return CacheInMemory {
                key,
                size: [0, 0],
                text_offset: [0, 0],
                data: Vec::new(),
            };
        }

        buffer.draw(swash_cache, color, |x, y, _w, _h, color| {
            let x = (x - x_min) as usize;
            let y = (y - y_min) as usize;
            if let Some(index) = y
                .checked_mul(size[0])
                .and_then(|v| v.checked_add(x))
                .and_then(|v| v.checked_mul(4))
            {
                if index + 3 < data_rgba.len() {
                    data_rgba[index] = color.r();
                    data_rgba[index + 1] = color.g();
                    data_rgba[index + 2] = color.b();
                    data_rgba[index + 3] = color.a();
                }
            }
        });

        // ! change y-axis heads up
        let data_offset = [data_offset[0], -data_offset[1]];

        CacheInMemory {
            key,
            size: [size[0] as u32, size[1] as u32],
            text_offset: [data_offset[0], data_offset[1]],
            data: data_rgba,
        }
    }

    fn render_to_texture(&self, size: [u32; 2], data: &[u8], ctx: &UiContext) -> wgpu::Texture {
        let device = ctx.gpu_device();
        let queue = ctx.gpu_queue();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("TextCosmic Texture"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Ensure bytes_per_row aligns to wgpu's required COPY_BYTES_PER_ROW_ALIGNMENT (usually 256)
        let bytes_per_pixel = 4u32;
        let unaligned_bytes_per_row = size[0].saturating_mul(bytes_per_pixel);
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = if unaligned_bytes_per_row == 0 {
            0
        } else {
            ((unaligned_bytes_per_row + align - 1) / align) * align
        };

        if padded_bytes_per_row == 0 {
            // empty texture (width or height 0) - nothing to upload
            return texture;
        }

        // copy into a padded buffer so bytes_per_row meets alignment requirements
        let padded_row_bytes = padded_bytes_per_row as usize;
        let src_row_bytes = (size[0] as usize) * (bytes_per_pixel as usize);
        let mut padded_data = vec![0u8; padded_row_bytes * (size[1] as usize)];
        for y in 0..(size[1] as usize) {
            let src_off = y * src_row_bytes;
            let dst_off = y * padded_row_bytes;
            padded_data[dst_off..dst_off + src_row_bytes]
                .copy_from_slice(&data[src_off..src_off + src_row_bytes]);
        }

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &padded_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(size[1]),
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );

        texture
    }

    fn draw_range(
        &self,
        boundary_size: [f32; 2],
        _ctx: &UiContext,
    ) -> matcha_core::tree_app::metrics::QRect {
        let mut font_system = self.font_context.font_system.lock();

        let mut buffer = Buffer::new(&mut font_system, self.metrics);

        let max_width = self.max_size[0].unwrap_or(boundary_size[0]);
        let max_height = self.max_size[1].unwrap_or(boundary_size[1]);

        let mut buffer_view = buffer.borrow_with(&mut font_system);

        buffer_view.set_size(max_width, max_height);

        for text in &self.texts {
            buffer_view.set_text(&text.text, text.attrs.clone(), Shaping::Advanced);
        }

        buffer_view.shape_until_scroll(true);

        let mut x_max = 0f32;
        let mut y_min = 0f32;
        let mut y_max = 0f32;

        for line in buffer_view.layout_runs() {
            if line.glyphs.is_empty() {
                continue;
            }
            x_max = x_max.max(line.line_w);
            y_min = y_min.min(line.line_y - line.line_top);
            y_max = y_max.max(line.line_y);
        }

        let width = x_max;
        let height = y_max - y_min;

        match (width > 0.0, height > 0.0) {
            (true, true) => matcha_core::metrics::QRect::new([0.0, -y_min], [width, height]),
            _ => matcha_core::metrics::QRect::zero(),
        }
    }
}

impl Style for TextCosmic<'static> {
    fn required_region(
        &self,
        constraints: &matcha_core::tree_app::metrics::Constraints,
        ctx: &UiContext,
    ) -> Option<matcha_core::tree_app::metrics::QRect> {
        let rect = self.draw_range(constraints.max_size(), ctx);
        if rect.area() > 0.0 { Some(rect) } else { None }
    }

    fn is_inside(&self, position: [f32; 2], boundary_size: [f32; 2], ctx: &UiContext) -> bool {
        let draw_range = self.draw_range(boundary_size, ctx);
        let x_range = draw_range.x();
        let y_range = draw_range.y();

        x_range[0] <= position[0]
            && position[0] <= x_range[1]
            && y_range[0] <= position[1]
            && position[1] <= y_range[1]
    }

    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &AtlasRegion,
        _boundary_size: [f32; 2],
        _offset: [f32; 2],
        ctx: &UiContext,
    ) {
        let font_system = &self.font_context.font_system;
        let swash_cache = &self.font_context.swash_cache;

        // compute key for current state
        let current_key = self.make_cache_key();

        let mut buffer_lock = self.buffer.lock();
        let mut cache_in_memory_lock = self.cache_in_memory.lock();
        let mut cache_in_texture_lock = self.cache_in_texture.lock();

        if buffer_lock.is_none() {
            *buffer_lock = Some(Self::set_buffer(&mut font_system.lock(), self.metrics));
        }
        let buffer = buffer_lock.as_mut().unwrap();

        // regenerate in-memory cache if missing or key mismatch
        let need_regen = match cache_in_memory_lock.as_ref() {
            Some(c) => c.key != current_key,
            None => true,
        };

        if need_regen {
            // invalidate cached GPU texture if we re-render in memory
            *cache_in_texture_lock = None;

            *cache_in_memory_lock = Some(Self::render_to_memory(
                &self.texts,
                self.color,
                buffer,
                &mut font_system.lock(),
                &mut swash_cache.lock(),
                self.max_size,
                current_key,
            ));
        }

        let cache_in_memory = cache_in_memory_lock.as_ref().unwrap();

        // Nothing to draw
        if cache_in_memory.size[0] == 0 || cache_in_memory.size[1] == 0 {
            return;
        }

        // Try to write raw RGBA data into the atlas region using the queue (preferred, cheaper).
        let data = &cache_in_memory.data;
        let write_result = target.write_data(ctx.gpu_queue(), data.as_slice());

        if write_result.is_ok() {
            // success, atlas now contains the bitmap
            return;
        }

        // Fallback: if write_data failed, try to create or reuse a GPU texture and sample-draw it into the atlas
        // This uses the TextureCopy renderer to blit the temporary texture into the atlas via a render pass.
        // Create or reuse cached GPU texture for the rendered bitmap
        let tex = match cache_in_texture_lock.as_ref() {
            Some(t) => t.clone(),
            None => {
                let t = self.render_to_texture(cache_in_memory.size, &cache_in_memory.data, ctx);
                *cache_in_texture_lock = Some(t.clone());
                t
            }
        };

        // begin a render pass targeting the atlas region so the renderer can create its own passes if needed
        let mut render_pass = match target.begin_render_pass(encoder) {
            Ok(rp) => rp,
            Err(_) => return,
        };

        // Use TextureCopy to render the temporary texture into the atlas region.
        let texture_copy = TextureCopy::default();
        texture_copy.render(
            &mut render_pass,
            TexTargetData {
                target_size: target.size(),
                target_format: target.format(),
            },
            TexRenderData {
                source_texture_view: &tex.create_view(&wgpu::TextureViewDescriptor::default()),
                source_texture_position_min: [0.0, 0.0],
                source_texture_position_max: [
                    cache_in_memory.size[0] as f32,
                    cache_in_memory.size[1] as f32,
                ],
                color_transformation: None,
                color_offset: None,
            },
            ctx.gpu_device(),
        );
    }
}
