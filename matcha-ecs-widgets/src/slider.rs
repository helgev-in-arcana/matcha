//! `Slider` — drag a value along a track.
//!
//! Declarative like every other widget here: the app passes the current value
//! on each `view()` call and receives a message on change. The widget keeps no
//! value of its own, so there is no "the slider and the model disagree" state
//! to reconcile.
//!
//! # What it reuses
//!
//! Almost everything. The drag is `PointerDispatch` plus the pointer capture a
//! press establishes (`matcha_ecs::input::PointerCapture`), which is what makes
//! a drag keep working once the cursor leaves the track — the same mechanism
//! that fixed the scrollbar thumb. Painting is three [`box_node`] calls, so a
//! rounded track and a round knob cost nothing this crate did not already have.
//! Keyboard support is `KeyDispatch` on a `FocusPolicy::Normal` entity, so
//! arrows work as soon as it is tabbed to.
//!
//! # Not supported
//!
//! Vertical orientation (nothing wants it yet; it would be an `Axis` field and
//! a swap in three places), tick marks, and a two-handle range. The value is a
//! plain `f32` — a discrete slider is `step`, which *is* here.

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component,
    world::EntityWorldMut,
};
use matcha_window::{
    event::device_event::{Key as LogicalKey, KeyInput, NamedKey},
    window::CursorIcon,
};
use nalgebra::{Matrix4, Vector3};
use renderer::RenderNode;

use matcha_ecs::{
    components::{
        focus::FocusPolicy,
        input::{
            Cursor, KeyDispatch, Message, Pickable, PointerDispatch, PointerInput, PointerPhase,
        },
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    input::emit_message,
    layout::LayoutDispatch,
    view::Widget,
};

use crate::{
    box_style::{box_node, BoxStyle},
    shape::ShapeCtx,
    sizing::{RectGeometry, Sizing},
};

/// How the value maps onto the track.
///
/// A separate component from the visual style because the pointer handler needs
/// exactly this and nothing else — it runs behind a non-generic fn pointer and
/// reads its state off the entity.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct SliderRange {
    pub min: f32,
    pub max: f32,
    /// Quantisation. `0.0` means continuous.
    pub step: f32,
    pub value: f32,
}

impl SliderRange {
    /// Where the knob sits, `0.0..=1.0`. A degenerate range pins it at the
    /// start rather than dividing by zero.
    pub fn fraction(&self) -> f32 {
        let span = self.max - self.min;
        if span.abs() < f32::EPSILON {
            return 0.0;
        }
        ((self.value - self.min) / span).clamp(0.0, 1.0)
    }

    /// The value a `0.0..=1.0` position along the track represents, clamped and
    /// snapped to `step`.
    pub fn value_at(&self, fraction: f32) -> f32 {
        let raw = self.min + (self.max - self.min) * fraction.clamp(0.0, 1.0);
        let snapped = if self.step > 0.0 {
            self.min + ((raw - self.min) / self.step).round() * self.step
        } else {
            raw
        };
        snapped.clamp(self.min.min(self.max), self.max.max(self.min))
    }

    /// One arrow-key press worth of movement. A continuous slider moves by a
    /// hundredth of its span, which is the granularity a keyboard user can
    /// actually aim with.
    fn nudge(&self) -> f32 {
        if self.step > 0.0 {
            self.step
        } else {
            (self.max - self.min).abs() / 100.0
        }
    }
}

/// Drawing parameters, kept apart from [`SliderRange`] so `patch` can tell an
/// appearance change from a value change.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
struct SliderStyle {
    w: f32,
    h: f32,
    track_height: f32,
    knob_radius: f32,
    track_color: [f32; 4],
    fill_color: [f32; 4],
    knob_color: [f32; 4],
    focus_ring_color: [f32; 4],
}

/// The message a slider emits, instantiated per `Msg` so the non-generic
/// handlers can build one.
#[derive(Component, Clone)]
struct OnSliderChange<Msg: Message>(Option<fn(f32) -> Msg>);

