//! Container / Column / Row — layout containers (children via `Scope::node`).

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    world::EntityWorldMut,
};
use nalgebra::Matrix4;

use matcha_ecs::{
    components::{layout::GlobalTransform, view::Key},
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch},
    view::Widget,
};

/// Which layout a container applies. Constant per widget type; carried as a
/// data component so systems can query it.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutKind {
    Container,
    Column,
    Row,
}

/// Spacing between children (used by Column/Row).
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct Gap(pub f32);

/// A neutral container with a single child area.
#[derive(Default)]
pub struct Container {
    key: Key,
}

impl Container {
    pub fn new() -> Self {
        Self { key: Key::Auto }
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }
}

impl Widget for Container {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (LayoutKind::Container, LayoutDispatch::of::<LayoutKind>())
    }

    fn patch(&self, _entity: &mut EntityWorldMut) {
        // LayoutKind is constant for this type; nothing to patch.
    }
}

macro_rules! stack_widget {
    ($name:ident, $kind:expr) => {
        /// A stacking container.
        pub struct $name {
            key: Key,
            gap: f32,
        }

        impl $name {
            pub fn new() -> Self {
                Self {
                    key: Key::Auto,
                    gap: 0.0,
                }
            }

            pub fn gap(mut self, gap: f32) -> Self {
                self.gap = gap;
                self
            }

            pub fn key(mut self, key: impl Into<Key>) -> Self {
                self.key = key.into();
                self
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Widget for $name {
            fn key(&self) -> Key {
                self.key
            }

            fn bundle(&self) -> impl Bundle {
                ($kind, Gap(self.gap), LayoutDispatch::of::<LayoutKind>())
            }

            fn patch(&self, entity: &mut EntityWorldMut) {
                if let Some(mut gap) = entity.get_mut::<Gap>() {
                    gap.set_if_neq(Gap(self.gap));
                }
            }
        }
    };
}

stack_widget!(Column, LayoutKind::Column);
stack_widget!(Row, LayoutKind::Row);

impl LayoutKind {
    fn gap(&self, ctx: &LayoutCtx, me: Entity) -> f32 {
        ctx.world().get::<Gap>(me).map(|g| g.0).unwrap_or(0.0)
    }

    fn my_affine(ctx: &LayoutCtx, me: Entity) -> Matrix4<f32> {
        ctx.world()
            .get::<GlobalTransform>(me)
            .map(|t| t.affine)
            .unwrap_or_else(Matrix4::identity)
    }
}

impl Layout for LayoutKind {
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, c: Constraints) -> [f32; 2] {
        let children = ctx.children(me);

        match self {
            LayoutKind::Container => match children.first() {
                Some(&child) => ctx.measure_child(child, c),
                None => [0.0, 0.0],
            },
            LayoutKind::Column => {
                let gap = self.gap(ctx, me);
                let child_c = Constraints::new([0.0, c.max_width()], [0.0, c.max_height()]);
                let sizes: Vec<[f32; 2]> = children
                    .iter()
                    .map(|&e| ctx.measure_child(e, child_c))
                    .collect();
                let total_h: f32 = sizes.iter().map(|s| s[1]).sum::<f32>()
                    + gap * sizes.len().saturating_sub(1) as f32;
                let max_w: f32 = sizes.iter().map(|s| s[0]).fold(0.0, f32::max);
                [max_w.min(c.max_width()), total_h.min(c.max_height())]
            }
            LayoutKind::Row => {
                let gap = self.gap(ctx, me);
                let child_c = Constraints::new([0.0, c.max_width()], [0.0, c.max_height()]);
                let sizes: Vec<[f32; 2]> = children
                    .iter()
                    .map(|&e| ctx.measure_child(e, child_c))
                    .collect();
                let total_w: f32 = sizes.iter().map(|s| s[0]).sum::<f32>()
                    + gap * sizes.len().saturating_sub(1) as f32;
                let max_h: f32 = sizes.iter().map(|s| s[1]).fold(0.0, f32::max);
                [total_w.min(c.max_width()), max_h.min(c.max_height())]
            }
        }
    }

    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, size: [f32; 2]) {
        let children = ctx.children(me);
        let my_affine = Self::my_affine(ctx, me);

        match self {
            LayoutKind::Container => {
                if let Some(&child) = children.first() {
                    let child_c = Constraints::from_max_size(size);
                    let child_size = ctx.measure_child(child, child_c);
                    ctx.arrange_child(child, [0.0, 0.0], my_affine, child_size);
                }
            }
            LayoutKind::Column => {
                let gap = self.gap(ctx, me);
                let child_c = Constraints::new([0.0, size[0]], [0.0, size[1]]);
                let mut y = 0.0f32;
                for child in children {
                    let child_size = ctx.measure_child(child, child_c);
                    ctx.arrange_child(child, [0.0, y], my_affine, child_size);
                    y += child_size[1] + gap;
                }
            }
            LayoutKind::Row => {
                let gap = self.gap(ctx, me);
                let child_c = Constraints::new([0.0, size[0]], [0.0, size[1]]);
                let mut x = 0.0f32;
                for child in children {
                    let child_size = ctx.measure_child(child, child_c);
                    ctx.arrange_child(child, [x, 0.0], my_affine, child_size);
                    x += child_size[0] + gap;
                }
            }
        }
    }
}
