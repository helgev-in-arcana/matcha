use std::borrow::Cow;

use bevy_ecs::component::Component;

// window

#[derive(Component)]
struct Window {
    window_id: matcha_window::window::WindowId,
    window: matcha_window::window::Window,
}

#[derive(Component)]
pub struct WindowBelonging {
    pub window_id: matcha_window::window::WindowId,
}

// widget

#[derive(Component)]
pub struct WidgetId {
    pub id: Cow<'static, str>,
}

// about layout

#[derive(Component)]
pub struct RelativeTransform {
    pub nalgebra: nalgebra::Matrix4<f32>,
}

#[derive(Component)]
pub struct GlobalTransform {
    pub nalgebra: nalgebra::Matrix4<f32>,
}

#[derive(Component)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

// about rndering

#[derive(Component)]
pub struct RenderCache {
    // todo
}

// dirty flag

#[derive(Component)]
pub struct LayoutDirtyFlag {
    pub dirty: bool,
}

#[derive(Component)]
pub struct RenderDirtyFlag {
    pub dirty: bool,
}

// enable hit test

#[derive(Component)]
pub struct HitTestEnabled {
    pub enabled: bool,
}
