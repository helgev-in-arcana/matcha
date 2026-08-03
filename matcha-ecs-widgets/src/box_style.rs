//! The CSS box decoration model: background, border, corner radius, shadow —
//! one description and one painter, shared by every widget that draws a box.
//!
//! Before this, `Panel`, `Button`, `Checkbox` and `TextBox` each open-coded the
//! same "border-coloured box with an inset fill on top" trick against
//! `solid_rect_node`, and the scrollbar had a rounded-rect path of its own that
//! none of them could reach. [`box_node`] replaces all of it, and adds the
//! decorations none of them could express.
//!
//! # How each layer is drawn, and why it differs
//!
//! Everything paints a 1x1 tint texel stretched over a quad ([`ShapeCtx::tint_region`]);
//! what changes is the mask over it.
//!
//! - **Square, unbordered background** — no mask at all. This is the common
//!   case and it costs *nothing*: no rasterisation, no per-size atlas region.
//!   (The `solid_rect_node` it replaces allocated a full-size region and ran a
//!   render pass to fill it with one colour.)
//! - **Square border** — up to four plain quads, one per side. A coverage
//!   bitmap would work, but a 220x180 ring costs a 39 KB upload to say
//!   something four quads say exactly.
//! - **Anything rounded, and every shadow** — a coverage bitmap, cached by
//!   shape alone, composited with `RenderNode::with_stencil`. See
//!   [`crate::shape`] for why coverage rather than an RGBA image.
//!
//! So a widget only pays for a rasterisation when it actually asks for a curve
//! or a shadow. Corollary worth keeping in mind: `radius` is not free the way
//! the other properties are.
//!
//! # Deliberately not supported
//!
//! Per-*side* border colours (four separate rings; rare enough not to earn the
//! API), `inset` shadows, multiple shadows, gradients and background images
//! (each needs a painted region rather than a 1x1 tint, which would defeat the
//! colour-independent coverage cache — a real addition, not an oversight), and
//! `background-clip`/`background-origin` (the background always fills the
//! border box, CSS's default).

use nalgebra::{Matrix4, Vector3};
use renderer::RenderNode;

use matcha_ecs::components::render::RenderCtx;

use crate::shape::{CoverageKey, ShapeCtx};

/// Four per-corner values, in CSS order: top-left, top-right, bottom-right,
/// bottom-left.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Corners {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_right: f32,
    pub bottom_left: f32,
}

impl Corners {
    pub fn all(radius: f32) -> Self {
        Self {
            top_left: radius,
            top_right: radius,
            bottom_right: radius,
            bottom_left: radius,
        }
    }

    /// CSS `border-radius: <top> <bottom>` for a pill/tab shape.
    pub fn top(radius: f32) -> Self {
        Self {
            top_left: radius,
            top_right: radius,
            ..Self::default()
        }
    }

    pub fn is_zero(&self) -> bool {
        self.as_array().iter().all(|&r| r <= 0.0)
    }

    fn as_array(&self) -> [f32; 4] {
        [
            self.top_left,
            self.top_right,
            self.bottom_right,
            self.bottom_left,
        ]
    }
}

/// Four per-side values, in CSS order: top, right, bottom, left.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Sides {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Sides {
    pub fn all(width: f32) -> Self {
        Self {
            top: width,
            right: width,
            bottom: width,
            left: width,
        }
    }

    /// CSS `border-bottom` — what a divider or an underlined tab is made of.
    pub fn bottom(width: f32) -> Self {
        Self {
            bottom: width,
            ..Self::default()
        }
    }

    pub fn is_zero(&self) -> bool {
        self.as_array().iter().all(|&w| w <= 0.0)
    }

    fn as_array(&self) -> [f32; 4] {
        [self.top, self.right, self.bottom, self.left]
    }
}

/// CSS `box-shadow`, outer only.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BoxShadow {
    /// How far the shadow is displaced from the box, `[x, y]`.
    pub offset: [f32; 2],
    /// CSS's blur *radius*. The Gaussian sigma is half of it, as in CSS.
    pub blur: f32,
    /// Grow (or, negative, shrink) the shadow's shape before blurring.
    pub spread: f32,
    pub color: [f32; 4],
}

impl BoxShadow {
    /// A soft drop shadow directly below the box.
    pub fn drop(y: f32, blur: f32, color: [f32; 4]) -> Self {
        Self {
            offset: [0.0, y],
            blur,
            spread: 0.0,
            color,
        }
    }
}

