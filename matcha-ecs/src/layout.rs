//! Layout: the protocol a widget implements, and the pass that runs it.
//!
//! - **Protocol** — [`Constraints`] and [`Measured`] (what a parent asks and
//!   what a child reports), the [`Layout`] trait, and the [`LayoutDispatch`]
//!   fn-pointer table that lets one exclusive system walk an arbitrarily-typed
//!   tree without the core knowing any concrete layout type
//!   (`ECS_ARCHITECTURE.md` §8.3/§8.4).
//! - **Pass** — [`LayoutCtx`], which carries the world and the frame's measure
//!   cache and is the only thing a `Layout` impl is handed, plus the two ways
//!   to start one: [`layout_root`] (a root and explicit constraints — what
//!   tests drive) and [`run_layout`] (the registered system, which resolves the
//!   UI root and its window size and then calls it).
//!
//! Core owns the protocol; concrete layouts (`Column`, `Row`, ...) live in
//! `matcha-ecs-widgets` and are wired in purely by including
//! `(XxxLayout, LayoutDispatch::of::<XxxLayout>())` in a widget's `bundle()` —
//! no registration step exists.
//!
//! Kept in one file deliberately: implementing a layout means reading the
//! constraint vocabulary, the trait and `LayoutCtx`'s methods together, and
//! splitting them would only add navigation to the most common reading path.
//! Worth revisiting when `LayoutCtx` grows a real cache policy or an
//! intrinsic-sizing pass of its own — not for size alone.

use std::collections::HashMap;

use bevy_ecs::{component::Component, entity::Entity, world::World};
use nalgebra::{Matrix4, Vector3};

use crate::{
    components::{
        layout::{GlobalTransform, Hidden, LayoutOutput},
        view::ViewChildren,
    },
    resources::ui_root_window,
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

/// What a child reports to its parent: the range of sizes it will accept, and
/// the one it wants.
///
/// [`preferred`](Self::preferred) is exactly what `measure` returned before
/// this type existed, so a parent that does not care about ranges reads that
/// field and behaves identically. [`min`](Self::min) and [`max`](Self::max)
/// correspond to CSS's min-content and max-content *contributions*, already
/// folded together with whatever `min-*`/`max-*` the widget declares for
/// itself — the parent does not need to know which part came from where.
///
/// # Height depends on width
///
/// The height components of `min`/`max` are **not** "the height at that
/// width". Text at its min-content width is very tall and at its max-content
/// width is one line, so a single call cannot report both a width range and
/// the height belonging to each. Resolving a layout is therefore two passes:
/// take the range here, decide a width, then measure again at that width to
/// learn the height. CSS works the same way; this is why measuring is cached
/// (see [`LayoutCtx::measure_child`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Measured {
    /// Smallest size this child will accept without overflowing.
    pub min: [f32; 2],
    /// The size it wants, within the constraints it was given.
    pub preferred: [f32; 2],
    /// Largest size it can make use of; more than this is wasted on it.
    pub max: [f32; 2],
    /// Reserved for `align-items: baseline`. Always `None` today — no layout
    /// reads it yet, and nothing produces it.
    pub baseline: Option<f32>,
}

impl Measured {
    /// A child that accepts exactly one size. The default answer while a
    /// widget has no reason to report a range.
    pub fn exact(size: [f32; 2]) -> Self {
        Self {
            min: size,
            preferred: size,
            max: size,
            baseline: None,
        }
    }

    pub fn new(min: [f32; 2], preferred: [f32; 2], max: [f32; 2]) -> Self {
        debug_assert!(
            min[0] <= preferred[0]
                && preferred[0] <= max[0]
                && min[1] <= preferred[1]
                && preferred[1] <= max[1],
            "Measured range is inconsistent: min={min:?}, preferred={preferred:?}, max={max:?}"
        );
        Self {
            min,
            preferred,
            max,
            baseline: None,
        }
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
    /// Report this entity's acceptable size range within `constraints`.
    /// Recurses into children via `ctx.measure_child`; writes nothing.
    ///
    /// **Must be a pure function of `constraints` and the world as it stood at
    /// the start of the layout pass.** Results are cached for the duration of
    /// the pass (see [`LayoutCtx::measure_child`]), so a `measure` that reads
    /// state some `arrange` writes mid-pass can be served a value from before
    /// that write. A layout that genuinely needs the fresh value must ask for
    /// it with [`LayoutCtx::measure_child_uncached`].
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, constraints: Constraints) -> Measured;

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
    measure: fn(&mut LayoutCtx, Entity, Constraints) -> Measured,
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
    /// Measure results for this pass only.
    ///
    /// The cache lives and dies with the `LayoutCtx`, which is built inside
    /// [`layout_root`] and dropped when the pass ends — so it cannot outlive a
    /// frame and there is nothing to invalidate. `Constraints` is quantized
    /// `u32` and already `Hash + Eq`, so it keys directly.
    cache: HashMap<(Entity, Constraints), Measured>,
}

