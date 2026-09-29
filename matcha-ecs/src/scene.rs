//! Shared CPU drawing assembly. The ECS framework supplies resolved placement,
//! clipping and paint order; widgets emit Objects through the common writer.
//! The final Scene is owned by the framework and only borrowed by the renderer.
pub use render_interface::{Draw, Frame, push_quad, unit_quad};
