//! Layout protocol: `Constraints`, the `Layout` trait, and the `LayoutDispatch`
//! fn-pointer table that lets a single exclusive system ([`layout_root`]) walk
//! an arbitrarily-typed tree of layouts without the core knowing any concrete
//! layout type (`ECS_ARCHITECTURE.md` §8.3/§8.4).
//!
//! Core owns the protocol; concrete layouts (`Column`, `Row`, ...) live in
//! `matcha-ecs-widgets` and are wired in purely by including
//! `(XxxLayout, LayoutDispatch::of::<XxxLayout>())` in a widget's `bundle()` —
//! no registration step exists.

use bevy_ecs::{component::Component, entity::Entity, world::World};
use nalgebra::{Matrix4, Vector3};

use crate::{
    components::{
        layout::{GlobalTransform, LayoutOutput},
        view::ViewChildren,
        window::Window as WindowComp,
    },
    resources::RenderWindowRoot,
};

/// Quantization factor for layout size/bound keys, matching
/// `matcha-tree/src/ui_tree/metrics.rs` so the two crates' notions of
/// "the same size" agree.
pub const SUB_PIXEL_QUANTIZE: f32 = (1 << 8) as f32;

/// Min/max width and height passed from a parent layout down to a child.
/// Ported from `matcha-tree`'s `Constraints`; values are stored quantized
/// (`u32`) so equal-in-practice sizes compare equal despite float noise.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Constraints {
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

impl std::fmt::Debug for Constraints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Constraints(min_width={}, max_width={}, min_height={}, max_height={})",
            self.min_width(),
            self.max_width(),
            self.min_height(),
            self.max_height()
        )
    }
}

impl Constraints {
    /// A max that stands in for "no upper bound on this axis".
    ///
    /// `Constraints` stores quantized `u32`, so a true infinity is not
    /// representable — and `f32::INFINITY`/`f32::MAX` would saturate the
    /// quantizer rather than round-trip. `2^20` px is a sixteenth of what the
    /// quantizer can hold (`u32::MAX / SUB_PIXEL_QUANTIZE`, ~16.7M px) and is
    /// exact through it, while being far larger than any content a layout will
    /// legitimately produce.
    ///
    /// Used by a scrolling container to measure its content at its natural
    /// size on the axis it scrolls. Note that a widget which *wraps* to the
    /// incoming max (`Text`/`RichText` take their wrap width from
    /// `max_width()`) will not wrap at all on an axis measured this way — pass
    /// it only on the axis that genuinely scrolls.
    pub const UNBOUNDED: f32 = (1u32 << 20) as f32;

    /// `[min, max]` for each axis.
    pub fn new(width: [f32; 2], height: [f32; 2]) -> Self {
        if width[0] < 0.0 || width[0] > width[1] || height[0] < 0.0 || height[0] > height[1] {
            panic!("Invalid constraints: width={width:?}, height={height:?}");
        }
        Self {
            min_width: (width[0] * SUB_PIXEL_QUANTIZE) as u32,
            max_width: (width[1] * SUB_PIXEL_QUANTIZE) as u32,
            min_height: (height[0] * SUB_PIXEL_QUANTIZE) as u32,
            max_height: (height[1] * SUB_PIXEL_QUANTIZE) as u32,
        }
    }

    /// `min = 0`, `max = size` on both axes — "up to this much space".
    pub fn from_max_size(size: [f32; 2]) -> Self {
        if size[0] < 0.0 || size[1] < 0.0 {
            panic!("Invalid constraints: {size:?}");
        }
        Self {
            min_width: 0,
            max_width: (size[0] * SUB_PIXEL_QUANTIZE) as u32,
            min_height: 0,
            max_height: (size[1] * SUB_PIXEL_QUANTIZE) as u32,
        }
    }

    pub const fn min_width(&self) -> f32 {
        self.min_width as f32 / SUB_PIXEL_QUANTIZE
    }
    pub const fn max_width(&self) -> f32 {
        self.max_width as f32 / SUB_PIXEL_QUANTIZE
    }
    pub const fn min_height(&self) -> f32 {
        self.min_height as f32 / SUB_PIXEL_QUANTIZE
    }
    pub const fn max_height(&self) -> f32 {
        self.max_height as f32 / SUB_PIXEL_QUANTIZE
    }
    pub const fn max_size(&self) -> [f32; 2] {
        [self.max_width(), self.max_height()]
    }
}

/// A concrete layout algorithm. `measure` is pure (bottom-up, no writes);
/// `arrange` positions children via [`LayoutCtx::arrange_child`] (top-down,
/// writes `LayoutOutput`/`GlobalTransform`).
///
/// Implementors are widget-side data components (`Column`'s `LayoutKind`,
/// `ColorRect`'s size, ...); the core never names a concrete `Layout` type.
///
/// `LayoutDispatch::of` clones `L` out of the world on every dispatch (see
/// its doc comment) to release the borrow before recursing. A `Clone` impl
/// with unbounded cost (e.g. deep-copying a `Vec`) is on the implementor —
/// see `ECS_IMPLEMENTATION_PLAN.md` §5 progress log for why this is a known,
/// open gap rather than a `Copy` bound (rejected: too restrictive) or an
/// `unsafe` disjoint-borrow fix (rejected for now: soundness would depend on
/// `view.rs`'s reconcile invariants, a cross-module dependency not yet worth
/// taking on).
pub trait Layout: Component + Clone {
    /// Return this entity's desired size within `constraints`. Recurses into
    /// children via `ctx.measure_child`; writes nothing.
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, constraints: Constraints) -> [f32; 2];

    /// Place children within the `size` this entity was allocated by its
    /// parent (or by [`layout_root`] for a top-level entity), via
    /// `ctx.arrange_child`.
    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, size: [f32; 2]);
}

