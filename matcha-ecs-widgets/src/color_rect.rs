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

use matcha_ecs::{
    components::{
        render::{RenderCtx, RenderItem, RenderOpacity},
        view::{Key, ManualDespawn},
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
    view::Widget,
};

use crate::animation::{Easing, ExitFade, OpacityTween};
use crate::box_style::{box_node, BoxStyle};
use crate::shape::ShapeCtx;
use crate::sizing::{RectGeometry, Sizing};

/// The RGBA fill colour of a [`ColorRect`], carried so `patch` can detect changes.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct RectColor(pub [f32; 4]);

/// A [`ColorRect`]'s corner radius, carried so `patch` can detect changes.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct RectRadius(pub f32);

/// A solid-colour rectangle of fixed size.
pub struct ColorRect {
    key: Key,
    sizing: Sizing,
    w: f32,
    h: f32,
    color: [f32; 4],
    radius: f32,
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
            radius: 0.0,
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

    /// Round this rect's corners (CSS `border-radius`). Unlike the other
    /// properties this costs a rasterised coverage bitmap per distinct
    /// (size, radius) — see `crate::box_style`.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    fn style(&self) -> BoxStyle {
        BoxStyle::fill(self.color).radius(self.radius)
    }

    fn geometry(&self) -> RectGeometry {
        RectGeometry {
            w: self.w,
            h: self.h,
        }
    }
}

/// Build a `RenderItem` drawing this rect at the layout-allocated size
/// (`ctx.size`) — not the widget's declared size, which a parent layout (e.g.
/// `AlignItems::Stretch`) may have overridden.
fn color_rect_render_item(shape: ShapeCtx, style: BoxStyle) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| box_node(ctx, &shape, ctx.size, &style))
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
            RectRadius(self.radius),
            self.sizing,
            LayoutDispatch::of::<RectGeometry>(),
            RenderOpacity(initial_opacity),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        // The render item is built here rather than in `bundle()` because it
        // needs the `ShapeCtx` resource, which only world access can reach.
        let shape = ShapeCtx::get(entity);
        entity.insert(color_rect_render_item(shape, self.style()));

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
        if let Some(mut r) = entity.get_mut::<RectRadius>() {
            changed |= r.set_if_neq(RectRadius(self.radius));
        }
        // Rebuild the cached render node only when a draw-relevant prop changed.
        if changed {
            let shape = ShapeCtx::get(entity);
            let item = color_rect_render_item(shape, self.style());
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

