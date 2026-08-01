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
    components::{
        render::{RenderCtx, RenderItem, RenderOpacity},
        view::{Key, ManualDespawn},
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
    view::Widget,
};

use crate::sizing::Sizing;
use crate::animation::{Easing, ExitFade, OpacityTween};

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
    sizing: Sizing,
    w: f32,
    h: f32,
    color: [f32; 4],
    /// If set, this rect fades in from transparent when first spawned (baked
    /// directly into `bundle()`'s initial `RenderOpacity` plus an
    /// `OpacityTween` that closes the gap; there is no persisted "enter
    /// transition" component).
    enter_fade: Option<(Duration, Easing)>,
    /// If set, this rect fades out instead of vanishing immediately when
    /// pruned from the view (`after_spawn` attaches `ExitFade` +
    /// `ManualDespawn`; `crate::animation`'s systems do the rest).
    exit_fade: Option<(Duration, Easing)>,
}

impl ColorRect {
    /// Create a `w`×`h` rectangle (white by default).
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            key: Key::Auto,
            sizing: Sizing::default(),
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

    crate::sizing_builders!();

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

/// Build a `RenderItem` around a single [`solid_rect_node`] call, drawn at the
/// layout-allocated size (`ctx.size`) — not the widget's declared size, which
/// a parent layout (e.g. `AlignItems::Stretch`) may have overridden.
pub(crate) fn solid_rect_render_item(color: [f32; 4]) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| solid_rect_node(ctx, ctx.size[0], ctx.size[1], color))
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
            self.sizing,
            LayoutDispatch::of::<RectGeometry>(),
            solid_rect_render_item(self.color),
            RenderOpacity(initial_opacity),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        // These are only sometimes wanted, so they're attached here rather than
        // as part of `bundle()` (`Option<T>` isn't itself a `Bundle`).
        if let Some((duration, easing)) = self.enter_fade {
            entity.insert(OpacityTween {
                from: 0.0,
                to: 1.0,
                start: web_time::Instant::now(),
                duration,
                easing,
            });
        }
        if let Some((duration, easing)) = self.exit_fade {
            // `ManualDespawn` is what actually defers the despawn; `ExitFade`
            // only says what to do with the reprieve. Attaching it *only* when
            // an exit fade is configured is what keeps every other widget on
            // the despawn-immediately path (and keeps this one from leaking:
            // `crate::animation`'s systems are what eventually despawn it).
            entity.insert((ManualDespawn::new(), ExitFade { duration, easing }));
        }
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        self.sync_sizing(entity);
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
            let item = solid_rect_render_item(self.color);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }

        // Revival: this entity was mid-exit-fade and has just been re-declared
        // by the view. Reverse back toward full visibility, reusing the exit
        // fade's own duration/easing. The reconciler clears the pruned flag
        // right after `patch` returns, which is what stops the animation
        // systems from despawning it.
        if entity.get::<ManualDespawn>().is_some_and(|m| m.is_pruned()) {
            if let Some(exit) = entity.get::<ExitFade>().copied() {
                let current = entity.get::<RenderOpacity>().copied().unwrap_or_default();
                entity.insert(OpacityTween {
                    from: current.0,
                    to: 1.0,
                    start: web_time::Instant::now(),
                    duration: exit.duration,
                    easing: exit.easing,
                });
            }
        }
    }
}

impl Layout for RectGeometry {
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, c: Constraints) -> Measured {
        // Shared by every widget that is a plain box: `ColorRect`, `Button`,
        // `Checkbox`, `Image`. The declared `w`/`h` are the content size; a
        // `Sizing` on the entity overrides or bounds it.
        let sizing = Sizing::of(ctx, me);
        let inner = sizing.content_constraints(c);
        let content = [
            self.w.clamp(inner.min_width(), inner.max_width()),
            self.h.clamp(inner.min_height(), inner.max_height()),
        ];
        sizing.measured(c, Measured::exact(content))
    }

    fn arrange(&self, _ctx: &mut LayoutCtx, _me: Entity, _size: [f32; 2]) {
        // Leaf: no children to arrange.
    }
}