/// Everything a widget can say about how its box is painted.
///
/// `Default` is fully transparent and undecorated, so a widget can start from
/// it and set only what it cares about.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct BoxStyle {
    pub background: [f32; 4],
    pub border: Sides,
    pub border_color: [f32; 4],
    pub radius: Corners,
    pub shadow: Option<BoxShadow>,
}

impl BoxStyle {
    /// A solid box of `color`, with nothing else.
    pub fn fill(color: [f32; 4]) -> Self {
        Self {
            background: color,
            ..Self::default()
        }
    }

    pub fn background(mut self, color: [f32; 4]) -> Self {
        self.background = color;
        self
    }

    /// A uniform border on all four sides.
    pub fn border(mut self, width: f32, color: [f32; 4]) -> Self {
        self.border = Sides::all(width);
        self.border_color = color;
        self
    }

    /// A border with independent per-side widths.
    pub fn borders(mut self, widths: Sides, color: [f32; 4]) -> Self {
        self.border = widths;
        self.border_color = color;
        self
    }

    /// A uniform corner radius. Costs a rasterised coverage bitmap per distinct
    /// (size, radius) — see this module's docs.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Corners::all(radius);
        self
    }

    pub fn corners(mut self, corners: Corners) -> Self {
        self.radius = corners;
        self
    }

    pub fn shadow(mut self, shadow: BoxShadow) -> Self {
        self.shadow = Some(shadow);
        self
    }

    /// The area left inside the border, as `(origin, size)` — where a child
    /// belongs, and what `Padding`-like layouts inset by.
    pub fn inner_box(&self, size: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        (
            [self.border.left, self.border.top],
            [
                (size[0] - self.border.left - self.border.right).max(0.0),
                (size[1] - self.border.top - self.border.bottom).max(0.0),
            ],
        )
    }
}

/// Paint `style` over a `size` box, back to front: shadow, background, border.
///
/// Returns an empty node for a degenerate size, so a caller never has to branch
/// on "is there anything to draw". Children are painted afterwards by the
/// extract stage, which is why a container's own decoration sits underneath
/// them without any ordering work here.
pub fn box_node(ctx: &RenderCtx, shape: &ShapeCtx, size: [f32; 2], style: &BoxStyle) -> RenderNode {
    let mut node = RenderNode::new();
    if size[0] < 0.5 || size[1] < 0.5 {
        return node;
    }

    if let Some(shadow) = style.shadow {
        if let Some((child, offset)) = shadow_node(ctx, shape, size, style, &shadow) {
            node.push_child(child, translation(offset));
        }
    }

    if style.background[3] > 0.0 {
        if let Some(child) = background_node(ctx, shape, size, style) {
            node.push_child(child, Matrix4::identity());
        }
    }

    if !style.border.is_zero() && style.border_color[3] > 0.0 {
        for (child, offset) in border_nodes(ctx, shape, size, style) {
            node.push_child(child, translation(offset));
        }
    }

    node
}

fn translation(offset: [f32; 2]) -> Matrix4<f32> {
    Matrix4::new_translation(&Vector3::new(offset[0], offset[1], 0.0))
}

/// A flat quad of `color` at `size`, sampling one shared tint texel.
fn tint_quad(ctx: &RenderCtx, shape: &ShapeCtx, size: [f32; 2], color: [f32; 4]) -> Option<RenderNode> {
    if size[0] < 0.5 || size[1] < 0.5 {
        return None;
    }
    let tint = shape.tint_region(color, ctx)?;
    Some(RenderNode::new().with_texture(tint, size, Matrix4::identity()))
}

fn background_node(
    ctx: &RenderCtx,
    shape: &ShapeCtx,
    size: [f32; 2],
    style: &BoxStyle,
) -> Option<RenderNode> {
    let quad = tint_quad(ctx, shape, size, style.background)?;
    if style.radius.is_zero() {
        return Some(quad);
    }

    // Rasterised at whole pixels and drawn at that same size, so the coverage
    // bitmap maps one texel per pixel and its antialiased edge is not resampled
    // soft.
    let key = CoverageKey::filled(
        size[0].round().max(1.0) as u32,
        size[1].round().max(1.0) as u32,
        style.radius.as_array(),
    );
    let coverage = shape.coverage_region(key, ctx)?;
    let drawn = [key.w as f32, key.h as f32];
    Some(
        tint_quad(ctx, shape, drawn, style.background)?
            .with_stencil(coverage, drawn, Matrix4::identity()),
    )
}

