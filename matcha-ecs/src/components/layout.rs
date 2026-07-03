use bevy_ecs::component::Component;

/// Placeholder size constraints. Superseded by the real `Constraints` type in M3.
#[derive(Component)]
pub struct SizeConstraints {
    pub x: [f32; 2],
    pub y: [f32; 2],
    pub z: [f32; 2],
}

/// Absolute placement + scale of an entity in window (UI) space, applied by the
/// render stage as the transform for the entity's `RenderNode`.
#[derive(Component, Clone, Copy)]
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

/// A widget's desired absolute rectangle in window (UI) space.
///
/// M1 stand-in for real layout output: the throwaway placement system
/// (`crate::systems::temp_place`) reads it and writes a matching translation into
/// [`GlobalTransform`]. Removed in M3 when the real layout pass
/// (`Constraints`/`LayoutOutput`) lands.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct RectGeometry {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
