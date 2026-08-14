pub mod core_renderer;
pub use core_renderer::{CoreRenderer, FlatItem, MaskNode};
pub mod pipeline_cache;
pub mod render_node;
pub use render_node::RenderNode;

pub mod debug_renderer;
pub use debug_renderer::DebugRenderer;

pub mod vertex;

// Helper renderers for the legacy `matcha-tree` stack, which is native-only.
// They still use immediates directly and have no uniform fallback; nothing in
// the ECS stack reaches them (its atlas uploads all go through
// `AtlasRegion::write_data`), so they are simply absent on the web.
#[cfg(not(web))]
pub mod widgets_renderer;
#[cfg(not(web))]
pub use widgets_renderer::{bezier_2d, line_strip, texture_color, texture_copy, vertex_color};