/// A horizontal value slider.
pub struct Slider<Msg: Message> {
    key: Key,
    sizing: Sizing,
    range: SliderRange,
    style: SliderStyle,
    on_change: Option<fn(f32) -> Msg>,
    cursor: CursorIcon,
}

impl<Msg: Message> Slider<Msg> {
    /// A slider over `min..=max`, currently at `value`.
    pub fn new(value: f32, min: f32, max: f32) -> Self {
        Self {
            key: Key::Auto,
            sizing: Sizing::default(),
            range: SliderRange {
                min,
                max,
                step: 0.0,
                value,
            },
            style: SliderStyle {
                w: 200.0,
                h: 24.0,
                track_height: 6.0,
                knob_radius: 9.0,
                track_color: [0.25, 0.25, 0.3, 1.0],
                fill_color: [0.35, 0.6, 0.95, 1.0],
                knob_color: [0.92, 0.92, 0.95, 1.0],
                focus_ring_color: [0.45, 0.7, 1.0, 1.0],
            },
            on_change: None,
            cursor: CursorIcon::Pointer,
        }
    }

    /// Called with the new value whenever the user moves the slider.
    pub fn on_change(mut self, f: fn(f32) -> Msg) -> Self {
        self.on_change = Some(f);
        self
    }

    /// Quantise the value. `0.0` (the default) is continuous.
    pub fn step(mut self, step: f32) -> Self {
        self.range.step = step;
        self
    }

    /// Override the default 200x24 box the slider occupies.
    pub fn size(mut self, w: f32, h: f32) -> Self {
        self.style.w = w;
        self.style.h = h;
        self
    }

    /// Thickness of the track bar (default 6px).
    pub fn track_height(mut self, height: f32) -> Self {
        self.style.track_height = height;
        self
    }

    /// Radius of the knob (default 9px).
    pub fn knob_radius(mut self, radius: f32) -> Self {
        self.style.knob_radius = radius;
        self
    }

    /// Colour of the track behind the knob, and of the filled part before it.
    pub fn colors(mut self, track: [f32; 4], fill: [f32; 4], knob: [f32; 4]) -> Self {
        self.style.track_color = track;
        self.style.fill_color = fill;
        self.style.knob_color = knob;
        self
    }

    pub fn cursor(mut self, cursor: CursorIcon) -> Self {
        self.cursor = cursor;
        self
    }

    crate::sizing_builders!();

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn geometry(&self) -> RectGeometry {
        RectGeometry {
            w: self.style.w,
            h: self.style.h,
        }
    }
}

/// The horizontal span the knob's *centre* travels, given the drawn width.
///
/// Inset by the knob radius at each end so the knob stays inside the box at
/// both extremes — the same reason a scrollbar thumb's travel is shorter than
/// its track.
fn travel(width: f32, knob_radius: f32) -> (f32, f32) {
    let start = knob_radius;
    let span = (width - knob_radius * 2.0).max(0.0);
    (start, span)
}

/// Turn a local x into a `0.0..=1.0` position along the track.
fn fraction_at(local_x: f32, width: f32, knob_radius: f32) -> f32 {
    let (start, span) = travel(width, knob_radius);
    if span <= 0.0 {
        return 0.0;
    }
    ((local_x - start) / span).clamp(0.0, 1.0)
}

