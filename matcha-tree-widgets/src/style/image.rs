use std::sync::Arc;

use crate::style::Style;
use dashmap::DashMap;
use gpu_utils::device_loss_recoverable::DeviceLossRecoverable;
use image::EncodableLayout;
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::{Constraints, QRect},
};
use renderer::widgets_renderer::texture_copy::{RenderData, TargetData, TextureCopy};

use crate::types::size::{ChildSize, Size};

#[derive(Default)]
struct ImageCache {
    map: DashMap<ImageCacheKey, ImageCacheData, fxhash::FxBuildHasher>,
}

impl DeviceLossRecoverable for ImageCache {
    fn recover(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) {
        log::info!("ImageCache: recovering from device loss");
        self.map.clear();
    }
}

#[derive(Clone, PartialEq)]
pub enum ImageSource {
    Path(String),
    StaticSlice { data: &'static [u8] },
    Arc(Arc<Vec<u8>>),
}

impl ImageSource {
    fn to_key(&self) -> ImageCacheKey {
        match self {
            ImageSource::Path(path) => ImageCacheKey::Path(path.clone()),
            ImageSource::StaticSlice { data } => ImageCacheKey::StaticSlice {
                ptr: data.as_ptr() as usize,
                size: data.len(),
            },
            ImageSource::Arc(data) => ImageCacheKey::Arc {
                ptr: Arc::as_ptr(data) as usize,
                size: data.len(),
            },
        }
    }
}

impl From<&str> for ImageSource {
    fn from(path: &str) -> Self {
        ImageSource::Path(path.to_string())
    }
}

impl From<String> for ImageSource {
    fn from(path: String) -> Self {
        ImageSource::Path(path)
    }
}

impl<const N: usize> From<&'static [u8; N]> for ImageSource {
    fn from(data: &'static [u8; N]) -> Self {
        ImageSource::StaticSlice { data }
    }
}

impl From<Arc<Vec<u8>>> for ImageSource {
    fn from(data: Arc<Vec<u8>>) -> Self {
        ImageSource::Arc(data)
    }
}

