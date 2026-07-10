use bevy_ecs::component::Component;

/// Absolute placement + scale of an entity in window (UI) space, applied by the
/// render stage as the transform for the entity's `RenderNode`.
#[derive(Component, Clone, Copy, PartialEq)]
pub struct GlobalTransform {
    pub affine: nalgebra::Matrix4<f32>,
}

impl Default for GlobalTransform {
    fn default() -> Self {
        Self {
            affine: nalgebra::Matrix4::identity(),
        }
    }
}

/// The result of laying out one entity: its allocated size and its origin
/// (top-left) relative to its parent's coordinate space. Written by
/// [`crate::layout::LayoutCtx::arrange_child`].
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct LayoutOutput {
    pub size: [f32; 2],
    pub origin: [f32; 2],
}
