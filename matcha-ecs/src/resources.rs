//! World resources shared across systems and the render stage.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, resource::Resource, world::World};
use gpu_utils::texture_atlas::TextureAtlas;
use renderer::CoreRenderer;

use matcha_window::window::WindowId;

use crate::components::window::Window;

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

/// The coverage image every rectangular clip is drawn with: a single fully
/// opaque texel, stretched over the clip's box by its own transform.
///
/// One texel is enough because a mask's shape comes from its transform, and the
/// shader rejects anything outside the mask's unit square rather than clamping
/// to its edge. So a rectangular clip of any size costs no allocation at all,
/// and the whole application shares this one region. Non-rectangular clips will
/// want their own coverage images, or an analytic mask kind.
///
/// Allocated on first use, from the one place that owns the atlas.
#[derive(Resource, Clone)]
pub struct ClipMask {
    pub region: gpu_utils::texture_atlas::AtlasRegion,
}

/// The single UI-root window entity and its id. M1 supports one window; this is
/// how the render stage finds the entity whose `ViewChildren` to walk.
///
/// Prefer [`ui_root`] and [`ui_root_window`] over reading this directly: the
/// singleton is temporary, and every site that spells out the lookup is a site
/// that has to change when it stops being one.
#[derive(Resource)]
pub struct RenderWindowRoot {
    pub entity: Entity,
    pub window_id: WindowId,
}

/// The entity whose view tree is the UI, or `None` before the window exists.
///
/// Layout, picking, focus validation and tab order all need exactly this and
/// nothing else. Going through one function keeps "which root?" answerable in
/// one place — which is the whole preparation for there being more than one.
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

/// Physical pixels per UI pixel — the display's scale factor.
///
/// The window reports its size, and delivers pointer positions, in *physical*
/// pixels. Layout and hit-testing work in UI pixels. On a desktop at 100% those
/// are the same thing, which is why nothing needed this before; on a HiDPI
/// display or any browser with `devicePixelRatio != 1` they are not, and a UI
/// laid out directly against physical pixels comes out at the wrong size.
///
/// Dividing physical by this yields UI pixels. The framebuffer itself stays at
/// full physical resolution — only the coordinate system is scaled, so text and
/// edges keep their sharpness.
///
/// Defaults to 1.0, which reproduces the previous behaviour exactly.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct UiScale(pub f32);

impl Default for UiScale {
    fn default() -> Self {
        Self(1.0)
    }
}

impl UiScale {
    /// Physical pixels -> UI pixels.
    pub fn to_ui(self, physical: [f32; 2]) -> [f32; 2] {
        [physical[0] / self.0, physical[1] / self.0]
    }
}

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
