use std::sync::Arc;

use crate::style::Style;
use gpu_utils::texture_atlas::atlas_simple::atlas::AtlasRegion;
use matcha_tree::ui_tree::{context::UiContext, metrics::QRect};
use utils::cache::Cache;

pub struct Buffer {
    style: Vec<Arc<dyn Style>>,
    cache: Cache<[f32; 2], Option<BufferData>>,
}

pub struct BufferData {
    pub texture: AtlasRegion,
    pub texture_position: QRect,
}

impl Buffer {
    pub fn new(style: Vec<Arc<dyn Style>>) -> Self {
        Self {
            style,
            cache: Cache::new(),
        }
    }

    pub fn is_inside(&self, position: [f32; 2], boundary_size: [f32; 2], ctx: &UiContext) -> bool {
        for style in &self.style {
            if style.is_inside(position, boundary_size, ctx) {
                return true;
            }
        }
        false
    }

    pub fn render(
        &mut self,
        boundary: [f32; 2],
        encoder: &mut wgpu::CommandEncoder,
        ctx: &UiContext,
    ) -> Option<&BufferData> {
        let (_, cache) = self.cache.get_or_insert_with(&boundary, || {
            // calculate necessary size for the texture
            let mut x_min = f32::MAX;
            let mut x_max = f32::MIN;
            let mut y_min = f32::MAX;
            let mut y_max = f32::MIN;

            for style in &self.style {
                let range = style.required_region(
                    &matcha_tree::ui_tree::metrics::Constraints::from_boundary(boundary),
                    ctx,
                );
                let Some(range) = range else {
                    continue;
                };
                x_min = x_min.min(range.min_x());
                x_max = x_max.max(range.max_x());
                y_min = y_min.min(range.min_y());
                y_max = y_max.max(range.max_y());
            }

            if x_min >= x_max || y_min >= y_max {
                // No styles required any space.
                return None;
            }

            let texture_position = QRect::new([x_min, y_min], [x_max - x_min, y_max - y_min]);

            let x_min_int = x_min.floor() as i32;
            let x_max_int = x_max.ceil() as i32;
            let y_min_int = y_min.floor() as i32;
            let y_max_int = y_max.ceil() as i32;

            // create a texture with the calculated size

            let texture_size = [
                (x_max_int - x_min_int) as u32,
                (y_max_int - y_min_int) as u32,
            ];

            // Allocate a region in the texture atlas and render each style into it.
            // We unwrap here because allocation failure is unexpected in normal operation.
            let atlas_region = ctx
                .texture_atlas()
                .allocate(ctx.gpu_device(), ctx.gpu_queue(), texture_size)
                .expect("Texture atlas allocation failed for Buffer");

            for style in &self.style {
                style.draw(
                    encoder,
                    &atlas_region,
                    boundary,
                    [x_min_int as f32, y_min_int as f32],
                    ctx,
                );
            }

            Some(BufferData {
                texture: atlas_region,
                texture_position,
            })
        });

        cache.as_ref()
    }
}
