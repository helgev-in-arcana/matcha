//! Shared GPU context with optional compatibility atlases.
//!
//! GPU initialization is always available. Enable `legacy-atlas` only for the
//! UI-owned texture/buffer atlas APIs used by the old renderer and UI stacks.

#[cfg(feature = "legacy-atlas")]
pub mod buffer_atlas;
pub mod gpu;
mod gpu_defaults;
#[cfg(feature = "legacy-atlas")]
pub mod texture_atlas;

#[cfg(debug_assertions)]
pub mod wgpu_utils;