impl<'w> LayoutCtx<'w> {
    pub fn world(&self) -> &World {
        self.world
    }

    /// `me`'s declared view children in declaration order, minus any that are
    /// [`Hidden`].
    ///
    /// Every container reaches its children through here, which is what makes
    /// `display: none` work without a single container knowing about it.
    pub fn children(&self, me: Entity) -> Vec<Entity> {
        self.world
            .get::<ViewChildren>(me)
            .map(|vc| visible_children(self.world, vc))
            .unwrap_or_default()
    }

    /// Recursively measure `child` against `constraints`. Pure: writes no
    /// components.
    ///
    /// Cached for the rest of this layout pass, since resolving a size
    /// legitimately measures the same child more than once — `Column::arrange`
    /// already re-measures every child, and intrinsic sizing needs a second
    /// pass by construction (see [`Measured`]). Without the cache those
    /// repeats multiply with depth.
    pub fn measure_child(&mut self, child: Entity, constraints: Constraints) -> Measured {
        if let Some(hit) = self.cache.get(&(child, constraints)) {
            return *hit;
        }
        let measured = self.measure_child_uncached(child, constraints);
        self.cache.insert((child, constraints), measured);
        measured
    }

    /// [`measure_child`](Self::measure_child) keeping only the size the child
    /// asked for. The common case for a layout that does not reason about
    /// ranges.
    pub fn measure_child_size(&mut self, child: Entity, constraints: Constraints) -> [f32; 2] {
        self.measure_child(child, constraints).preferred
    }

    /// Measure without consulting or filling the cache.
    ///
    /// The escape hatch for a layout whose `measure` cannot honour the purity
    /// contract on [`Layout::measure`] — it reads state that an `arrange`
    /// earlier in the same pass wrote. Nothing needs this today.
    pub fn measure_child_uncached(&mut self, child: Entity, constraints: Constraints) -> Measured {
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
    // Same `Hidden` filter `LayoutCtx::children` applies. Spelled out because
    // the root is read directly here rather than through the ctx, which does
    // not exist yet.
    let top_level: Vec<Entity> = world
        .get::<ViewChildren>(root)
        .map(|vc| visible_children(world, vc))
        .unwrap_or_default();

    let mut ctx = LayoutCtx {
        world,
        cache: HashMap::new(),
    };
    for child in top_level {
        let size = ctx.measure_child_size(child, constraints);
        ctx.arrange_child(child, [0.0, 0.0], Matrix4::identity(), size);
    }
}

/// Measure a single entity as a pass of its own, without arranging anything.
///
/// The counterpart to [`layout_root`] for callers that want a child's report
/// rather than a placement — a parent driving its own sizing outside the
/// normal walk, and tests asserting on a widget's [`Measured`] range, which
/// `layout_root` discards.
pub fn measure_entity(world: &mut World, entity: Entity, constraints: Constraints) -> Measured {
    let mut ctx = LayoutCtx {
        world,
        cache: HashMap::new(),
    };
    ctx.measure_child(entity, constraints)
}

/// Declared children in order, skipping [`Hidden`] ones.
fn visible_children(world: &World, children: &ViewChildren) -> Vec<Entity> {
    children
        .slots
        .iter()
        .map(|(_, e)| *e)
        .filter(|e| world.get::<Hidden>(*e).is_none())
        .collect()
}

/// Exclusive system: find the window root and lay out its view tree against
/// the window's current inner size. Registered in `MatchaSet::Layout`.
pub fn run_layout(world: &mut World) {
    let Some((root, window)) = ui_root_window(world) else {
        return;
    };
    let inner = window.window.inner_size();
    // The window reports physical pixels; layout works in UI pixels.
    let scale = world
        .get::<crate::components::window::UiScale>(root)
        .copied()
        .unwrap_or_default();
    let size = scale.to_ui([inner[0] as f32, inner[1] as f32]);
    let constraints = Constraints::from_max_size(size);

    layout_root(world, root, constraints);
}
