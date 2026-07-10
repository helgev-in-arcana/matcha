//! `ColorRect` — a fixed-size solid colour rectangle, positioned by whatever
//! layout it is nested under.
//!
//! It is a layout *leaf*: `measure` returns its own `w`×`h` (clamped to the
//! incoming constraints) and ignores children; `arrange` does nothing further
//! since it has none. Its [`RenderItem`] rasterises a single-colour quad into
//! the texture atlas.

use std::time::Duration;

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
    animation::{Animated, Easing, ExitTransition, Opacity, Target, ToBeDespawn, Tween},
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
    /// If set, this rect fades in from transparent when first spawned
    /// (`ECS_ARCHITECTURE.md` §9, M7 — baked directly into `bundle()`'s
    /// initial `Animated<Opacity>`/`Tween<Opacity>` mismatch; there is no
    /// persisted "enter transition" component).
    enter_fade: Option<(Duration, Easing)>,
    /// If set, this rect fades out instead of vanishing immediately when
    /// pruned from the view (`view.rs`'s `begin_or_continue_exit` reads
    /// the resulting `ExitTransition<Opacity>`).
    exit_fade: Option<(Duration, Easing)>,
}

impl ColorRect {
    /// Create a `w`×`h` rectangle (white by default).
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            key: Key::Auto,
            w,
            h,
            color: [1.0, 1.0, 1.0, 1.0],
            enter_fade: None,
            exit_fade: None,
        }
    }

    /// Set the RGBA fill colour (components in `0.0..=1.0`).
    pub fn color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    /// Fade in from transparent over `duration` when first spawned.
    pub fn enter_fade(mut self, duration: Duration, easing: Easing) -> Self {
        self.enter_fade = Some((duration, easing));
        self
    }

    /// Fade out over `duration` instead of vanishing immediately when pruned
    /// from the view.
    pub fn exit_fade(mut self, duration: Duration, easing: Easing) -> Self {
        self.exit_fade = Some((duration, easing));
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

/// Rasterise a `w`×`h` solid `color` quad into the colour atlas and return a
/// textured `RenderNode` (positioned later by the entity's `GlobalTransform`).
/// Shared by every widget that needs to composite one or more flat-colour
/// rects into a larger `RenderItem` (`ColorRect`, `Button`'s box, `Checkbox`'s
/// border/fill, `Panel`'s border/background) without duplicating the
/// render-pass/`VertexColor` boilerplate.
pub(crate) fn solid_rect_node(ctx: &RenderCtx, w: f32, h: f32, color: [f32; 4]) -> RenderNode {
    let node = RenderNode::new();
    if w <= 0.0 || h <= 0.0 {
        return node;
    }

    let size_px = [w.ceil() as u32, h.ceil() as u32];
    let region = match ctx.texture_atlas.allocate(ctx.device, ctx.queue, size_px) {
        Ok(region) => region,
        Err(e) => {
            log::error!("solid_rect_node atlas allocation failed: {e}");
            return node;
        }
    };

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("solid_rect_node Render Encoder"),
        });

    // Colour is baked into the atlas texture at build time (no per-instance
    // alpha uniform at draw time), so a live opacity animation (M7) must
    // multiply it in here and rely on `RenderItem::invalidate()` to force
    // a rebuild on every frame the opacity actually changes.
    let color = [color[0], color[1], color[2], color[3] * ctx.opacity];

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
}

/// Build a `RenderItem` around a single [`solid_rect_node`] call.
pub(crate) fn solid_rect_render_item(w: f32, h: f32, color: [f32; 4]) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| solid_rect_node(ctx, w, h, color))
}

impl Widget for ColorRect {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        let initial_opacity = if self.enter_fade.is_some() { 0.0 } else { 1.0 };

        (
            self.geometry(),
            RectColor(self.color),
            LayoutDispatch::of::<RectGeometry>(),
            solid_rect_render_item(self.w, self.h, self.color),
            Target(Opacity(1.0)),
            Animated(Opacity(initial_opacity)),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        // `Tween<Opacity>`/`ExitTransition<Opacity>` are SparseSet and only
        // sometimes wanted, so they're attached here rather than as part of
        // `bundle()` (`Option<T>` isn't itself a `Bundle`).
        if let Some((duration, easing)) = self.enter_fade {
            entity.insert(Tween::<Opacity> {
                from: Opacity(0.0),
                start: web_time::Instant::now(),
                duration,
                easing,
            });
        }
        if let Some((duration, easing)) = self.exit_fade {
            entity.insert(ExitTransition::<Opacity> {
                to: Opacity(0.0),
                duration,
                easing,
            });
        }
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
            let item = solid_rect_render_item(self.w, self.h, self.color);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }

        // Revival (M7): this entity was mid-exit-fade and has just been
        // re-declared by the view. Reverse back toward full visibility,
        // reusing the exit fade's own duration/easing. `view.rs` removes
        // `ToBeDespawn` right after `patch` returns.
        if entity.get::<ToBeDespawn>().is_some() {
            if let Some(exit) = entity.get::<ExitTransition<Opacity>>().copied() {
                let current = entity
                    .get::<Animated<Opacity>>()
                    .copied()
                    .unwrap_or(Animated(Opacity(1.0)));
                entity.insert((
                    Target(Opacity(1.0)),
                    Tween::<Opacity> {
                        from: current.0,
                        start: web_time::Instant::now(),
                        duration: exit.duration,
                        easing: exit.easing,
                    },
                ));
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
