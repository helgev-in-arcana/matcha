//! `ColorRect` — a fixed-size solid colour rectangle, positioned by whatever
//! layout it is nested under.
//!
//! It is a layout *leaf*: `measure` returns its own `w`×`h` (clamped to the
//! incoming constraints) and ignores children; `arrange` does nothing further
//! since it has none. Its [`RenderItem`] rasterises a single-colour quad into
//! the texture atlas.

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    world::EntityWorldMut,
};
use nalgebra::{Matrix4, Point3};
use renderer::{
    vertex::colored_vertex::ColorVertex,
    vertex_color::{RenderData, TargetData, VertexColor},
    RenderNode,
};

use matcha_ecs::{
    components::{
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch},
    view::Widget,
};

/// A [`ColorRect`]'s requested (unconstrained) size.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct RectGeometry {
    pub w: f32,
    pub h: f32,
}

/// The RGBA fill colour of a [`ColorRect`], carried so `patch` can detect changes.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct RectColor(pub [f32; 4]);

/// A solid-colour rectangle of fixed size.
pub struct ColorRect {
    key: Key,
    w: f32,
    h: f32,
    color: [f32; 4],
}

impl ColorRect {
    /// Create a `w`×`h` rectangle (white by default).
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            key: Key::Auto,
            w,
            h,
            color: [1.0, 1.0, 1.0, 1.0],
        }
    }

    /// Set the RGBA fill colour (components in `0.0..=1.0`).
    pub fn color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    /// Override the reconciliation key.
    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn geometry(&self) -> RectGeometry {
        RectGeometry {
            w: self.w,
            h: self.h,
        }
    }
}

/// Build a `RenderItem` that rasterises a `w`×`h` solid `color` quad into the
/// colour atlas and returns a textured `RenderNode` (positioned later by the
/// entity's `GlobalTransform`).
fn make_render_item(w: f32, h: f32, color: [f32; 4]) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let node = RenderNode::new();
        if w <= 0.0 || h <= 0.0 {
            return node;
        }

        let size_px = [w.ceil() as u32, h.ceil() as u32];
        let region = match ctx.texture_atlas.allocate(ctx.device, ctx.queue, size_px) {
            Ok(region) => region,
            Err(e) => {
                log::error!("ColorRect atlas allocation failed: {e}");
                return node;
            }
        };

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ColorRect Render Encoder"),
            });

        let target_size = region.texture_size();
        let target_format = region.format();
        if let Ok(mut render_pass) = region.begin_render_pass(&mut encoder) {
            let vertices = [
                ColorVertex {
                    position: Point3::new(0.0, 0.0, 0.0),
                    color,
                },
                ColorVertex {
                    position: Point3::new(w, 0.0, 0.0),
                    color,
                },
                ColorVertex {
                    position: Point3::new(w, h, 0.0),
                    color,
                },
                ColorVertex {
                    position: Point3::new(0.0, h, 0.0),
                    color,
                },
            ];
            let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];

            VertexColor::default().render(
                &mut render_pass,
                TargetData {
                    target_size,
                    target_format,
                },
                RenderData {
                    transform: Matrix4::identity(),
                    vertices: &vertices,
                    indices: &indices,
                },
                ctx.device,
            );
        }
        ctx.queue.submit(Some(encoder.finish()));

        node.with_texture(region, [w, h], Matrix4::identity())
    })
}

impl Widget for ColorRect {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.geometry(),
            RectColor(self.color),
            LayoutDispatch::of::<RectGeometry>(),
            make_render_item(self.w, self.h, self.color),
        )
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        let geometry = self.geometry();
        let mut changed = false;
        if let Some(mut g) = entity.get_mut::<RectGeometry>() {
            changed |= g.set_if_neq(geometry);
        }
        if let Some(mut c) = entity.get_mut::<RectColor>() {
            changed |= c.set_if_neq(RectColor(self.color));
        }
        // Rebuild the cached render node only when a draw-relevant prop changed.
        if changed {
            let item = make_render_item(self.w, self.h, self.color);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}

impl Layout for RectGeometry {
    fn measure(&self, _ctx: &mut LayoutCtx, _me: Entity, c: Constraints) -> [f32; 2] {
        [
            self.w.clamp(c.min_width(), c.max_width()),
            self.h.clamp(c.min_height(), c.max_height()),
        ]
    }

    fn arrange(&self, _ctx: &mut LayoutCtx, _me: Entity, _size: [f32; 2]) {
        // Leaf: no children to arrange.
    }
}
