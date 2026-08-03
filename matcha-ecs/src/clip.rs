//! Clipping: turning [`Clip`] markers into the renderer's clip arena.
//!
//! A [`Clip`] confines an entity and its descendants to that entity's own box.
//! Because the extract stage flattens the tree into a list of per-entity render
//! nodes, the nesting between clips cannot be carried by the nodes themselves —
//! it is carried here instead, as an arena of rectangles each naming its parent.
//! The renderer turns a chain of those into a product of coverage masks.
//!
//! Everything in this module is GPU-free. A [`ClipRect`] is pure geometry; the
//! coverage image that gives it a shape is attached later, in the one place that
//! owns the atlas (see [`ClipMask`](crate::resources::ClipMask)). That split is
//! what lets clipping be tested headlessly.
//!
//! [`Clip`]: crate::components::layout::Clip

use bevy_ecs::{entity::Entity, world::World};
use nalgebra::{Matrix3, Matrix4, Vector3};

use crate::components::layout::{Clip, GlobalTransform, LayoutOutput};

/// One clip in a frame's arena.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipRect {
    /// The enclosing clip, if any. Always a **smaller index** than this rect's
    /// own — clips are pushed in tree order, so a parent is always seen first.
    pub parent: Option<u32>,
    /// Unit quad -> window space. Composed the same way a widget's own drawing
    /// transform is, so a clip is transformed with the tree it belongs to.
    pub transform: Matrix4<f32>,
}

impl ClipRect {
    /// Whether `point` (window space) falls inside this clip's box.
    ///
    /// Inverse-maps the point through the clip's planar homography and tests
    /// the unit square, so it is correct for a rotated or projectively
    /// transformed clip — an axis-aligned rectangle test is not. Half-open
    /// (`0.0 <= u < 1.0`), matching how a widget's own box is tested, so
    /// abutting boxes never both claim the same point.
    ///
    /// A degenerate transform contains nothing, and neither does a point behind
    /// the camera (`w <= 0`), matching how the shader rejects both.
    pub fn contains(&self, point: [f32; 2]) -> bool {
        let Some(inverse) = planar_homography(&self.transform).try_inverse() else {
            return false;
        };
        let h = inverse * Vector3::new(point[0], point[1], 1.0);
        if h.z <= 0.0 {
            return false;
        }
        let (u, v) = (h.x / h.z, h.y / h.z);
        (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v)
    }
}

/// A frame's clips, in the order the extract walk met them.
#[derive(Clone, Debug, Default)]
pub struct ClipArena {
    rects: Vec<ClipRect>,
}

impl ClipArena {
    /// Record a clip covering the box at `transform` of size `size`, nested
    /// inside `parent`. Returns its index, which descendants name as their own
    /// parent.
    pub fn push(&mut self, parent: Option<u32>, transform: Matrix4<f32>, size: [f32; 2]) -> u32 {
        let unit_quad_to_box =
            transform * Matrix4::new_nonuniform_scaling(&Vector3::new(size[0], size[1], 1.0));
        self.rects.push(ClipRect {
            parent,
            transform: unit_quad_to_box,
        });
        self.rects.len() as u32 - 1
    }

    pub fn as_slice(&self) -> &[ClipRect] {
        &self.rects
    }

    /// Whether `point` (window space) survives the clip at `index` and every
    /// clip enclosing it. `None` means unclipped, so everything survives.
    ///
    /// This is the same product-of-masks the fragment shader evaluates, with
    /// one term per chain element — which is why picking can answer it by
    /// walking the same arena the renderer is handed, rather than keeping a
    /// second, rectangle-shaped idea of what a clip is.
    pub fn contains(&self, index: Option<u32>, point: [f32; 2]) -> bool {
        let mut cursor = index;
        while let Some(i) = cursor {
            let Some(rect) = self.rects.get(i as usize) else {
                return false;
            };
            if !rect.contains(point) {
                return false;
            }
            cursor = rect.parent;
        }
        true
    }

    pub fn len(&self) -> usize {
        self.rects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// The chain of clips enclosing `index`, outermost first. Diagnostics and
    /// tests; the renderer resolves the same chain itself from the parent links.
    pub fn chain(&self, index: u32) -> Vec<u32> {
        let mut chain = Vec::new();
        let mut cursor = Some(index);
        while let Some(i) = cursor {
            chain.push(i);
            cursor = self.rects.get(i as usize).and_then(|r| r.parent);
        }
        chain.reverse();
        chain
    }
}

/// The clip that `entity`'s children sit inside, given the one handed down to
/// it: unchanged, unless `entity` declares a [`Clip`] of its own, in which case
/// its box is pushed onto `arena` and becomes the innermost.
///
/// Both walks over the tree — drawing ([`crate::render`]) and picking
/// ([`crate::pick`]) — go through here, so there is one answer to "which clips
/// apply to this entity" rather than one per consumer. A [`Clip`] covers the
/// declaring entity as well as its descendants, which is why the index this
/// returns is also the one the entity itself is recorded with.
///
/// An entity with no `GlobalTransform` or `LayoutOutput` has not been laid out;
/// its `Clip` is ignored rather than guessed at.
pub fn descend(
    arena: &mut ClipArena,
    world: &World,
    entity: Entity,
    inherited: Option<u32>,
) -> Option<u32> {
    if world.get::<Clip>(entity).is_none() {
        return inherited;
    }
    let (Some(transform), Some(layout)) = (
        world.get::<GlobalTransform>(entity),
        world.get::<LayoutOutput>(entity),
    ) else {
        return inherited;
    };
    Some(arena.push(inherited, transform.affine, layout.size))
}

/// The planar homography of a unit-quad transform: the restriction of `m` to
/// rows and columns `{0, 1, 3}`.
///
/// A mask's local coordinates are `(u, v, 0, 1)`, so only those rows and
/// columns ever contribute. Inverting this 3x3 is exact for any affine *or*
/// projective transform, where inverting the full 4x4 would presuppose that
/// the point lies on the mask's plane. `renderer` computes the same matrix for
/// the GPU; this is the CPU side of the identical question.
#[rustfmt::skip]
fn planar_homography(m: &Matrix4<f32>) -> Matrix3<f32> {
    Matrix3::new(
        m[(0, 0)], m[(0, 1)], m[(0, 3)],
        m[(1, 0)], m[(1, 1)], m[(1, 3)],
        m[(3, 0)], m[(3, 1)], m[(3, 3)],
    )
}
