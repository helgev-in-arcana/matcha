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

/// The current frame's timestamp, refreshed once per frame before the render
/// schedule runs so every system this frame agrees on "now".
#[derive(Resource, Clone, Copy)]
pub struct FrameTime(pub web_time::Instant);

/// "This frame is not the final one — schedule another."
///
/// Reset to `false` before each render-schedule run; any system may call
/// [`RedrawRequest::request`] during the frame to ask the event loop for a
/// follow-up redraw once the frame is presented. This is how an ongoing
/// animation (which lives outside the core, in a registered PreLayout system)
/// keeps itself running without the core having to know anything about it.
#[derive(Resource, Default)]
pub struct RedrawRequest {
    requested: bool,
}

impl RedrawRequest {
    /// Ask for another frame after this one.
    pub fn request(&mut self) {
        self.requested = true;
    }

    /// Whether a follow-up frame was requested during the current frame.
    pub fn is_requested(&self) -> bool {
        self.requested
    }

    pub(crate) fn reset(&mut self) {
        self.requested = false;
    }
}
