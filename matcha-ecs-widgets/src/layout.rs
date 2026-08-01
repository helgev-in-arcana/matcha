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

use crate::sizing::Sizing;

pub mod distribute;

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
/// Only has an effect when the container is larger on its main axis than its
/// children's natural sum, which means giving it a main-axis size of its own:
/// `Column::new().height(Length::Fill).justify_content(..)`. Left at the
/// default `Auto` sizing a `Column`/`Row` is exactly as tall/wide as its
/// content, so there is no leftover space to distribute.
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

/// Reverses the main axis, as CSS's `row-reverse`/`column-reverse` do: the
/// last child is laid out first, from what was the far edge.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Reverse(bool);

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Justify(JustifyContent);
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Align(AlignItems);

/// A neutral container with a single child area.
pub struct Container {
    key: Key,
    visible: bool,
    sizing: Sizing,
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
            sizing: Sizing::default(),
        }
    }

    crate::sizing_builders!();

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
        (
            LayoutKind::Container,
            self.sizing,
            LayoutDispatch::of::<LayoutKind>(),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        self.sync_visible(entity);
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        // LayoutKind is constant for this type.
        self.sync_visible(entity);
        if let Some(mut sizing) = entity.get_mut::<Sizing>() {
            sizing.set_if_neq(self.sizing);
        }
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
            reverse: bool,
            sizing: Sizing,
        }

        impl $name {
            pub fn new() -> Self {
                Self {
                    key: Key::Auto,
                    gap: 0.0,
                    justify: JustifyContent::default(),
                    align: AlignItems::default(),
                    reverse: false,
                    sizing: Sizing::default(),
                }
            }

            /// Lay the children out from the far end of the main axis, as
            /// CSS's `row-reverse`/`column-reverse` do.
            pub fn reverse(mut self, reverse: bool) -> Self {
                self.reverse = reverse;
                self
            }

            crate::sizing_builders!();

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
                    Reverse(self.reverse),
                    self.sizing,
                    LayoutDispatch::of::<LayoutKind>(),
                )
            }

            fn patch(&self, entity: &mut EntityWorldMut) {
                if let Some(mut gap) = entity.get_mut::<Gap>() {
                    gap.set_if_neq(Gap(self.gap));
                }
                if let Some(mut sizing) = entity.get_mut::<Sizing>() {
                    sizing.set_if_neq(self.sizing);
                }
                if let Some(mut justify) = entity.get_mut::<Justify>() {
                    justify.set_if_neq(Justify(self.justify));
                }
                if let Some(mut align) = entity.get_mut::<Align>() {
                    align.set_if_neq(Align(self.align));
                }
                if let Some(mut reverse) = entity.get_mut::<Reverse>() {
                    reverse.set_if_neq(Reverse(self.reverse));
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

/// What a container needs to know about one child to place it.
struct ChildInfo {
    item: distribute::Item,
    sizing: Sizing,
    /// The size it asked for before any distribution, kept so a child whose
    /// size did not change need not be measured a third time.
    natural: [f32; 2],
}

/// Measure each child and turn its report into a distribution input.
///
/// The bounds come from the child's *declared* `min-*`/`max-*` rather than
/// from what it measured: growth is limited by an explicit maximum, never by
/// the content's own max-content size, and `Auto` on the minimum means "down
/// to min-content" — CSS's `min-width: auto` on a flex item.
fn collect_children(
    ctx: &mut LayoutCtx,
    children: &[Entity],
    child_c: Constraints,
    main: usize,
    main_available: f32,
) -> Vec<ChildInfo> {
    children
        .iter()
        .map(|&child| {
            let measured = ctx.measure_child(child, child_c);
            let sizing = Sizing::of(ctx, child);
            ChildInfo {
                item: distribute::Item {
                    base: measured.preferred[main],
                    min: sizing
                        .min_bound(main, main_available)
                        .unwrap_or(measured.min[main]),
                    max: sizing
                        .max_bound(main, main_available)
                        .unwrap_or(f32::INFINITY),
                    grow: sizing.grow,
                    shrink: sizing.shrink,
                },
                sizing,
                natural: measured.preferred,
            }
        })
        .collect()
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

    fn reverse(&self, ctx: &LayoutCtx, me: Entity) -> bool {
        ctx.world().get::<Reverse>(me).map(|r| r.0).unwrap_or(false)
    }

    /// Children in layout order: by `order`, ties keeping declaration order,
    /// then flipped for `reverse`.
    ///
    /// This reorders layout only — painting still follows declaration order,
    /// which is invisible here because a `Column`/`Row` never overlaps its
    /// children. Unifying the two orders is its own step in the roadmap.
    fn ordered(ctx: &LayoutCtx, children: &[Entity], reverse: bool) -> Vec<Entity> {
        let mut indexed: Vec<(i32, usize, Entity)> = children
            .iter()
            .enumerate()
            .map(|(i, &e)| (Sizing::of(ctx, e).order, i, e))
            .collect();
        indexed.sort_by_key(|(order, i, _)| (*order, *i));

        let mut out: Vec<Entity> = indexed.into_iter().map(|(_, _, e)| e).collect();
        if reverse {
            out.reverse();
        }
        out
    }

    /// Index into an `[x, y]` pair of the axis children stack along.
    /// Meaningless for `Container`, which stacks nothing.
    fn main_axis(&self) -> usize {
        match self {
            LayoutKind::Row => 0,
            LayoutKind::Column | LayoutKind::Container => 1,
        }
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
        let sizing = Sizing::of(ctx, me);
        // The content sees the box's own size where it has one, so a `Fill`
        // column wraps its text to the space it took rather than to the
        // window.
        let inner = sizing.content_constraints(c);
        let children = ctx.children(me);

        let content = match self {
            LayoutKind::Container => match children.first() {
                Some(&child) => ctx.measure_child(child, inner),
                None => Measured::exact([0.0, 0.0]),
            },
            LayoutKind::Column | LayoutKind::Row => {
                let main = self.main_axis();
                let cross = 1 - main;
                let gap = self.gap(ctx, me);
                let sizes: Vec<[f32; 2]> = children
                    .iter()
                    .map(|&e| ctx.measure_child_size(e, inner))
                    .collect();
                let mut size = [0.0; 2];
                size[main] = sizes.iter().map(|s| s[main]).sum::<f32>()
                    + gap * sizes.len().saturating_sub(1) as f32;
                size[cross] = sizes.iter().map(|s| s[cross]).fold(0.0, f32::max);
                Measured::exact(size)
            }
        };

        sizing.measured(c, content)
    }

    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, size: [f32; 2]) {
        let children = ctx.children(me);
        let my_affine = Self::my_affine(ctx, me);
        let child_c = Constraints::from_max_size(size);

        if let LayoutKind::Container = self {
            if let Some(&child) = children.first() {
                // A container has no main axis, so a child's `align_self`
                // applies to both: it is placed within the container's box.
                let align = Sizing::of(ctx, child).align_self.unwrap_or(AlignItems::Start);
                let child_size = if align == AlignItems::Stretch {
                    ctx.measure_child_size(child, Constraints::new([size[0], size[0]], [size[1], size[1]]))
                } else {
                    ctx.measure_child_size(child, child_c)
                };
                let origin = [
                    align_offset(align, size[0], child_size[0]),
                    align_offset(align, size[1], child_size[1]),
                ];
                ctx.arrange_child(child, origin, my_affine, child_size);
            }
            return;
        }

        // Column and Row differ only in which axis stacks.
        let main = self.main_axis();
        let cross = 1 - main;
        let gap = self.gap(ctx, me);
        let justify = self.justify(ctx, me);
        let align = self.align(ctx, me);
        let children = Self::ordered(ctx, &children, self.reverse(ctx, me));

        // Gaps are not distributable space; only what is left after them is.
        let gaps = gap * children.len().saturating_sub(1) as f32;
        let available = (size[main] - gaps).max(0.0);

        let infos = collect_children(ctx, &children, child_c, main, size[main]);
        let items: Vec<distribute::Item> = infos.iter().map(|i| i.item).collect();
        let resolved = distribute::distribute(&items, available);

        let used: f32 = resolved.iter().sum::<f32>() + gaps;
        let extra = (size[main] - used).max(0.0);
        let (mut main_pos, extra_gap) = justify_offsets(justify, extra, children.len());

        for (i, &child) in children.iter().enumerate() {
            let info = &infos[i];
            let align = info.sizing.align_self.unwrap_or(align);
            let flexed = (resolved[i] - info.item.base).abs() >= 0.01;

            let child_size = if !flexed && align != AlignItems::Stretch {
                // Nothing moved, so the child has already answered — the
                // common case for every container whose children just fit.
                info.natural
            } else {
                // Ask again now that the main size is settled: the cross size
                // may depend on it (text wraps to the width it ended up with).
                let mut lo = [0.0; 2];
                let mut hi = size;
                lo[main] = resolved[i];
                hi[main] = resolved[i];
                if align == AlignItems::Stretch {
                    lo[cross] = size[cross];
                    hi[cross] = size[cross];
                }
                let settled =
                    Constraints::new([lo[0], hi[0].max(lo[0])], [lo[1], hi[1].max(lo[1])]);
                let mut settled_size = ctx.measure_child_size(child, settled);
                // The distributed main size is imposed, not offered: a child
                // with a definite width would otherwise answer with that width
                // and undo the distribution. Its own `width` is the base the
                // distribution started from, so it has already been heard.
                // The cross size stays the child's own answer, which is why an
                // explicitly sized child is not stretched — CSS's rule too.
                settled_size[main] = resolved[i];
                settled_size
            };

            let mut origin = [0.0; 2];
            origin[main] = main_pos;
            origin[cross] = align_offset(align, size[cross], child_size[cross]);
            ctx.arrange_child(child, origin, my_affine, child_size);
            main_pos += child_size[main] + gap + extra_gap;
        }
    }
}

#[cfg(test)]
mod tests {
    //! `justify_offsets`/`align_offset` are pure arrange-math helpers, covered
    //! here case by case; `matcha-ecs/tests/sizing.rs` drives the interesting
    //! ones end-to-end through a `Fill`-sized column.
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
