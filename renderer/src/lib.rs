//! Compatibility renderer for the existing tree/ECS UI stacks.
//!
//! Enable `legacy` to use these APIs. New renderer development lives in the
//! independent `scene-renderer` crate; this crate has no default runtime dependencies.

#[cfg(feature = "legacy")]
pub mod core_renderer;
#[cfg(feature = "legacy")]
pub use core_renderer::{CoreRenderer, FlatItem, MaskNode};
#[cfg(feature = "legacy")]
pub mod pipeline_cache;
#[cfg(feature = "legacy")]
pub mod render_node;
#[cfg(feature = "legacy")]
pub use render_node::RenderNode;

#[cfg(feature = "legacy")]
pub mod debug_renderer;
#[cfg(feature = "legacy")]
pub use debug_renderer::DebugRenderer;

#[cfg(feature = "legacy")]
pub mod vertex;

#[cfg(feature = "legacy")]
pub mod widgets_renderer;
#[cfg(feature = "legacy")]
pub use widgets_renderer::{bezier_2d, line_strip, texture_color, texture_copy, vertex_color};
