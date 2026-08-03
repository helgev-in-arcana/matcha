//! `Anchor` — place a child at an offset, out of the flow.
//!
//! # What this buys, and why it is not `position: absolute`
//!
//! Tooltips, dropdowns and context menus all want the same two things: draw
//! somewhere the parent's layout would not have put me, and draw *over* my
//! siblings. The second is already solved — [`ZIndex`] reorders painting and
//! picking together — so all that was missing was the first, and it is a
//! `Layout` impl of about ten lines.
//!
//! This is deliberately **not** CSS `position: absolute`. It does not escape to
//! a positioned ancestor, and it does not resolve `top`/`left`/`right`/`bottom`
//! against a containing block; the offset is from the anchor's own top-left,
//! full stop. Real absolute positioning needs a containing-block chain in the
//! core, which is out of scope by an earlier decision. What is here covers the
//! overlay cases that motivated it, in the widget layer, with no core change.
//!
//! # The one thing to know
//!
//! **An anchor is zero-sized.** It measures to nothing, so the flow closes up
//! around it exactly as if it were not there, and the child is drawn outside
//! its parent's box. That means a container that clips ([`Panel::clip`], any
//! `ScrollView`) will cut the child off — which is correct, and is also why a
//! dropdown belongs at the top of the view rather than inside the scroll box
//! whose row opened it.

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    world::EntityWorldMut,
};
use nalgebra::Matrix4;

use matcha_ecs::{
    components::{
        layout::GlobalTransform,
        render::ZIndex,
        view::Key,
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
    view::Widget,
};

/// An [`Anchor`]'s offset — doubles as its `Layout` impl, like `PanelLayout`.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct AnchorLayout {
    pub offset: [f32; 2],
}

/// A zero-sized container that draws its child at an offset, over whatever the
/// flow put nearby.
pub struct Anchor {
    key: Key,
    offset: [f32; 2],
    z_index: i32,
}

impl Anchor {
    /// Place the child `[x, y]` from where the anchor itself sits.
    pub fn at(x: f32, y: f32) -> Self {
        Self {
            key: Key::Auto,
            offset: [x, y],
            z_index: 1,
        }
    }

    /// Where the child sits among its parent's other children, low to high
    /// (CSS `z-index`). Default `1`, i.e. above ordinary siblings.
    ///
    /// This reorders **painting and picking together**, so an overlay that
    /// covers a button also intercepts its clicks — which is what an overlay is
    /// for. Note the ordering is among *siblings*: an anchor cannot lift its
    /// child above its own parent, so a menu belongs high in the tree.
    pub fn z_index(mut self, z: i32) -> Self {
        self.z_index = z;
        self
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn layout(&self) -> AnchorLayout {
        AnchorLayout {
            offset: self.offset,
        }
    }
}

impl Widget for Anchor {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.layout(),
            LayoutDispatch::of::<AnchorLayout>(),
            ZIndex(self.z_index),
        )
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        if let Some(mut l) = entity.get_mut::<AnchorLayout>() {
            l.set_if_neq(self.layout());
        }
        if let Some(mut z) = entity.get_mut::<ZIndex>() {
            z.set_if_neq(ZIndex(self.z_index));
        }
        // No `RenderItem`: an anchor draws nothing itself, so there is nothing
        // to invalidate. Its child is repositioned by `arrange` every frame,
        // and the generic `invalidate_on_layout_change` covers that.
    }
}

impl Layout for AnchorLayout {
    fn measure(&self, _ctx: &mut LayoutCtx, _me: Entity, _c: Constraints) -> Measured {
        // Zero-sized on purpose: the flow must close up around an anchor as if
        // it were not declared at all. The child is measured in `arrange`,
        // where the offset is applied.
        Measured::exact([0.0, 0.0])
    }

    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, _size: [f32; 2]) {
        let my_affine = ctx
            .world()
            .get::<GlobalTransform>(me)
            .map(|t| t.affine)
            .unwrap_or_else(Matrix4::identity);

        let Some(&child) = ctx.children(me).first() else {
            return;
        };
        // The child is measured **unbounded**: an overlay is not confined by
        // whatever space the flow had left over at the anchor's position —
        // that is the whole point of being out of flow. It is the caller's job
        // to give the child a size it wants.
        let child_size = ctx.measure_child_size(
            child,
            Constraints::from_max_size([Constraints::UNBOUNDED; 2]),
        );
        ctx.arrange_child(child, self.offset, my_affine, child_size);
    }
}
