pub mod image;
pub mod polygon;
pub mod solid_box;
// Disabled: depends on crates.io suzuri 0.2.1 (pins wgpu ^27), which blocks the
// workspace's wgpu 29 upgrade. Re-enable once suzuri supports wgpu 29.
// pub mod text;
pub mod viewport_clear;

use std::sync::Arc;

use gpu_utils::texture_atlas::atlas_simple::atlas::AtlasRegion;
use matcha_tree::ui_tree::{
    context::UiContext,
    metrics::{Constraints, QRect},
};

/// A trait that defines the visual appearance and drawing logic of a widget.
///
/// This allows for custom rendering logic to be encapsulated and reused.
pub trait Style: utils::MaybeSendSync {
    /// Calculates the size required to draw this style within the given constraints.
    ///
    /// This method returns the intrinsic size of the visual content defined by the style,
    /// such as the dimensions of an image or the bounding box of a piece of text,
    /// adjusted to fit within the provided `constraints`.
    /// The layout system uses this information to determine the widget's final size.
    ///
    /// # Parameters
    ///
    /// - `constraints`: The layout constraints (e.g., max width and height) that the style must adhere to.
    /// - `ctx`: The widget context, providing access to GPU resources and other shared data.
    ///
    /// # Returns
    ///
    /// An array `[width, height]` representing the required size in pixels.
    /// If the style does not have a specific size requirement, it returns `None`.
    fn required_region(&self, constraints: &Constraints, ctx: &UiContext) -> Option<QRect>;

    /// Checks if a given position is inside the shape defined by this style.
    /// This is necessary for styles that have non-rectangular shapes.
    fn is_inside(&self, position: [f32; 2], bounds: [f32; 2], ctx: &UiContext) -> bool {
        let Some(rect) = self.required_region(&Constraints::from_boundary(bounds), ctx) else {
            return false;
        };
        rect.contains(position)
    }

    /// Draws the style onto the render pass.
    ///
    /// - `offset`: The position of the upper left corner of the texture relative to the upper left corner of the boundary.
    /// - Coordinates are in pixels; the origin is the upper-left of the boundary and the Y axis points downwards.
    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &AtlasRegion,
        boundary_size: [f32; 2],
        offset: [f32; 2],
        ctx: &UiContext,
    );
}

impl Style for Vec<Arc<dyn Style>> {
    fn required_region(&self, constraints: &Constraints, ctx: &UiContext) -> Option<QRect> {
        let mut result: Option<QRect> = None;
        for style in self {
            if let Some(region) = style.required_region(constraints, ctx) {
                result = Some(match result {
                    Some(r) => r.union(&region),
                    None => region,
                });
            }
        }
        result
    }

    fn is_inside(&self, position: [f32; 2], bounds: [f32; 2], ctx: &UiContext) -> bool {
        for style in self {
            if style.is_inside(position, bounds, ctx) {
                return true;
            }
        }
        false
    }

    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &AtlasRegion,
        boundary_size: [f32; 2],
        offset: [f32; 2],
        ctx: &UiContext,
    ) {
        for style in self {
            style.draw(encoder, target, boundary_size, offset, ctx);
        }
    }
}
