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
