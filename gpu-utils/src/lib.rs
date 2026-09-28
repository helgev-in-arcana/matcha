//! Shared GPU context with optional texture and buffer atlases.
//!
//! GPU initialization is always available. Enable `atlas` for the independent
//! atlas APIs. Current tree/ECS rendering uses plain-renderer-owned placement.

#[cfg(feature = "atlas")]
pub mod buffer_atlas;
pub mod gpu;
mod gpu_defaults;
#[cfg(feature = "atlas")]
pub mod texture_atlas;

#[cfg(debug_assertions)]
pub mod wgpu_utils;