fn slider_render_item(shape: ShapeCtx, style: SliderStyle, range: SliderRange) -> RenderItem {
    let track = BoxStyle::fill(style.track_color).radius(style.track_height / 2.0);
    let fill = BoxStyle::fill(style.fill_color).radius(style.track_height / 2.0);
    let knob = BoxStyle::fill(style.knob_color).radius(style.knob_radius);
    let ring = BoxStyle::fill(style.focus_ring_color).radius(style.knob_radius + 2.0);

    RenderItem::new(move |ctx: &RenderCtx| {
        let [w, h] = ctx.size;
        let mut node = RenderNode::new();

        let track_y = ((h - style.track_height) / 2.0).max(0.0);
        let (start, span) = travel(w, style.knob_radius);
        let knob_x = start + span * range.fraction();

        node.push_child(
            box_node(ctx, &shape, [w, style.track_height], &track),
            Matrix4::new_translation(&Vector3::new(0.0, track_y, 0.0)),
        );
        node.push_child(
            box_node(ctx, &shape, [knob_x, style.track_height], &fill),
            Matrix4::new_translation(&Vector3::new(0.0, track_y, 0.0)),
        );

        // The focus ring goes under the knob so the knob stays crisp.
        let d = style.knob_radius * 2.0;
        if ctx.focused {
            let rd = d + 4.0;
            node.push_child(
                box_node(ctx, &shape, [rd, rd], &ring),
                Matrix4::new_translation(&Vector3::new(
                    knob_x - rd / 2.0,
                    (h - rd) / 2.0,
                    0.0,
                )),
            );
        }
        node.push_child(
            box_node(ctx, &shape, [d, d], &knob),
            Matrix4::new_translation(&Vector3::new(knob_x - style.knob_radius, (h - d) / 2.0, 0.0)),
        );
        node
    })
}

/// Report a new value, if it is actually new.
///
/// Deliberately does **not** write it back into `SliderRange`: the value is the
/// app's, and it comes back on the next `view()` pass. Writing it here would
/// give the widget a second, competing copy — the thing every other widget in
/// this crate avoids.
fn report<Msg: Message>(entity: &mut EntityWorldMut, value: f32) -> bool {
    let range = entity.get::<SliderRange>().copied();
    let Some(range) = range else { return false };
    if (value - range.value).abs() < f32::EPSILON {
        return false;
    }
    let Some(f) = entity.get::<OnSliderChange<Msg>>().and_then(|c| c.0) else {
        return false;
    };
    let msg = f(value);
    entity.world_scope(|world| emit_message(world, msg));
    true
}

/// A press jumps the knob to the pointer and a drag keeps it there.
///
/// Both phases do the same thing, which is what "grab anywhere on the track"
/// means. The drag arrives even once the cursor has left the widget, because
/// consuming the press captured the pointer.
fn on_pointer<Msg: Message>(entity: &mut EntityWorldMut, input: &PointerInput) -> bool {
    if !matches!(
        input.phase,
        PointerPhase::Press { .. } | PointerPhase::Drag
    ) {
        return false;
    }
    let (Some(range), Some(style)) = (
        entity.get::<SliderRange>().copied(),
        entity.get::<SliderStyle>().copied(),
    ) else {
        return false;
    };
    // The *drawn* width, which layout may have stretched past the declared one.
    let width = entity
        .get::<matcha_ecs::components::layout::LayoutOutput>()
        .map(|l| l.size[0])
        .unwrap_or(style.w);

    let fraction = fraction_at(input.local_pos[0], width, style.knob_radius);
    report::<Msg>(entity, range.value_at(fraction));
    // Consumed either way: the press must capture the pointer even when it
    // landed exactly on the current value, or the drag that follows is lost.
    true
}

fn on_key<Msg: Message>(entity: &mut EntityWorldMut, input: &KeyInput) -> bool {
    let Some(range) = entity.get::<SliderRange>().copied() else {
        return false;
    };
    let LogicalKey::Named(named) = input.logical_key() else {
        return false;
    };
    let target = match named {
        NamedKey::ArrowLeft | NamedKey::ArrowDown => range.value - range.nudge(),
        NamedKey::ArrowRight | NamedKey::ArrowUp => range.value + range.nudge(),
        NamedKey::Home => range.min,
        NamedKey::End => range.max,
        _ => return false,
    };
    // Back through `value_at` so a keyboard step lands on the same grid a drag
    // would, rather than drifting off it.
    let span = range.max - range.min;
    let fraction = if span.abs() < f32::EPSILON {
        0.0
    } else {
        (target - range.min) / span
    };
    report::<Msg>(entity, range.value_at(fraction));
    true
}

