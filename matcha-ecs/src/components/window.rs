use bevy_ecs::{component::Component, entity::Entity};

#[derive(Component)]
pub struct Window {
    pub window: matcha_window::window::Window,
}

#[derive(Component)]
pub struct WindowBelonging {
    pub window_id: matcha_window::window::WindowId,
    pub window_entity: Entity,
}

/// Physical pixels per UI pixel for the window this sits on — the display's
/// scale factor.
///
/// A window reports its size, and delivers pointer positions, in *physical*
/// pixels. Layout and picking work in UI pixels. On a desktop at 100% those
/// are the same thing, which is why nothing needed this before; on a HiDPI
/// display or any browser with `devicePixelRatio != 1` they are not, and a UI
/// laid out directly against physical pixels comes out at the wrong size while
/// picking resolves clicks against rects that do not match the screen.
///
/// Per window rather than a global resource because it genuinely differs per
/// window: two displays of different densities give the same app two different
/// factors at the same time. `resumed` seeds it from the window's own reported
/// scale factor and `ScaleFactorChanged` updates the window it names, so an
/// app never supplies this itself.
///
/// Dividing physical by this yields UI pixels. The framebuffer stays at full
/// physical resolution — only the coordinate system is scaled, so text and
/// edges keep their sharpness.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
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
