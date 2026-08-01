//! Container / Column / Row — layout containers (children via `Scope::node`).

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    world::EntityWorldMut,
};
use nalgebra::Matrix4;

use matcha_ecs::{
    components::{
        layout::{GlobalTransform, Hidden},
        view::Key,
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
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

/// CSS `justify-content` subset: how leftover main-axis space (container size
/// minus the children's natural summed extent) is distributed. Never changes
/// a child's own size — there is no flex-grow/flex-basis here, only
/// repositioning. Default matches CSS's `flex-start`.
///
/// **Currently has no visible effect with this crate's widget set**: every
/// `Layout::arrange` call in this codebase (`layout_root`, `Container`,
/// `Padding`, `Panel`) always hands a child exactly the size its own
/// `measure()` returned for the same constraints — `Column`/`Row` size
/// themselves to content ("fit-content"), so main-axis leftover space is
/// always `0.0`. It starts having a visible effect once a widget exists that
/// can allocate a `Column`/`Row` *more* main-axis space than its children's
/// natural sum (e.g. a future "fill available space" sizing mode) — until
/// then this is forward-compatible scaffolding, exercised directly at the
/// unit level below rather than through an end-to-end layout test.
/// `AlignItems` does not have this limitation: a `Column`/`Row`'s
/// *cross*-axis size is the max of its children's natural cross sizes, so a
/// narrower sibling reachably differs from that today.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum JustifyContent {
    #[default]
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
}

/// CSS `align-items` subset: cross-axis placement of each child within the
/// container's cross-axis size. Default matches CSS's `stretch`.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AlignItems {
    #[default]
    Stretch,
    Start,
    End,
    Center,
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Justify(JustifyContent);
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Align(AlignItems);

/// A neutral container with a single child area.
pub struct Container {
    key: Key,
    visible: bool,
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Container {
    pub fn new() -> Self {
        Self {
            key: Key::Auto,
            visible: true,
        }
    }

    /// CSS `display: none` when `false`: this container and everything under it
    /// take no part in layout, drawing or picking.
    ///
    /// Absent, not merely invisible — a hidden container claims no space, so a
    /// surrounding `Column`'s `gap` closes up around it. Wrapping a subtree in
    /// a `Container` is how any part of a view is hidden; the marker covers
    /// descendants, so no other widget needs its own `.visible()`.
    ///
    /// State inside the subtree survives being hidden: the entities are still
    /// there, so a text box keeps its text and re-showing costs no rebuild.
    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    /// `Hidden` is a data-less marker, so it cannot sit in `bundle()`'s fixed
    /// return type conditionally — inserted and removed here instead, on spawn
    /// and on every patch, so toggling `.visible(..)` takes effect. Same shape
    /// as `Panel::sync_clip`.
    fn sync_visible(&self, entity: &mut EntityWorldMut) {
        if self.visible {
            entity.remove::<Hidden>();
        } else {
            entity.insert(Hidden);
        }
    }
}

impl Widget for Container {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (LayoutKind::Container, LayoutDispatch::of::<LayoutKind>())
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        self.sync_visible(entity);
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        // LayoutKind is constant for this type; only visibility can change.
        self.sync_visible(entity);
    }
}

macro_rules! stack_widget {
    ($name:ident, $kind:expr) => {
        /// A stacking container.
        pub struct $name {
            key: Key,
            gap: f32,
            justify: JustifyContent,
            align: AlignItems,
        }

        impl $name {
            pub fn new() -> Self {
                Self {
                    key: Key::Auto,
                    gap: 0.0,
                    justify: JustifyContent::default(),
                    align: AlignItems::default(),
                }
            }

            pub fn gap(mut self, gap: f32) -> Self {
                self.gap = gap;
                self
            }

            /// CSS `justify-content` (main-axis distribution of leftover space).
            pub fn justify_content(mut self, justify: JustifyContent) -> Self {
                self.justify = justify;
                self
            }

            /// CSS `align-items` (cross-axis placement/stretch of children).
            pub fn align_items(mut self, align: AlignItems) -> Self {
                self.align = align;
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
                (
                    $kind,
                    Gap(self.gap),
                    Justify(self.justify),
                    Align(self.align),
                    LayoutDispatch::of::<LayoutKind>(),
                )
            }

            fn patch(&self, entity: &mut EntityWorldMut) {
                if let Some(mut gap) = entity.get_mut::<Gap>() {
                    gap.set_if_neq(Gap(self.gap));
                }
                if let Some(mut justify) = entity.get_mut::<Justify>() {
                    justify.set_if_neq(Justify(self.justify));
                }
                if let Some(mut align) = entity.get_mut::<Align>() {
                    align.set_if_neq(Align(self.align));
                }
            }
        }
    };
}

stack_widget!(Column, LayoutKind::Column);
stack_widget!(Row, LayoutKind::Row);

/// Main-axis start offset and inter-child gap inflation for `justify`, given
/// `extra` leftover main-axis space (container size minus children's natural
/// summed extent, already clamped to `>= 0.0`) and `n` children. Shared by
/// `Column`(main axis = height) and `Row`(main axis = width).
fn justify_offsets(justify: JustifyContent, extra: f32, n: usize) -> (f32, f32) {
    match justify {
        JustifyContent::Start => (0.0, 0.0),
        JustifyContent::End => (extra, 0.0),
        JustifyContent::Center => (extra / 2.0, 0.0),
        JustifyContent::SpaceBetween => {
            let per_gap = if n > 1 { extra / (n - 1) as f32 } else { 0.0 };
            (0.0, per_gap)
        }
        JustifyContent::SpaceAround => {
            let per_child = if n > 0 { extra / n as f32 } else { 0.0 };
            (per_child / 2.0, per_child)
        }
    }
}

/// Cross-axis start offset for `align`, given the container's cross-axis
/// `container_cross` size and the child's own (possibly stretched)
/// `child_cross` size.
fn align_offset(align: AlignItems, container_cross: f32, child_cross: f32) -> f32 {
    match align {
        AlignItems::Stretch | AlignItems::Start => 0.0,
        AlignItems::End => (container_cross - child_cross).max(0.0),
        AlignItems::Center => ((container_cross - child_cross) / 2.0).max(0.0),
    }
}

impl LayoutKind {
    fn gap(&self, ctx: &LayoutCtx, me: Entity) -> f32 {
        ctx.world().get::<Gap>(me).map(|g| g.0).unwrap_or(0.0)
    }

    fn justify(&self, ctx: &LayoutCtx, me: Entity) -> JustifyContent {
        ctx.world().get::<Justify>(me).map(|j| j.0).unwrap_or_default()
    }

    fn align(&self, ctx: &LayoutCtx, me: Entity) -> AlignItems {
        ctx.world().get::<Align>(me).map(|a| a.0).unwrap_or_default()
    }

    fn my_affine(ctx: &LayoutCtx, me: Entity) -> Matrix4<f32> {
        ctx.world()
            .get::<GlobalTransform>(me)
            .map(|t| t.affine)
            .unwrap_or_else(Matrix4::identity)
    }
}

impl Layout for LayoutKind {
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, c: Constraints) -> Measured {
        let children = ctx.children(me);

        match self {
            LayoutKind::Container => match children.first() {
                Some(&child) => ctx.measure_child(child, c),
                None => Measured::exact([0.0, 0.0]),
            },
            LayoutKind::Column => {
                let gap = self.gap(ctx, me);
                let child_c = Constraints::new([0.0, c.max_width()], [0.0, c.max_height()]);
                let sizes: Vec<[f32; 2]> = children
                    .iter()
                    .map(|&e| ctx.measure_child_size(e, child_c))
                    .collect();
                let total_h: f32 = sizes.iter().map(|s| s[1]).sum::<f32>()
                    + gap * sizes.len().saturating_sub(1) as f32;
                let max_w: f32 = sizes.iter().map(|s| s[0]).fold(0.0, f32::max);
                Measured::exact([max_w.min(c.max_width()), total_h.min(c.max_height())])
            }
            LayoutKind::Row => {
                let gap = self.gap(ctx, me);
                let child_c = Constraints::new([0.0, c.max_width()], [0.0, c.max_height()]);
                let sizes: Vec<[f32; 2]> = children
                    .iter()
                    .map(|&e| ctx.measure_child_size(e, child_c))
                    .collect();
                let total_w: f32 = sizes.iter().map(|s| s[0]).sum::<f32>()
                    + gap * sizes.len().saturating_sub(1) as f32;
                let max_h: f32 = sizes.iter().map(|s| s[1]).fold(0.0, f32::max);
                Measured::exact([total_w.min(c.max_width()), max_h.min(c.max_height())])
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
                    let child_size = ctx.measure_child_size(child, child_c);
                    ctx.arrange_child(child, [0.0, 0.0], my_affine, child_size);
                }
            }
            LayoutKind::Column => {
                let gap = self.gap(ctx, me);
                let justify = self.justify(ctx, me);
                let align = self.align(ctx, me);
                let child_c = Constraints::new([0.0, size[0]], [0.0, size[1]]);
                let natural: Vec<[f32; 2]> = children.iter().map(|&e| ctx.measure_child_size(e, child_c)).collect();
                let natural_total: f32 = natural.iter().map(|s| s[1]).sum::<f32>()
                    + gap * natural.len().saturating_sub(1) as f32;
                let extra = (size[1] - natural_total).max(0.0);
                let (mut y, extra_gap) = justify_offsets(justify, extra, children.len());
                for (i, &child) in children.iter().enumerate() {
                    let child_size = if align == AlignItems::Stretch {
                        let stretched_c = Constraints::new([size[0], size[0]], [0.0, size[1]]);
                        ctx.measure_child_size(child, stretched_c)
                    } else {
                        natural[i]
                    };
                    let x = align_offset(align, size[0], child_size[0]);
                    ctx.arrange_child(child, [x, y], my_affine, child_size);
                    y += child_size[1] + gap + extra_gap;
                }
            }
            LayoutKind::Row => {
                let gap = self.gap(ctx, me);
                let justify = self.justify(ctx, me);
                let align = self.align(ctx, me);
                let child_c = Constraints::new([0.0, size[0]], [0.0, size[1]]);
                let natural: Vec<[f32; 2]> = children.iter().map(|&e| ctx.measure_child_size(e, child_c)).collect();
                let natural_total: f32 = natural.iter().map(|s| s[0]).sum::<f32>()
                    + gap * natural.len().saturating_sub(1) as f32;
                let extra = (size[0] - natural_total).max(0.0);
                let (mut x, extra_gap) = justify_offsets(justify, extra, children.len());
                for (i, &child) in children.iter().enumerate() {
                    let child_size = if align == AlignItems::Stretch {
                        let stretched_c = Constraints::new([0.0, size[0]], [size[1], size[1]]);
                        ctx.measure_child_size(child, stretched_c)
                    } else {
                        natural[i]
                    };
                    let y = align_offset(align, size[1], child_size[1]);
                    ctx.arrange_child(child, [x, y], my_affine, child_size);
                    x += child_size[0] + gap + extra_gap;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! `justify_offsets`/`align_offset` are pure arrange-math helpers. Given
    //! this crate's current widget set never hands a `Column`/`Row` more
    //! main-axis space than its children's natural sum (see
    //! `JustifyContent`'s doc comment), `justify_offsets`'s non-zero-`extra`
    //! branches are otherwise unreachable through an end-to-end layout test
    //! today — exercised directly here instead.
    use super::*;

    #[test]
    fn justify_start_has_no_offset_and_no_gap_inflation() {
        assert_eq!(justify_offsets(JustifyContent::Start, 40.0, 3), (0.0, 0.0));
    }

    #[test]
    fn justify_end_shifts_the_whole_block_by_the_full_extra() {
        assert_eq!(justify_offsets(JustifyContent::End, 40.0, 3), (40.0, 0.0));
    }

    #[test]
    fn justify_center_shifts_the_whole_block_by_half_the_extra() {
        assert_eq!(justify_offsets(JustifyContent::Center, 40.0, 3), (20.0, 0.0));
    }

    #[test]
    fn justify_space_between_inflates_only_inter_child_gaps() {
        let (start, extra_gap) = justify_offsets(JustifyContent::SpaceBetween, 40.0, 3);
        assert_eq!(start, 0.0);
        assert_eq!(extra_gap, 20.0); // 40 spread across (3 - 1) gaps
    }

    #[test]
    fn justify_space_between_with_a_single_child_has_no_gap_to_inflate() {
        let (start, extra_gap) = justify_offsets(JustifyContent::SpaceBetween, 40.0, 1);
        assert_eq!(start, 0.0);
        assert_eq!(extra_gap, 0.0);
    }

    #[test]
    fn justify_space_around_splits_extra_evenly_with_half_shares_at_each_end() {
        let (start, extra_gap) = justify_offsets(JustifyContent::SpaceAround, 60.0, 3);
        assert_eq!(extra_gap, 20.0); // 60 / 3 children
        assert_eq!(start, 10.0); // half a share before the first child
    }

    #[test]
    fn align_start_and_stretch_have_no_cross_axis_offset() {
        assert_eq!(align_offset(AlignItems::Start, 100.0, 30.0), 0.0);
        assert_eq!(align_offset(AlignItems::Stretch, 100.0, 30.0), 0.0);
    }

    #[test]
    fn align_end_places_the_child_flush_with_the_far_edge() {
        assert_eq!(align_offset(AlignItems::End, 100.0, 30.0), 70.0);
    }

    #[test]
    fn align_center_splits_the_leftover_cross_space_evenly() {
        assert_eq!(align_offset(AlignItems::Center, 100.0, 30.0), 35.0);
    }
}
