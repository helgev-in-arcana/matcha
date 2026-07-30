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

use nalgebra::{Matrix4, Vector3};

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

/// Intersect two window-space rectangles, each `[min_x, min_y, max_x, max_y]`.
/// `None` when nothing survives.
///
/// Picking uses this to trim a widget's rectangle to the clips around it, so
/// something clipped out of sight stops being clickable too — a clip that only
/// affected drawing would leave invisible click targets behind. Picking carries
/// the running intersection rather than a [`ClipArena`] index: it only ever
/// needs the innermost result, never the chain that produced it.
pub fn intersect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let out = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (out[0] < out[2] && out[1] < out[3]).then_some(out)
}
