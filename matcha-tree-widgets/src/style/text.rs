use std::num::NonZeroUsize;

use gpu_utils::texture_atlas::atlas_simple::atlas::AtlasRegion;
use matcha_tree::{
    color::Color,
    ui_tree::{
        context::UiContext,
        metrics::{Constraints, QSize},
    },
};

use suzuri::font_system::FontSystem;
use suzuri::renderer::gpu_renderer::GpuCacheConfig;
use utils::cache::RwCache;
use utils::rwoption::RwOption;

pub use suzuri::fontdb::{Family, Stretch, Style, Weight};

pub struct TextSpan<'a> {
    pub text: &'a str,
    pub size: f32,
    pub families: &'a [Family<'a>],
    pub weight: Weight,
    pub stretch: Stretch,
    pub style: Style,
    pub color: Color,
}

/// Internal use.
#[derive(Clone, PartialEq)]
enum OwnedFamily {
    Name(String),
    Serif,
    SansSerif,
    Cursive,
    Fantasy,
    Monospace,
}

/// Internal use.
#[derive(Clone, PartialEq)]
struct OwnedTextSpan {
    text: String,
    size: f32,
    families: Vec<OwnedFamily>,
    weight: Weight,
    stretch: Stretch,
    style: Style,
    premultiplied_color: [f32; 4],
}

impl From<TextSpan<'_>> for OwnedTextSpan {
    fn from(span: TextSpan<'_>) -> Self {
        let straight_color = span.color.to_rgba_f32();
        let premultiplied_color = [
            straight_color[0] * straight_color[3],
            straight_color[1] * straight_color[3],
            straight_color[2] * straight_color[3],
            straight_color[3],
        ];

        Self {
            text: span.text.to_string(),
            size: span.size,
            families: span
                .families
                .iter()
                .map(|f| match f {
                    Family::Name(name) => OwnedFamily::Name(name.to_string()),
                    Family::Serif => OwnedFamily::Serif,
                    Family::SansSerif => OwnedFamily::SansSerif,
                    Family::Cursive => OwnedFamily::Cursive,
                    Family::Fantasy => OwnedFamily::Fantasy,
                    Family::Monospace => OwnedFamily::Monospace,
                })
                .collect(),
            weight: span.weight,
            stretch: span.stretch,
            style: span.style,
            premultiplied_color,
        }
    }
}

/// **To prevent deadlock, must lock in the order of font_system -> swash_cache -> cache -> text_atlas.**
struct TextShared {
    font_system: FontSystem,
}

impl TextShared {
    fn setup(device: &wgpu::Device, _queue: &wgpu::Queue) -> Self {
        let font_system = FontSystem::new();

        font_system.load_system_fonts();

        font_system.wgpu_init(
            device,
            #[allow(clippy::unwrap_used)]
            &[
                GpuCacheConfig {
                    tile_size: NonZeroUsize::new(32).unwrap(),
                    tiles_per_axis: NonZeroUsize::new(32).unwrap(),
                    texture_size: NonZeroUsize::new(1024).unwrap(),
                },
                GpuCacheConfig {
                    tile_size: NonZeroUsize::new(64).unwrap(),
                    tiles_per_axis: NonZeroUsize::new(16).unwrap(),
                    texture_size: NonZeroUsize::new(1024).unwrap(),
                },
                GpuCacheConfig {
                    tile_size: NonZeroUsize::new(128).unwrap(),
                    tiles_per_axis: NonZeroUsize::new(8).unwrap(),
                    texture_size: NonZeroUsize::new(1024).unwrap(),
                },
                GpuCacheConfig {
                    tile_size: NonZeroUsize::new(256).unwrap(),
                    tiles_per_axis: NonZeroUsize::new(4).unwrap(),
                    texture_size: NonZeroUsize::new(1024).unwrap(),
                },
            ],
            &[
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ],
        );

        Self { font_system }
    }
}

