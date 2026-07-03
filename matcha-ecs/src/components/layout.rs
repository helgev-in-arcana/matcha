use bevy_ecs::component::Component;

#[derive(Component)]
pub struct SizeConstraints {
    pub x: [f32; 2],
    pub y: [f32; 2],
    pub z: [f32; 2],
}

#[derive(Component)]
pub struct GlobalTransform {
    pub affine: nalgebra::Matrix4<f32>,
}