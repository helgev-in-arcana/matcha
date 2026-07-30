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

/// Marker: confine drawing to this entity's own box — CSS `overflow: hidden`.
///
/// Applies to the entity **itself** as well as everything below it. Covering
/// itself matters for a widget that draws its own content directly rather than
/// through a child (a text box painting glyphs, say); it costs nothing for the
/// usual container, whose background and border sit inside its box anyway.
///
/// Nesting works: an entity inside two clips is confined to the intersection.
///
/// This is a purely visual and hit-testing bound — layout is unaffected, so a
/// child still measures and arranges exactly as it would without it, and simply
/// does not paint outside.
#[derive(Component, Clone, Copy)]
pub struct Clip;

/// The result of laying out one entity: its allocated size and its origin
/// (top-left) relative to its parent's coordinate space. Written by
/// [`crate::layout::LayoutCtx::arrange_child`].
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct LayoutOutput {
    pub size: [f32; 2],
    pub origin: [f32; 2],
}