pub struct TextRenderer {
    owned_texts: Vec<OwnedTextSpan>,
    text_data: RwOption<suzuri::text::TextData<[f32; 4]>>,
    layout_config: suzuri::text::TextLayoutConfig,
    layout_cache: RwCache<Constraints, suzuri::text::TextLayout<[f32; 4]>>,
    texture_cache: RwCache<QSize, wgpu::Texture>,
    text_shared: RwOption<TextShared>,
    texture_copy: renderer::texture_copy::TextureCopy,
}

impl PartialEq for TextRenderer {
    fn eq(&self, other: &Self) -> bool {
        self.owned_texts == other.owned_texts
    }
}

impl Clone for TextRenderer {
    fn clone(&self) -> Self {
        Self {
            owned_texts: self.owned_texts.clone(),
            text_data: RwOption::new(),
            layout_config: self.layout_config.clone(),
            layout_cache: RwCache::new(),
            texture_cache: RwCache::new(),
            text_shared: RwOption::new(),
            texture_copy: renderer::texture_copy::TextureCopy::default(),
        }
    }
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    pub fn new() -> Self {
        Self {
            owned_texts: Vec::new(),
            text_data: RwOption::new(),
            layout_config: suzuri::text::TextLayoutConfig::default(),
            layout_cache: RwCache::new(),
            texture_cache: RwCache::new(),
            text_shared: RwOption::new(),
            texture_copy: renderer::texture_copy::TextureCopy::default(),
        }
    }

    pub fn push_span(&mut self, span: TextSpan<'_>) {
        let owned_span = OwnedTextSpan::from(span);
        self.owned_texts.push(owned_span);
    }

    pub fn set_layout_config(&mut self, config: suzuri::text::TextLayoutConfig) {
        self.layout_config = config;
    }

    /// Updates text spans from another `TextRenderer`, **preserving the initialized
    /// `FontSystem`** (`text_shared`) so system-font loading is not repeated on
    /// every frame.  Derived caches (`text_data`, `layout_cache`, `texture_cache`)
    /// are invalidated because the content changed.
    pub fn set_spans_from(&mut self, other: &TextRenderer) {
        self.owned_texts = other.owned_texts.clone();
        self.layout_config = other.layout_config.clone();
        // Invalidate content-derived caches.
        self.text_data = RwOption::new();
        self.layout_cache = RwCache::new();
        self.texture_cache = RwCache::new();
        // `text_shared` (FontSystem) and `texture_copy` are kept as-is.
    }
}

impl TextRenderer {
    fn build_text_data(
        owned_texts: &[OwnedTextSpan],
        font_system: &FontSystem,
    ) -> suzuri::text::TextData<[f32; 4]> {
        let mut text_data = suzuri::text::TextData::new();

        for span in owned_texts {
            let OwnedTextSpan {
                text,
                size,
                families,
                weight,
                stretch,
                style,
                premultiplied_color,
            } = span;

            let family = families
                .iter()
                .map(|f| match f {
                    OwnedFamily::Name(name) => Family::Name(name.as_str()),
                    OwnedFamily::Serif => Family::Serif,
                    OwnedFamily::SansSerif => Family::SansSerif,
                    OwnedFamily::Cursive => Family::Cursive,
                    OwnedFamily::Fantasy => Family::Fantasy,
                    OwnedFamily::Monospace => Family::Monospace,
                })
                .collect::<smallvec::SmallVec<[_; 10]>>();

            let Some((font_id, _)) = font_system.query(&suzuri::fontdb::Query {
                families: &family,
                weight: *weight,
                stretch: *stretch,
                style: *style,
            }) else {
                todo!();
            };

            text_data.append(suzuri::text::TextElement {
                font_id,
                font_size: *size,
                content: text.clone(),
                user_data: *premultiplied_color,
            });
        }

        text_data
    }