/// Two fn pointers that let [`LayoutCtx`] measure/arrange an entity without
/// knowing its concrete `Layout` type. One per widget entity, baked into its
/// `bundle()` via [`LayoutDispatch::of`] — there is no registry.
#[derive(Component, Clone, Copy)]
pub struct LayoutDispatch {
    measure: fn(&mut LayoutCtx, Entity, Constraints) -> [f32; 2],
    arrange: fn(&mut LayoutCtx, Entity, [f32; 2]),
}

impl LayoutDispatch {
    /// Build a dispatch table for `L`. The generated fns clone `L` out of the
    /// world (releasing the borrow) before calling `L::measure`/`L::arrange`,
    /// so the subsequent recursive `&mut LayoutCtx` call never conflicts with
    /// a held component borrow (`ECS_ARCHITECTURE.md` §8.3).
    pub fn of<L: Layout>() -> Self {
        Self {
            measure: |ctx, e, c| {
                let l: L = ctx
                    .world()
                    .get::<L>(e)
                    .unwrap_or_else(|| {
                        panic!(
                            "entity {e:?} has LayoutDispatch::of::<{}>() but no {} component",
                            std::any::type_name::<L>(),
                            std::any::type_name::<L>()
                        )
                    })
                    .clone();
                l.measure(ctx, e, c)
            },
            arrange: |ctx, e, size| {
                let l: L = ctx
                    .world()
                    .get::<L>(e)
                    .unwrap_or_else(|| {
                        panic!(
                            "entity {e:?} has LayoutDispatch::of::<{}>() but no {} component",
                            std::any::type_name::<L>(),
                            std::any::type_name::<L>()
                        )
                    })
                    .clone();
                l.arrange(ctx, e, size)
            },
        }
    }
}

/// The recursion entry point handed to `Layout` implementations. Wraps the
/// `&mut World` being laid out; `measure_child`/`arrange_child` are the only
/// way a `Layout` impl should recurse into its children.
pub struct LayoutCtx<'w> {
    world: &'w mut World,
}

impl<'w> LayoutCtx<'w> {
    pub fn world(&self) -> &World {
        self.world
    }

    /// `me`'s declared view children, in declaration order.
    pub fn children(&self, me: Entity) -> Vec<Entity> {
        self.world
            .get::<ViewChildren>(me)
            .map(|vc| vc.slots.iter().map(|(_, e)| *e).collect())
            .unwrap_or_default()
    }

    /// Recursively measure `child` against `constraints`. Pure: writes no components.
    pub fn measure_child(&mut self, child: Entity, constraints: Constraints) -> [f32; 2] {
        let dispatch = *self.world.get::<LayoutDispatch>(child).unwrap_or_else(|| {
            panic!("entity {child:?} has no LayoutDispatch; every entity reachable from a layout-managed subtree must carry one")
        });
        (dispatch.measure)(self, child, constraints)
    }

    /// Place `child` at `origin` (local to `parent_affine`'s space) with
    /// allocated `size`: writes `LayoutOutput`/`GlobalTransform` on `child`,
    /// then recurses into the child's own `Layout::arrange`.
    pub fn arrange_child(
        &mut self,
        child: Entity,
        origin: [f32; 2],
        parent_affine: Matrix4<f32>,
        size: [f32; 2],
    ) {
        let global_affine =
            parent_affine * Matrix4::new_translation(&Vector3::new(origin[0], origin[1], 0.0));

        match self.world.get_mut::<LayoutOutput>(child) {
            Some(mut out) => {
                use bevy_ecs::change_detection::DetectChangesMut;
                out.set_if_neq(LayoutOutput { size, origin });
            }
            None => {
                self.world.entity_mut(child).insert(LayoutOutput { size, origin });
            }
        }
        match self.world.get_mut::<GlobalTransform>(child) {
            Some(mut gt) => {
                use bevy_ecs::change_detection::DetectChangesMut;
                gt.set_if_neq(GlobalTransform { affine: global_affine });
            }
            None => {
                self.world
                    .entity_mut(child)
                    .insert(GlobalTransform { affine: global_affine });
            }
        }

        let dispatch = *self.world.get::<LayoutDispatch>(child).unwrap_or_else(|| {
            panic!("entity {child:?} has no LayoutDispatch; every entity reachable from a layout-managed subtree must carry one")
        });
        (dispatch.arrange)(self, child, size);
    }
}

/// Testable layout core: measure + arrange `root`'s declared view children
/// against `constraints`, each anchored at `[0.0, 0.0]` in window space.
///
/// `root` itself is treated as a transparent container (it has no
/// `LayoutDispatch` of its own — it is either the window-root entity or a
/// headless test root spawned with just a `ViewChildren`).
pub fn layout_root(world: &mut World, root: Entity, constraints: Constraints) {
    let top_level: Vec<Entity> = world
        .get::<ViewChildren>(root)
        .map(|vc| vc.slots.iter().map(|(_, e)| *e).collect())
        .unwrap_or_default();

    let mut ctx = LayoutCtx { world };
    for child in top_level {
        let size = ctx.measure_child(child, constraints);
        ctx.arrange_child(child, [0.0, 0.0], Matrix4::identity(), size);
    }
}

/// Exclusive system: find the window root and lay out its view tree against
/// the window's current inner size. Registered in `MatchaSet::Layout`.
pub fn run_layout(world: &mut World) {
    let Some(root) = world.get_resource::<RenderWindowRoot>().map(|r| r.entity) else {
        return;
    };
    let Some(window) = world.get::<WindowComp>(root) else {
        return;
    };
    let inner = window.window.inner_size();
    let constraints = Constraints::from_max_size([inner[0] as f32, inner[1] as f32]);

    layout_root(world, root, constraints);
}
