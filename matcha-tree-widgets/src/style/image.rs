//! Decoding is CPU-owned and shared between clones. Prepared painters capture
//! decoded pixels and resolved geometry only; GPU storage belongs to generation
//! and the final renderer, never this style or a UI-owned atlas.
use std::sync::{Arc, OnceLock};

use crate::{
    paint::{self, ImageData},
    style::{PreparedStyle, Style},
};
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::{Constraints, QRect},
};
use parking_lot::Mutex;

use crate::types::size::{ChildSize, Size};

#[derive(Default)]
struct ImageCache {
    decoded: OnceLock<Option<Arc<ImageData>>>,
}

#[derive(Clone, PartialEq)]
pub enum ImageSource {
    Path(String),
    StaticSlice { data: &'static [u8] },
    Arc(Arc<Vec<u8>>),
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
    prepared: Mutex<Option<([u32; 6], PreparedStyle)>>,
}

impl Clone for Image {
    fn clone(&self) -> Self {
        Self {
            image: self.image.clone(),
            size: self.size.clone(),
            offset: self.offset.clone(),
            image_cache: self.image_cache.clone(),
            prepared: Mutex::new(None),
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
            prepared: Mutex::new(None),
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

// helper methods
impl Image {
    fn decoded(&self) -> Option<Arc<ImageData>> {
        self.image_cache
            .decoded
            .get_or_init(|| {
                let image = match &self.image {
                    ImageSource::Path(path) => image::open(path).ok(),
                    ImageSource::StaticSlice { data } => image::load_from_memory(data).ok(),
                    ImageSource::Arc(data) => image::load_from_memory(data).ok(),
                }?;
                Some(Arc::new(paint::image_data(image)))
            })
            .clone()
    }

    fn calc_layout(&self, boundary: [f32; 2], image: &ImageData, ctx: &UiContext) -> QRect {
        let image_size = image.size.map(|dimension| dimension as f32);

        let size_x = self.size[0].size(boundary, &mut ChildSize::new(|| image_size), ctx);
        let size_y = self.size[1].size(boundary, &mut ChildSize::new(|| image_size), ctx);
        let offset_x = self.offset[0].size(boundary, &mut ChildSize::new(|| image_size), ctx);
        let offset_y = self.offset[1].size(boundary, &mut ChildSize::new(|| image_size), ctx);

        QRect::new([offset_x, offset_y], [size_x, size_y])
    }

    fn prepare_resolved(
        &self,
        image: Arc<ImageData>,
        rect: QRect,
        offset: [f32; 2],
    ) -> PreparedStyle {
        let key = [
            rect.min_x(),
            rect.min_y(),
            rect.width(),
            rect.height(),
            offset[0],
            offset[1],
        ]
        .map(f32::to_bits);
        let mut cache = self.prepared.lock();
        if cache.as_ref().is_none_or(|(old, _)| *old != key) {
            let vertices = paint::rectangle(
                [rect.min_x(), rect.min_y()],
                [rect.width(), rect.height()],
                [1.; 4],
                offset,
            );
            *cache = Some((
                key,
                PreparedStyle::new(move |mut context| {
                    paint::draw(&mut context, &vertices, Some(&image))
                }),
            ));
        }
        cache
            .as_ref()
            .expect("resolved painter was cached")
            .1
            .clone()
    }
}

// MARK: Style implementation

impl Style for Image {
    fn required_region(&self, constraints: &Constraints, ctx: &UiContext) -> Option<QRect> {
        let boundary_size = constraints.max_size();

        self.decoded()
            .map(|image| self.calc_layout(boundary_size, &image, ctx))
    }

    fn is_inside(&self, position: [f32; 2], boundary_size: [f32; 2], ctx: &UiContext) -> bool {
        let draw_range = self.required_region(&Constraints::from_boundary(boundary_size), ctx);
        if let Some(rect) = draw_range {
            rect.contains(position)
        } else {
            false
        }
    }

    fn prepare(
        &self,
        boundary: [f32; 2],
        offset: [f32; 2],
        ctx: &UiContext,
    ) -> Option<PreparedStyle> {
        let image = self.decoded()?;
        let rect = self.calc_layout(boundary, &image, ctx);
        if rect.area() <= 0. || offset.iter().any(|value| !value.is_finite()) {
            return None;
        }
        Some(self.prepare_resolved(image, rect, offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_pixel() -> Vec<u8> {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([255, 0, 0, 128]),
        ));
        let mut output = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut output, image::ImageFormat::Png)
            .expect("encode fixture");
        output.into_inner()
    }

    #[test]
    fn clones_share_decoded_pixels_but_have_independent_prepared_identity() {
        let image = Image::new(encoded_pixel());
        let decoded = image.decoded().expect("valid encoded pixel");
        let copy = image.clone();
        let copied_pixels = copy.decoded().expect("shared decoded pixel");
        assert!(Arc::ptr_eq(&decoded, &copied_pixels));
        assert_eq!(decoded.size, [1, 1]);
        assert_eq!(decoded.pixels, [188, 0, 0, 128]);
        let rect = QRect::new([0., 0.], [1., 1.]);
        let first = image.prepare_resolved(decoded.clone(), rect, [0., 0.]);
        let warm = image.prepare_resolved(decoded, rect, [0., 0.]);
        assert_eq!(first.ids(), warm.ids());
        let independent = copy.prepare_resolved(copied_pixels, rect, [0., 0.]);
        assert_ne!(first.ids(), independent.ids());
    }

    #[test]
    fn resolved_layout_and_subpixel_output_offset_change_painter_identity() {
        let image = Image::new(encoded_pixel());
        let decoded = image.decoded().expect("valid fixture");
        let rect = QRect::new([0., 0.], [1., 1.]);
        let first = image.prepare_resolved(decoded.clone(), rect, [0., 0.]);
        let translated = image.prepare_resolved(decoded.clone(), rect, [1. / 4096., 0.]);
        assert_ne!(first.ids(), translated.ids());
        let resized =
            image.prepare_resolved(decoded, QRect::new([0., 0.], [2., 1.]), [1. / 4096., 0.]);
        assert_ne!(translated.ids(), resized.ids());
    }

    #[test]
    fn failed_decodes_are_cached_without_creating_gpu_resources() {
        let image = Image::new(vec![0, 1, 2]);
        assert!(image.decoded().is_none());
        assert!(image.image_cache.decoded.get().is_some());
        assert!(image.clone().decoded().is_none());
    }
}
