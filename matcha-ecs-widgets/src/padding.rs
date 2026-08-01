//! `Padding` — insets a single child by fixed margins on each side.

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    world::EntityWorldMut,
};
use nalgebra::Matrix4;

use matcha_ecs::{
    components::{layout::GlobalTransform, view::Key},
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
    view::Widget,
};

use crate::sizing::Sizing;

/// Fixed inset applied to a [`Padding`]'s single child.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct PaddingLayout {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl PaddingLayout {
    fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// Insets its single child (declared via `Scope::node`) by fixed margins.
pub struct Padding {
    key: Key,
    sizing: Sizing,
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
}

impl Padding {
    pub fn new() -> Self {
        Self {
            key: Key::Auto,
            sizing: Sizing::default(),
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }

    /// The same inset on all four sides.
    pub fn all(v: f32) -> Self {
        Self {
            key: Key::Auto,
            sizing: Sizing::default(),
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub fn top(mut self, v: f32) -> Self {
        self.top = v;
        self
    }

    pub fn right(mut self, v: f32) -> Self {
        self.right = v;
        self
    }

    pub fn bottom(mut self, v: f32) -> Self {
        self.bottom = v;
        self
    }

    pub fn left(mut self, v: f32) -> Self {
        self.left = v;
        self
    }

    crate::sizing_builders!();

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn layout(&self) -> PaddingLayout {
        PaddingLayout {
            top: self.top,
            right: self.right,
            bottom: self.bottom,
            left: self.left,
        }
    }
}

impl Default for Padding {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Padding {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (self.layout(), LayoutDispatch::of::<PaddingLayout>())
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        self.sync_sizing(entity);
        if let Some(mut layout) = entity.get_mut::<PaddingLayout>() {
            layout.set_if_neq(self.layout());
        }
    }
}

impl Layout for PaddingLayout {
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, c: Constraints) -> Measured {
        let h = self.horizontal();
        let v = self.vertical();
        let sizing = Sizing::of(ctx, me);
        let own = sizing.content_constraints(c);
        let inner_c = Constraints::new(
            [0.0, (own.max_width() - h).max(0.0)],
            [0.0, (own.max_height() - v).max(0.0)],
        );

        let content = match ctx.children(me).first() {
            Some(&child) => {
                let s = ctx.measure_child_size(child, inner_c);
                [s[0] + h, s[1] + v]
            }
            None => [h, v],
        };
        sizing.measured(c, Measured::exact(content))
    }

    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, size: [f32; 2]) {
        let my_affine = ctx
            .world()
            .get::<GlobalTransform>(me)
            .map(|t| t.affine)
            .unwrap_or_else(Matrix4::identity);

        if let Some(&child) = ctx.children(me).first() {
            let h = self.horizontal();
            let v = self.vertical();
            let inner_size = [(size[0] - h).max(0.0), (size[1] - v).max(0.0)];
            let child_c = Constraints::from_max_size(inner_size);
            let child_size = ctx.measure_child_size(child, child_c);
            ctx.arrange_child(child, [self.left, self.top], my_affine, child_size);
        }
    }
}
