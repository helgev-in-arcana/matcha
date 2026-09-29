//! World resources shared across systems and the render stage.

use std::sync::Arc;

use crate::render::GuiRenderer;
use bevy_ecs::{entity::Entity, resource::Resource, world::World};
use parking_lot::Mutex;

use matcha_window::window::WindowId;

use crate::components::window::Window;

/// The GPU context (instance / device / queue).
#[derive(Resource)]
pub struct GpuResource {
    pub gpu: gpu_utils::gpu::Gpu,
}

/// UI assembly state and backend cache, serialized across render workers.
#[derive(Resource)]
pub struct RendererResource {
    pub core: Arc<Mutex<GuiRenderer>>,
}

/// The UI-root window entity and its id. The application manages one root;
/// the render stage walks that entity's `ViewChildren`.
///
/// [`ui_root`] and [`ui_root_window`] centralize root lookup for layout, input
/// and rendering.
#[derive(Resource)]
pub struct RenderWindowRoot {
    pub entity: Entity,
    pub window_id: WindowId,
}

/// The entity whose view tree is the UI, or `None` before the window exists.
///
/// Layout, picking, focus validation and tab order all need exactly this and
/// nothing else. This helper centralizes the root-selection policy.
pub fn ui_root(world: &World) -> Option<Entity> {
    world.get_resource::<RenderWindowRoot>().map(|r| r.entity)
}

/// The UI root together with its window, for the systems that push state *to*
/// the OS window — the cursor shape, IME enablement, the size to lay out
/// against.
///
/// `None` if there is no root yet, or if the root entity has lost its
/// [`Window`] component (which happens between `resumed` and surface creation).
pub fn ui_root_window(world: &World) -> Option<(Entity, &Window)> {
    let root = ui_root(world)?;
    Some((root, world.get::<Window>(root)?))
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
