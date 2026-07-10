//! World resources shared across systems and the render stage.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, resource::Resource};
use gpu_utils::texture_atlas::TextureAtlas;
use renderer::CoreRenderer;

use matcha_window::window::WindowId;

/// The GPU context (instance / device / queue).
#[derive(Resource)]
pub struct GpuResource {
    pub gpu: gpu_utils::gpu::Gpu,
}

/// Shared rendering resources: the core wgpu pipeline plus the color and stencil
/// texture atlases. All `Arc` so future render tasks (M4) can share them.
#[derive(Resource)]
pub struct RendererResource {
    pub core: Arc<CoreRenderer>,
    pub texture_atlas: Arc<TextureAtlas>,
    pub stencil_atlas: Arc<TextureAtlas>,
}

/// The single UI-root window entity and its id. M1 supports one window; this is
/// how the render stage finds the entity whose `ViewChildren` to walk.
#[derive(Resource)]
pub struct RenderWindowRoot {
    pub entity: Entity,
    pub window_id: WindowId,
}