impl From<Vec<u8>> for ImageSource {
    fn from(data: Vec<u8>) -> Self {
        ImageSource::Arc(Arc::new(data))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ImageCacheKey {
    /// Full path to the image file
    Path(String),
    /// pointer address (as usize) and size of the image data
    /// This is safe because the data is guaranteed to be static
    StaticSlice {
        ptr: usize,
        size: usize,
    },
    Arc {
        ptr: usize,
        size: usize,
    },
}

struct ImageCacheData {
    /// None if the image failed to load
    texture: Option<wgpu::Texture>,
}

// MARK: Image Construct

pub enum HAlign {
    Left,
    Center,
    Right,
}

pub enum VAlign {
    Top,
    Center,
    Bottom,
}

pub struct Image {
    image: ImageSource,
    size: [Size; 2],
    offset: [Size; 2],
    image_cache: Arc<ImageCache>,
}

impl Clone for Image {
    fn clone(&self) -> Self {
        Self {
            image: self.image.clone(),
            size: self.size.clone(),
            offset: self.offset.clone(),
            image_cache: self.image_cache.clone(),
        }
    }
}

impl PartialEq for Image {
    fn eq(&self, other: &Self) -> bool {
        self.image == other.image && self.size == other.size && self.offset == other.offset
    }
}

impl Image {
    pub fn new(source: impl Into<ImageSource>) -> Self {
        Self {
            image: source.into(),
            size: [Size::child_w(1.0), Size::child_h(1.0)],
            offset: [Size::px(0.0), Size::px(0.0)],
            image_cache: Arc::new(ImageCache::default()),
        }
    }

    pub fn stretch_to_boundary(mut self) -> Self {
        self.size = [Size::parent_w(1.0), Size::parent_h(1.0)];
        self
    }

    /// Set absolute size in pixels.
    pub fn size_px(mut self, w: f32, h: f32) -> Self {
        self.size = [Size::px(w), Size::px(h)];
        self
    }

    /// Set width in pixels, keep height as is.
    pub fn size_px_w(mut self, w: f32) -> Self {
        let h = self.size[1].clone();
        self.size = [Size::px(w), h];
        self
    }

    /// Set height in pixels, keep width as is.
    pub fn size_px_h(mut self, h: f32) -> Self {
        let w = self.size[0].clone();
        self.size = [w, Size::px(h)];
        self
    }

    /// Set size as percentage of parent (percent values, e.g. 50.0 == 50%).
    pub fn size_percent(mut self, w_percent: f32, h_percent: f32) -> Self {
        self.size = [
            Size::parent_w(w_percent / 100.0),
            Size::parent_h(h_percent / 100.0),
        ];
        self
    }

    /// Set absolute offset in pixels.
    pub fn offset_px(mut self, x: f32, y: f32) -> Self {
        self.offset = [Size::px(x), Size::px(y)];
        self
    }

    /// Set offset as percentage of parent (percent values).
    pub fn offset_percent(mut self, x_percent: f32, y_percent: f32) -> Self {
        self.offset = [
            Size::parent_w(x_percent / 100.0),
            Size::parent_h(y_percent / 100.0),
        ];
        self
    }

    /// Align center both axes.
    pub fn align_center(mut self) -> Self {
        let ox = Size::from_size(|parent, child, _ctx| (parent[0] - child.get()[0]) * 0.5);
        let oy = Size::from_size(|parent, child, _ctx| (parent[1] - child.get()[1]) * 0.5);
        self.offset = [ox, oy];
        self
    }

    /// Align horizontally (left/center/right) with optional margin (Size).
    pub fn align_h(mut self, align: HAlign, margin: Size) -> Self {
        let m1 = margin.clone();
        let m2 = margin.clone();
        let m3 = margin.clone();
        let ox = match align {
            HAlign::Left => Size::from_size(move |parent, _child, ctx| {
                m1.size(parent, &mut ChildSize::default(), ctx)
            }),
            HAlign::Center => Size::from_size(move |parent, child, ctx| {
                (parent[0] - child.get()[0]) * 0.5 + m2.size(parent, child, ctx)
            }),
            HAlign::Right => Size::from_size(move |parent, child, ctx| {
                (parent[0] - child.get()[0]) - m3.size(parent, child, ctx)
            }),
        };
        let oy = self.offset[1].clone();
        self.offset = [ox, oy];
        self
    }

    /// Align vertically (top/center/bottom) with optional margin (Size).
    pub fn align_v(mut self, align: VAlign, margin: Size) -> Self {
        let m1 = margin.clone();
        let m2 = margin.clone();
        let m3 = margin.clone();
        let oy = match align {
            VAlign::Top => Size::from_size(move |parent, _child, ctx| {
                m1.size(parent, &mut ChildSize::default(), ctx)
            }),
            VAlign::Center => Size::from_size(move |parent, child, ctx| {
                (parent[1] - child.get()[1]) * 0.5 + m2.size(parent, child, ctx)
            }),
            VAlign::Bottom => Size::from_size(move |parent, child, ctx| {
                (parent[1] - child.get()[1]) - m3.size(parent, child, ctx)
            }),
        };
        let ox = self.offset[0].clone();
        self.offset = [ox, oy];
        self
    }

    /// Generic anchor: sets horizontal and vertical alignment with margins.
    pub fn anchor(mut self, halign: HAlign, valign: VAlign, margin: [Size; 2]) -> Self {
        self = self.align_h(halign, margin[0].clone());
        self = self.align_v(valign, margin[1].clone());
        self
    }

    pub fn size(mut self, size: [Size; 2]) -> Self {
        self.size = size;
        self
    }

    pub fn offset(mut self, offset: [Size; 2]) -> Self {
        self.offset = offset;
        self
    }
}

impl Image {
    fn key(&self) -> ImageCacheKey {
        self.image.to_key()
    }
}

// helper methods
impl Image {
    fn with_image<R>(&self, ctx: &UiContext, f: impl FnOnce(&wgpu::Texture) -> R) -> Option<R> {
        let image_cache = self
            .image_cache
            .map
            .entry(self.key())
            .or_insert_with(|| load_image_to_texture(&self.image, ctx));

        let Some(image) = &image_cache.value().texture else {
            return None;
        };
        Some(f(image))
    }

    fn calc_layout(
        &self,
        boundary: [f32; 2],
        pic_texture: &wgpu::Texture,
        ctx: &UiContext,
    ) -> QRect {
        let image_size = [pic_texture.width() as f32, pic_texture.height() as f32];

        let size_x = self.size[0].size(boundary, &mut ChildSize::new(|| image_size), ctx);
        let size_y = self.size[1].size(boundary, &mut ChildSize::new(|| image_size), ctx);
        let offset_x = self.offset[0].size(boundary, &mut ChildSize::new(|| image_size), ctx);
        let offset_y = self.offset[1].size(boundary, &mut ChildSize::new(|| image_size), ctx);

        QRect::new([offset_x, offset_y], [size_x, size_y])
    }
}

// MARK: Style implementation

impl Style for Image {
    fn required_region(&self, constraints: &Constraints, ctx: &UiContext) -> Option<QRect> {
        let boundary_size = constraints.max_size();

        self.with_image(ctx, |texture| self.calc_layout(boundary_size, texture, ctx))
    }

    fn is_inside(&self, position: [f32; 2], boundary_size: [f32; 2], ctx: &UiContext) -> bool {
        let draw_range = self.required_region(&Constraints::from_boundary(boundary_size), ctx);
        if let Some(rect) = draw_range {
            rect.contains(position)
        } else {
            false
        }
    }

    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &gpu_utils::texture_atlas::atlas_simple::atlas::AtlasRegion,
        boundary_size: [f32; 2],
        offset: [f32; 2],
        ctx: &UiContext,
    ) {
        let target_size = target.texture_size();
        let target_format = target.format();
        self.with_image(ctx, |texture| {
            let rect: QRect = self.calc_layout(boundary_size, texture, ctx);

            let draw_offset = [rect.min_x() - offset[0], rect.min_y() - offset[1]];
            let draw_size = [rect.width(), rect.height()];

            let mut render_pass = match target.begin_render_pass(encoder) {
                Ok(rp) => rp,
                Err(_) => return,
            };

            let texture_copy = TextureCopy::default();
            texture_copy.render(
                ctx.gpu_queue(),
                &mut render_pass,
                TargetData {
                    target_size,
                    target_format,
                },
                RenderData {
                    source_texture_view: &texture
                        .create_view(&wgpu::TextureViewDescriptor::default()),
                    source_texture_position_min: [draw_offset[0], draw_offset[1]],
                    source_texture_position_max: [
                        draw_offset[0] + draw_size[0],
                        draw_offset[1] + draw_size[1],
                    ],
                    color_transformation: None,
                    color_offset: None,
                },
                ctx.gpu_device(),
            );
        });
    }
}

fn load_image_to_texture(image_source: &ImageSource, ctx: &UiContext) -> ImageCacheData {
    let dynamic_image = match image_source {
        ImageSource::Path(path) => image::open(path).ok(),
        ImageSource::StaticSlice { data, .. } => image::load_from_memory(data).ok(),
        ImageSource::Arc(data) => image::load_from_memory(data).ok(),
    };

    let Some(dynamic_image) = dynamic_image else {
        return ImageCacheData { texture: None };
    };

    let (image, format) = prepare_image_and_format(dynamic_image);
    ImageCacheData {
        texture: Some(make_cache(image, format, ctx)),
    }
}

fn prepare_image_and_format(
    dynamic_image: image::DynamicImage,
) -> (
    image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    wgpu::TextureFormat,
) {
    let image_rgba8: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> = dynamic_image.to_rgba8();
    (image_rgba8, wgpu::TextureFormat::Rgba8UnormSrgb)
}

fn make_cache(
    image: image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    format: wgpu::TextureFormat,
    ctx: &UiContext,
) -> wgpu::Texture {
    let (width, height) = image.dimensions();
    let data = image.as_bytes();

    let device = ctx.gpu_device();
    let queue = ctx.gpu_queue();

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Image Texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    texture
}

#[rustfmt::skip]
fn _color_transform(color_type: image::ColorType) -> nalgebra::Matrix4<f32> {
    match color_type {
        image::ColorType::L8
        | image::ColorType::L16 => nalgebra::Matrix4::new(
            1.0, 0.0, 0.0, 0.0,
            1.0, 0.0, 0.0, 0.0,
            1.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0,
        ),
        image::ColorType::La8
        | image::ColorType::La16 => nalgebra::Matrix4::new(
            1.0, 0.0, 0.0, 0.0,
            1.0, 0.0, 0.0, 0.0,
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
        ),
        image::ColorType::Rgb8
        | image::ColorType::Rgb16
        | image::ColorType::Rgb32F
        | image::ColorType::Rgba8
        | image::ColorType::Rgba16
        | image::ColorType::Rgba32F => nalgebra::Matrix4::new(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ),
        _ => todo!(),
    }
}

fn _color_offset(color_type: image::ColorType) -> [f32; 4] {
    match color_type {
        image::ColorType::L8 | image::ColorType::L16 => [0.0, 0.0, 0.0, 1.0],
        image::ColorType::La8
        | image::ColorType::La16
        | image::ColorType::Rgb8
        | image::ColorType::Rgb16
        | image::ColorType::Rgb32F
        | image::ColorType::Rgba8
        | image::ColorType::Rgba16
        | image::ColorType::Rgba32F => [0.0, 0.0, 0.0, 0.0],
        _ => todo!(),
    }
}
