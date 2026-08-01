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

/// Marker: this entity and its subtree are absent — CSS `display: none`.
///
/// Absent, not merely invisible: the entity takes no place in its parent's
/// layout at all, so a `Column`'s `gap` and a `justify-content` distribution
/// skip it exactly as if it had never been declared. Giving it a zero size
/// would not do that.
///
/// The layout side is enforced in one place, [`LayoutCtx::children`](crate::layout::LayoutCtx::children)
/// (plus [`layout_root`](crate::layout::layout_root), which reads
/// [`ViewChildren`](crate::components::view::ViewChildren) directly), so every
/// container gets it without knowing it exists. Drawing and picking need their
/// own guard: a hidden entity keeps whatever [`GlobalTransform`] it was last
/// laid out with, so a walk that did not skip it would keep painting it at its
/// old position.
///
/// Re-showing costs no stale frame: `MatchaSet::Layout` runs before
/// `MatchaSet::Extract`, so the entity is re-arranged before it is next drawn.
#[derive(Component, Clone, Copy)]
pub struct Hidden;

/// The result of laying out one entity: its allocated size and its origin
/// (top-left) relative to its parent's coordinate space. Written by
/// [`crate::layout::LayoutCtx::arrange_child`].
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct LayoutOutput {
    pub size: [f32; 2],
    pub origin: [f32; 2],
}