    fn build_layout(
        constraints: &Constraints,
        text_data: &suzuri::text::TextData<[f32; 4]>,
        font_system: &FontSystem,
    ) -> suzuri::text::TextLayout<[f32; 4]> {
        let max_size = constraints.max_size();

        let layout_config = suzuri::text::TextLayoutConfig {
            max_width: Some(max_size[0]),
            max_height: Some(max_size[1]),
            horizontal_align: suzuri::text::HorizontalAlign::Left,
            vertical_align: suzuri::text::VerticalAlign::Top,
            line_height_scale: 1.0,
            wrap_style: suzuri::text::WrapStyle::WordWrap,
            wrap_hard_break: false,
            word_separators: " ".chars().collect(),
            linebreak_char: ['\n'].into_iter().collect(),
        };

        font_system.layout_text(text_data, &layout_config)
    }

    fn build_texture(
        text_layout: &suzuri::text::TextLayout<[f32; 4]>,
        font_system: &FontSystem,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
    ) -> wgpu::Texture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Text Texture"),
            size: wgpu::Extent3d {
                width: text_layout.total_width as u32,
                height: text_layout.total_height as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        font_system.wgpu_render(text_layout, device, encoder, &texture_view);

        texture
    }
}

impl crate::style::Style for TextRenderer {
    fn required_region(
        &self,
        constraints: &matcha_tree::ui_tree::metrics::Constraints,
        ctx: &UiContext,
    ) -> Option<matcha_tree::ui_tree::metrics::QRect> {
        let shared = self
            .text_shared
            .get_or_insert_with(|| TextShared::setup(ctx.gpu_device(), ctx.gpu_queue()));

        let TextShared { font_system } = &*shared;

        let text_data = self
            .text_data
            .get_or_insert_with(|| Self::build_text_data(&self.owned_texts, font_system));

        let text_layout = self.layout_cache.get_or_insert_with(constraints, || {
            Self::build_layout(constraints, &text_data, font_system)
        });
        let (_, layout) = &*text_layout;

        let size = [layout.total_width, layout.total_height];

        let size = [
            (size[0] + 0.5).min(constraints.max_width()),
            (size[1] + 0.5).min(constraints.max_height()),
        ];

        Some(matcha_tree::ui_tree::metrics::QRect::new([0.0, 0.0], size))
    }

    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &AtlasRegion,
        boundary_size: [f32; 2],
        offset: [f32; 2],
        ctx: &UiContext,
    ) {
        let boundary_size = [boundary_size[0] + 0.5, boundary_size[1] + 0.5];

        let q_size = QSize::from(boundary_size);
        let constraints = Constraints::from_max_q_size(q_size);

        let shared = self
            .text_shared
            .get_or_insert_with(|| TextShared::setup(ctx.gpu_device(), ctx.gpu_queue()));

        let TextShared { font_system } = &*shared;

        let text_data = self
            .text_data
            .get_or_insert_with(|| Self::build_text_data(&self.owned_texts, font_system));

        let text_layout = self.layout_cache.get_or_insert_with(&constraints, || {
            Self::build_layout(&constraints, &text_data, font_system)
        });
        let (_, text_layout) = &*text_layout;

        let texture = self.texture_cache.get_or_insert_with(&q_size, || {
            Self::build_texture(text_layout, font_system, encoder, ctx.gpu_device())
        });

        let (_, texture) = &*texture;

        let Ok(mut render_pass) = target.begin_render_pass(encoder) else {
            log::error!("Failed to begin render pass");
            todo!()
        };

        self.texture_copy.render(
            &mut render_pass,
            renderer::texture_copy::TargetData {
                target_size: target.texture_size(),
                target_format: target.format(),
            },
            renderer::texture_copy::RenderData {
                source_texture_view: &texture.create_view(&wgpu::TextureViewDescriptor::default()),
                source_texture_position_min: offset,
                source_texture_position_max: [
                    offset[0] + texture.size().width as f32,
                    offset[1] + texture.size().height as f32,
                ],
                color_transformation: None,
                color_offset: None,
            },
            ctx.gpu_device(),
        );
    }
}