/// The border, as `(node, offset)` pairs.
///
/// A square border is up to four plain quads; a rounded one is a single ring
/// mask. Both are exact — the split is purely about what is cheaper.
fn border_nodes(
    ctx: &RenderCtx,
    shape: &ShapeCtx,
    size: [f32; 2],
    style: &BoxStyle,
) -> Vec<(RenderNode, [f32; 2])> {
    let color = style.border_color;
    let Sides {
        top,
        right,
        bottom,
        left,
    } = style.border;

    if !style.radius.is_zero() {
        let key = CoverageKey::ring(
            size[0].round().max(1.0) as u32,
            size[1].round().max(1.0) as u32,
            style.radius.as_array(),
            style.border.as_array(),
        );
        let drawn = [key.w as f32, key.h as f32];
        let (Some(quad), Some(coverage)) = (
            tint_quad(ctx, shape, drawn, color),
            shape.coverage_region(key, ctx),
        ) else {
            return Vec::new();
        };
        return vec![(quad.with_stencil(coverage, drawn, Matrix4::identity()), [0.0; 2])];
    }

    // Corners belong to one side each; giving the full width to the horizontal
    // bars and insetting the vertical ones is the simplest split that leaves no
    // gap and no double-painted corner (invisible for an opaque colour,
    // visible for a translucent one).
    let mid_h = (size[1] - top - bottom).max(0.0);
    let sides = [
        ([size[0], top], [0.0, 0.0]),
        ([size[0], bottom], [0.0, size[1] - bottom]),
        ([left, mid_h], [0.0, top]),
        ([right, mid_h], [size[0] - right, top]),
    ];

    sides
        .into_iter()
        .filter_map(|(quad_size, offset)| {
            Some((tint_quad(ctx, shape, quad_size, color)?, offset))
        })
        .collect()
}

/// The shadow, as `(node, offset)`.
///
/// Rasterised larger than the box by the spread plus room for the blur to fade
/// out in, then drawn shifted back by that margin so the shape stays centred on
/// the box before `offset` displaces it.
fn shadow_node(
    ctx: &RenderCtx,
    shape: &ShapeCtx,
    size: [f32; 2],
    style: &BoxStyle,
    shadow: &BoxShadow,
) -> Option<(RenderNode, [f32; 2])> {
    if shadow.color[3] <= 0.0 {
        return None;
    }

    // CSS's blur radius is two sigma. Three box-blur passes reach about three
    // sigma, which is where the falloff is no longer distinguishable from zero.
    let sigma = (shadow.blur / 2.0).max(0.0);
    let margin = (sigma * 3.0).ceil();

    let spread_size = [
        (size[0] + shadow.spread * 2.0).max(0.0),
        (size[1] + shadow.spread * 2.0).max(0.0),
    ];
    if spread_size[0] < 0.5 || spread_size[1] < 0.5 {
        return None;
    }

    let w = (spread_size[0] + margin * 2.0).round().max(1.0) as u32;
    let h = (spread_size[1] + margin * 2.0).round().max(1.0) as u32;

    // The spread grows the shape, so its corners grow with it — CSS does the
    // same, keeping the shadow's curve concentric with the box's.
    let radius = style
        .radius
        .as_array()
        .map(|r| if r > 0.0 { (r + shadow.spread).max(0.0) } else { 0.0 });

    // Rasterised centred in its own bitmap: the SDF is evaluated about the
    // bitmap's centre, so the margin has to be part of the shape's extent.
    // The shape is the spread box, inset from the bitmap by the blur margin so
    // the falloff has somewhere to go.
    let key = CoverageKey::filled(w, h, radius)
        .inset(margin)
        .blurred(sigma);
    let drawn = [w as f32, h as f32];

    let coverage = shape.coverage_region(key, ctx)?;
    let quad = tint_quad(ctx, shape, drawn, shadow.color)?;

    let offset = [
        shadow.offset[0] - shadow.spread - margin,
        shadow.offset[1] - shadow.spread - margin,
    ];
    Some((
        quad.with_stencil(coverage, drawn, Matrix4::identity()),
        offset,
    ))
}
