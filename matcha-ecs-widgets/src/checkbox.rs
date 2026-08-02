//! `Checkbox` — a clickable leaf widget rendering a bordered box, filled when
//! `checked`, emitting an Elm-style message on click.
//!
//! Fully declarative like every other widget here: the app passes the
//! current `checked` bool on every `view()` call (compared via `patch`'s
//! `set_if_neq`, same pattern as `Button`'s `.color()`) — there is no
//! internal widget-side toggle state. A text label ("Remember me") is
//! deliberately not part of this widget; compose one externally (e.g.
//! `Row > (Checkbox, Text)`), matching HTML where `<input type=checkbox>` and
//! its `<label>` are siblings, not one element.
//!
//! (A formerly-documented "known issue" here — intermittent corruption of
//! unrelated widgets while this widget rebuilt per toggle — was root-caused
//! and fixed on 2026-07-10: it was never atlas churn, but nondeterministic
//! instance ordering in `renderer`'s culling compute shader. See
//! `renderer/src/core_renderer/renderer_cull.wgsl` and CLAUDE.md.)

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, world::EntityWorldMut,
};
use matcha_window::window::CursorIcon;
use nalgebra::{Matrix4, Vector3};

use matcha_ecs::{
    components::{
        focus::FocusPolicy,
        input::{Cursor, Message, OnClick, Pickable},
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    layout::LayoutDispatch,
    view::Widget,
};

use crate::box_style::{box_node, BoxStyle};
use crate::sizing::RectGeometry;
use crate::shape::ShapeCtx;
use crate::sizing::Sizing;

/// Draw-relevant checkbox state, tracked so `patch` can detect changes and
/// rebuild the cached render item.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
struct CheckboxState {
    checked: bool,
    border_color: [f32; 4],
    fill_color: [f32; 4],
    border_width: f32,
    radius: f32,
}

/// A `size`×`size` checkbox: an outer bordered box, filled with `fill_color`
/// (inset by `border_width`) when `checked`.
pub struct Checkbox<Msg: Message> {
    key: Key,
    sizing: Sizing,
    checked: bool,
    size: f32,
    border_color: [f32; 4],
    fill_color: [f32; 4],
    border_width: f32,
    radius: f32,
    cursor: CursorIcon,
    msg: Option<Msg>,
}

impl<Msg: Message> Checkbox<Msg> {
    pub fn new(checked: bool) -> Self {
        Self {
            key: Key::Auto,
            sizing: Sizing::default(),
            checked,
            size: 20.0,
            border_color: [0.6, 0.6, 0.65, 1.0],
            fill_color: [0.25, 0.5, 0.9, 1.0],
            border_width: 2.0,
            radius: 0.0,
            cursor: CursorIcon::Pointer,
            msg: None,
        }
    }

    /// The message emitted when this checkbox is clicked.
    pub fn on(mut self, msg: Msg) -> Self {
        self.msg = Some(msg);
        self
    }

    /// Override the default 20px size.
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Override the default border colour (components in `0.0..=1.0`).
    pub fn border_color(mut self, color: [f32; 4]) -> Self {
        self.border_color = color;
        self
    }

    /// Override the default fill colour (components in `0.0..=1.0`).
    pub fn fill_color(mut self, color: [f32; 4]) -> Self {
        self.fill_color = color;
        self
    }

    /// Override the default 2px border width.
    pub fn border_width(mut self, width: f32) -> Self {
        self.border_width = width;
        self
    }

    /// Round the box's corners; half the size makes it a radio button.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }


    /// What the pointer looks like over this widget (CSS `cursor`).
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
            w: self.size,
            h: self.size,
        }
    }

    fn state(&self) -> CheckboxState {
        CheckboxState {
            checked: self.checked,
            border_color: self.border_color,
            fill_color: self.fill_color,
            border_width: self.border_width,
            radius: self.radius,
        }
    }
}

/// Build a `RenderItem` compositing the outer border box with an inset fill
/// box on top, only when `checked`. Drawn at the layout-allocated size
/// (`ctx.size`) — normally the declared square, but a stretching parent
/// layout may allocate a non-square box, and paint must track layout.
fn checkbox_render_item(shape: ShapeCtx, state: CheckboxState) -> RenderItem {
    let outline = BoxStyle::default()
        .border(state.border_width, state.border_color)
        .radius(state.radius);
    // The tick is a second, inset box rather than part of the outline's style:
    // it comes and goes while the outline never moves, and the two therefore
    // want separate cached shapes.
    let inset = state.border_width;
    let tick = BoxStyle::fill(state.fill_color).radius((state.radius - inset).max(0.0));

    RenderItem::new(move |ctx: &RenderCtx| {
        let [w, h] = ctx.size;
        let mut node = box_node(ctx, &shape, [w, h], &outline);
        if state.checked {
            let fill_size = [(w - inset * 2.0).max(0.0), (h - inset * 2.0).max(0.0)];
            let fill_node = box_node(ctx, &shape, fill_size, &tick);
            let transform = Matrix4::new_translation(&Vector3::new(inset, inset, 0.0));
            node.push_child(fill_node, transform);
        }
        node
    })
}

impl<Msg: Message> Widget for Checkbox<Msg> {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.geometry(),
            self.state(),
            OnClick(self.msg.clone()),
            self.sizing,
            LayoutDispatch::of::<RectGeometry>(),
            Pickable,
            FocusPolicy::Normal,
            Cursor(self.cursor),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        // Built here rather than in `bundle()`: it needs the `ShapeCtx`
        // resource, which only world access can reach.
        let shape = ShapeCtx::get(entity);
        entity.insert(checkbox_render_item(shape, self.state()));
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        self.sync_sizing(entity);
        let mut changed = false;
        if let Some(mut g) = entity.get_mut::<RectGeometry>() {
            changed |= g.set_if_neq(self.geometry());
        }
        if let Some(mut s) = entity.get_mut::<CheckboxState>() {
            changed |= s.set_if_neq(self.state());
        }
        if let Some(mut on_click) = entity.get_mut::<OnClick<Msg>>() {
            on_click.set_if_neq(OnClick(self.msg.clone()));
        }

        if changed {
            let shape = ShapeCtx::get(entity);
            let item = checkbox_render_item(shape, self.state());
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}