impl<Msg: Message> Widget for Slider<Msg> {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.range,
            self.style,
            OnSliderChange(self.on_change),
            self.sizing,
            LayoutDispatch::of::<RectGeometry>(),
            self.geometry(),
            Pickable,
            FocusPolicy::Normal,
            Cursor(self.cursor),
            PointerDispatch::new(on_pointer::<Msg>),
            KeyDispatch::new(on_key::<Msg>),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        let shape = ShapeCtx::get(entity);
        entity.insert(slider_render_item(shape, self.style, self.range));
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        self.sync_sizing(entity);
        let mut changed = false;
        if let Some(mut r) = entity.get_mut::<SliderRange>() {
            changed |= r.set_if_neq(self.range);
        }
        if let Some(mut s) = entity.get_mut::<SliderStyle>() {
            changed |= s.set_if_neq(self.style);
        }
        if let Some(mut g) = entity.get_mut::<RectGeometry>() {
            changed |= g.set_if_neq(self.geometry());
        }
        // Fn pointers are not meaningfully comparable, so this is assigned
        // rather than compared — same rule as `TextBox`'s callbacks.
        entity.insert(OnSliderChange(self.on_change));

        if changed {
            let shape = ShapeCtx::get(entity);
            let item = slider_render_item(shape, self.style, self.range);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}

impl Slider<()> {
    /// Exposed for tests: the pure value mapping, with no ECS involved.
    pub fn value_for(range: SliderRange, local_x: f32, width: f32, knob_radius: f32) -> f32 {
        range.value_at(fraction_at(local_x, width, knob_radius))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range() -> SliderRange {
        SliderRange {
            min: 0.0,
            max: 100.0,
            step: 0.0,
            value: 50.0,
        }
    }

    #[test]
    fn the_knob_sits_where_the_value_says() {
        assert_eq!(range().fraction(), 0.5);
        assert_eq!(SliderRange { value: 0.0, ..range() }.fraction(), 0.0);
        assert_eq!(SliderRange { value: 100.0, ..range() }.fraction(), 1.0);
    }

    #[test]
    fn a_value_outside_the_range_is_clamped_rather_than_extrapolated() {
        assert_eq!(SliderRange { value: -20.0, ..range() }.fraction(), 0.0);
        assert_eq!(SliderRange { value: 999.0, ..range() }.fraction(), 1.0);
    }

    #[test]
    fn a_degenerate_range_pins_the_knob_instead_of_dividing_by_zero() {
        let r = SliderRange {
            min: 5.0,
            max: 5.0,
            step: 0.0,
            value: 5.0,
        };
        assert_eq!(r.fraction(), 0.0);
        assert_eq!(r.value_at(0.7), 5.0);
    }

    #[test]
    fn step_snaps_to_the_grid() {
        let r = SliderRange { step: 25.0, ..range() };
        assert_eq!(r.value_at(0.5), 50.0);
        assert_eq!(r.value_at(0.44), 50.0, "0.44 -> 44 -> nearest 25 is 50");
        assert_eq!(r.value_at(0.0), 0.0);
        assert_eq!(r.value_at(1.0), 100.0);
    }

    #[test]
    fn the_ends_of_the_track_are_reachable_despite_the_knob_inset() {
        // The knob's centre travels a shorter span than the box is wide, so a
        // press at either extreme must still resolve to min and max.
        let (w, kr) = (200.0, 9.0);
        assert_eq!(fraction_at(0.0, w, kr), 0.0);
        assert_eq!(fraction_at(w, w, kr), 1.0);
        assert!((fraction_at(w / 2.0, w, kr) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_box_narrower_than_the_knob_does_not_produce_a_nonsense_fraction() {
        assert_eq!(fraction_at(3.0, 10.0, 9.0), 0.0);
    }

    #[test]
    fn a_continuous_slider_nudges_by_a_hundredth_of_its_span() {
        assert_eq!(range().nudge(), 1.0);
        assert_eq!(SliderRange { step: 25.0, ..range() }.nudge(), 25.0);
    }
}
