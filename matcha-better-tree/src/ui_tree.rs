//! UI ownership, reconciliation, layout, and input. Storage lives in the generic arena module.
use std::any::{Any, TypeId};
use std::sync::atomic::{AtomicU64, Ordering};

use matcha_window::event::device_event::DeviceEvent;
use nalgebra::{Matrix4, Vector4};
use smallvec::SmallVec;

use self::context::Context;
use self::widget::{Draw, InputPropagation, InputResult, UiError, UpdateResult, View, Widget};
use crate::arena::{Arena, Id as UiId};
use crate::metrics::{Bounds, Constraints, Point};

mod cache;
pub mod cache_epoch;
pub mod context;
mod input;
mod layout;
mod render;
pub mod widget;
pub mod window;

use cache::NodeCache;

const CHILDREN_SMALLVEC: usize = 4;

/// Owns a UI tree. Declaration/children contexts expose operations without exposing its arena.
/// Widget callbacks that panic can leave reconciliation incomplete. Discard the tree and its
/// window registry after catching such a panic; callbacks are not a transactional boundary.
pub struct UiTree {
    nodes: Arena<UiNode>,
    root: Option<UiId>,
}

trait AnyView: Any {
    fn label(&self) -> Option<&str>;
    fn ui_key(&self) -> UiKey;
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> Box<dyn AnyWidget>;
}

impl<V: View> AnyView for V {
    fn label(&self) -> Option<&str> {
        View::label(self)
    }
    fn ui_key(&self) -> UiKey {
        match View::key(self) {
            Some(key) => UiKey::Key(key),
            // `TypeId::of::<V>()` rather than `self.type_id()`: the latter is easy to call on a
            // reference or trait object by accident and silently yields the wrong type.
            None => UiKey::Type(TypeId::of::<V>()),
        }
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> Box<dyn AnyWidget> {
        Box::new(View::build(self, ctx))
    }
}

trait AnyWidget: Any {
    fn try_update<'a>(
        &mut self,
        view: &'a dyn AnyView,
        ctx: &mut DeclContext<'a, '_>,
    ) -> Option<UpdateResult>;
    fn preview_input(
        &mut self,
        bounds: Bounds,
        event: &DeviceEvent,
        ctx: &mut Context,
    ) -> InputResult;
    fn input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: &mut Context) -> InputResult;
    fn is_inside(&self, b: Bounds, p: Point, ctx: &mut Context) -> bool;
    fn measure(
        &self,
        constraints: Constraints,
        children: Children<'_>,
        ctx: &mut Context,
    ) -> Bounds;
    fn layout(&self, bounds: Bounds, children: Children<'_>, ctx: &mut Context);
    fn uses_backdrop(&self) -> bool;
    fn render(&self, bounds: Bounds, ctx: &mut Context) -> Draw;
}

impl<W: Widget> AnyWidget for W {
    fn try_update<'a>(
        &mut self,
        view: &'a dyn AnyView,
        ctx: &mut DeclContext<'a, '_>,
    ) -> Option<UpdateResult> {
        let view = (view as &dyn Any).downcast_ref::<W::V>()?;
        Some(Widget::update(self, view, ctx))
    }

    fn preview_input(
        &mut self,
        bounds: Bounds,
        event: &DeviceEvent,
        ctx: &mut Context,
    ) -> InputResult {
        Widget::preview_input(self, bounds, event, ctx)
    }
    fn input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: &mut Context) -> InputResult {
        Widget::input(self, bounds, event, ctx)
    }
    fn is_inside(&self, b: Bounds, p: Point, ctx: &mut Context) -> bool {
        Widget::is_inside(self, b, p, ctx)
    }
    fn measure(
        &self,
        constraints: Constraints,
        children: Children<'_>,
        ctx: &mut Context,
    ) -> Bounds {
        Widget::measure(self, constraints, children, ctx)
    }
    fn layout(&self, bounds: Bounds, children: Children<'_>, ctx: &mut Context) {
        Widget::layout(self, bounds, children, ctx)
    }
    fn render(&self, bounds: Bounds, ctx: &mut Context) -> Draw {
        Widget::render(self, bounds, ctx)
    }
    fn uses_backdrop(&self) -> bool {
        Widget::uses_backdrop(self)
    }
}

/// Identity of a node among its siblings, used to match a new view against the old children.
///
/// - `Key`: the view returned an explicit key. Matched by key alone, regardless of type; if the
///   type changed, the node is rebuilt in place and keeps its `UiId`, `ChildId` and the
///   candidates for its own children.
/// - `Type`: no explicit key. Matched by type, in order of appearance among same-typed siblings.
///
/// The two variants never collide, so keyed and unkeyed siblings are matched independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum UiKey {
    Key(u64),
    Type(TypeId),
}

struct UiNode {
    // identify
    label: Option<String>,
    key: UiKey,

    // tree
    parent: Option<UiId>,
    children: SmallVec<[(ChildId, UiId); CHILDREN_SMALLVEC]>,

    // Current placement assigned by the parent, independent of layout cache validity.
    placement: Option<Placement>,

    cache: NodeCache,
    // Internal window hooks never become part of the public Widget trait.
    window: Option<crate::ui_tree::window::WindowBinding>,

    // widget main body
    widget: Option<Box<dyn AnyWidget>>,
}

#[derive(Clone, Copy)]
struct Placement {
    bounds: Bounds,
    // Child-local -> parent-local.
    affine: Matrix4<f32>,
    // Parent-local -> child-local. None means the relative affine is singular.
    affine_inverse: Option<Matrix4<f32>>,
}

impl Placement {
    fn new(position: crate::metrics::Position, previous: Option<Self>) -> Self {
        let affine_inverse = match previous {
            Some(old) if old.affine == position.affine => old.affine_inverse,
            _ => position.affine.try_inverse(),
        };
        Self {
            bounds: position.bounds,
            affine: position.affine,
            affine_inverse,
        }
    }

    /// Map a parent-local point only when this placement has usable local coordinates.
    fn local_point(self, point: Point) -> Option<Point> {
        let local = self.affine_inverse? * Vector4::new(point.x, point.y, point.z, 1.0);
        [local.x, local.y, local.z]
            .iter()
            .all(|value| value.is_finite())
            .then_some(Point {
                x: local.x,
                y: local.y,
                z: local.z,
            })
    }
}

impl UiNode {
    fn scoped_context<'a>(&self, parent: &'a mut Context<'_>) -> Context<'a> {
        crate::ui_tree::window::scoped_context(
            self.window,
            self.widget.as_deref().expect("live widget"),
            parent,
        )
    }

    /// `None` if `id` is not a current child of this node: stale, or issued to another parent.
    fn child(&self, id: ChildId) -> Option<UiId> {
        self.children
            .iter()
            .find(|(child_id, _)| *child_id == id)
            .map(|(_, ui_id)| *ui_id)
    }
}

impl UiTree {
    pub fn new() -> Self {
        Self {
            nodes: Arena::new(),
            root: None,
        }
    }

    /// Reconcile using application services borrowed for this operation.
    /// Window declarations require a registry supplied through Context. Use the
    /// same registry for this tree's lifetime; a registry serves one application tree.
    pub fn update<V: View>(&mut self, view: &V, context: &mut Context<'_>) {
        if let Some(id) = self.root {
            self.update_node(id, view, context);
        } else {
            // Clear storage explicitly before starting a fresh tree, including abandoned reservations.
            self.nodes.clear();
            let id = self.build_new(None, view, context);
            self.root = Some(id);
        }
    }

    /// Remove nodes and unregister their windows using borrowed application services.
    pub fn clear(&mut self, context: &mut Context<'_>) {
        if let Some(root) = self.root.take() {
            self.remove_reverse_scan_order(root, context);
        }
        self.nodes.clear();
    }

    /// Immediately release selected CPU cache values without changing window epochs or placement.
    /// Returned Scenes may still share their resources; GPU residency belongs to the renderer.
    pub fn clear_caches(&mut self, flags: UpdateResult) {
        if !flags.is_empty() {
            for (_, node) in self.nodes.iter_mut() {
                node.cache.invalidate(flags);
            }
        }
    }

    fn build_new(
        &mut self,
        parent: Option<UiId>,
        view: &dyn AnyView,
        context: &mut Context<'_>,
    ) -> UiId {
        let id = self.nodes.reserve();
        // Explicit two-phase allocation: the parent ID exists while children are built,
        // but a reserved node is not yet observable through get/get_mut.
        let node = self.build_node(parent, id, view, Vec::new(), context);
        self.nodes
            .insert_reserved(id, node)
            .unwrap_or_else(|_| panic!("new node must fill its current reservation"));
        id
    }

    fn build_node(
        &mut self,
        parent: Option<UiId>,
        id: UiId,
        view: &dyn AnyView,
        old_children: Vec<(UiKey, ChildId, UiId)>,
        context: &mut Context<'_>,
    ) -> UiNode {
        let mut context = context.with_cache_epoch(context.cache_epoch());
        let mut ctx = DeclContext {
            context: &mut context,
            current: id,
            old_children,
            queued_widgets: Vec::new(),
            structure_changed: false,
            window: None,
        };
        let label = view.label().map(str::to_owned);
        let key = view.ui_key();
        let widget = view.build(&mut ctx);
        let window = ctx.window;
        let (children, _) = ctx.finish(self, widget.as_ref());
        UiNode {
            label,
            key,
            parent,
            children,
            placement: None,
            cache: NodeCache::default(),
            window,
            widget: Some(widget),
        }
    }

    // Return whether this subtree changed so callers invalidate ancestors on the way back up.
    fn update_node(&mut self, id: UiId, view: &dyn AnyView, context: &mut Context<'_>) -> bool {
        let node = self.node_ref_mut(id).expect("live update node");
        let window = node.window;
        let mut widget = node.widget.take().expect("widget restored after callbacks");
        // A replacement must build in the parent scope, not the old window's domain.
        let old_children = std::mem::take(&mut node.children)
            .into_iter()
            .map(|(child_id, child)| {
                (
                    self.node_ref(child).expect("live child").key,
                    child_id,
                    child,
                )
            })
            .collect();
        let mut scoped = crate::ui_tree::window::scoped_context(window, widget.as_ref(), context);
        let mut ctx = DeclContext {
            context: &mut scoped,
            current: id,
            old_children,
            queued_widgets: Vec::new(),
            structure_changed: false,
            window,
        };
        match widget.try_update(view, &mut ctx) {
            Some(result) => {
                let (children, children_changed) = ctx.finish(self, widget.as_ref());
                let changed = result.contains(UpdateResult::LAYOUT) || children_changed;
                let node = self.node_ref_mut(id).expect("live update node");
                if node.label.as_deref() != view.label() {
                    node.label = view.label().map(str::to_owned);
                }
                node.key = view.ui_key();
                node.children = children;
                node.widget = Some(widget);
                node.cache.invalidate(
                    result
                        | if changed {
                            UpdateResult::LAYOUT
                        } else {
                            UpdateResult::empty()
                        },
                );
                // A native window is a layout boundary: its content cannot affect its parent's size.
                changed && window.is_none()
            }
            None => {
                let old_children = ctx.old_children;
                drop(scoped);
                if let Some(window) = window {
                    window.unregister(context);
                }
                let parent = self.node_ref(id).expect("live replacement node").parent;
                let node = self.build_node(parent, id, view, old_children, context);
                *self.node_ref_mut(id).expect("live replacement node") = node;
                true
            }
        }
    }
    fn remove_reverse_scan_order(&mut self, id: UiId, context: &mut Context<'_>) {
        let Some(node) = self.node_ref_mut(id) else {
            return;
        };
        let children = std::mem::take(&mut node.children);
        for (_, child) in children.into_iter().rev() {
            self.remove_reverse_scan_order(child, context);
        }
        if let Some(window) = self.node_ref(id).and_then(|node| node.window) {
            window.unregister(context);
        }
        self.nodes.remove(id);
    }

    fn node_ref(&self, id: UiId) -> Option<&UiNode> {
        self.nodes.get(id)
    }

    fn node_ref_mut(&mut self, id: UiId) -> Option<&mut UiNode> {
        self.nodes.get_mut(id)
    }
}

impl Default for UiTree {
    fn default() -> Self {
        Self::new()
    }
}

// -------------------
// UI Build and Update
// -------------------

// Matching contract (optimisation contract: violating it never corrupts the tree, it only makes
// state follow the wrong view):
// - Explicit keys must be unique among siblings. Checked by a `debug_assert!` in
//   `DeclContext::child`; in release the first remaining old child with that key wins.
// - Unkeyed siblings of the same type are matched by order of appearance. Reordering them moves
//   their state with the position, not with the view; give them explicit keys to reorder.

/// Borrows child views and application context for one declaration pass (`'a`).
/// The context's window registry has an independent lifetime (`'env`).
pub struct DeclContext<'a, 'env> {
    current: UiId,
    // Children of the previous frame not yet claimed by `DeclContext::child`.
    old_children: Vec<(UiKey, ChildId, UiId)>,
    queued_widgets: Vec<QueuedView<'a>>,
    // New children or a departure from the previous sibling order were declared.
    structure_changed: bool,
    window: Option<crate::ui_tree::window::WindowBinding>,

    // app context
    context: &'a mut Context<'env>,
}

struct QueuedView<'a> {
    // Some: the view is already in the tree, and this is its index
    // None: the view is new, and needs to be built and added to the tree
    existing_index: Option<UiId>,
    key: UiKey,
    view: &'a dyn AnyView,
    child_id: ChildId,
}

impl DeclContext<'_, '_> {
    fn finish(
        self,
        tree: &mut UiTree,
        widget: &dyn Any,
    ) -> (SmallVec<[(ChildId, UiId); CHILDREN_SMALLVEC]>, bool) {
        let Self {
            context,
            old_children,
            queued_widgets,
            structure_changed,
            current,
            window,
        } = self;
        let mut scoped = crate::ui_tree::window::scoped_context(window, widget, context);
        let context = &mut scoped;
        let mut changed = structure_changed || !old_children.is_empty();

        // remove old branch
        for (_, _, id) in old_children.iter().rev() {
            tree.remove_reverse_scan_order(*id, context);
        }

        let mut new_children = SmallVec::new();

        // reorder children and build new branch
        for q in &queued_widgets {
            if let Some(scoped_id) = q.existing_index {
                // update it
                // Do not short-circuit: every declared child must be updated.
                changed |= tree.update_node(scoped_id, q.view, context);
                new_children.push((q.child_id, scoped_id));
            } else {
                // build new
                let scoped_id = tree.build_new(Some(current), q.view, context);
                new_children.push((q.child_id, scoped_id));
            }
        }

        (new_children, changed)
    }
}

/// Handle to a child, issued by `DeclContext::child`.
///
/// Unique across the whole process (all parents, frames and arenas), so a stale id, or one that
/// leaked through the model into another widget, resolves to nothing instead of to the wrong
/// child. Stable across frames for as long as the child is reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChildId {
    uid: u64,
}

impl ChildId {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // Only uniqueness is needed; no other memory is published through this counter.
        Self {
            uid: NEXT
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .expect("child identity exhausted"),
        }
    }
}

pub struct Children<'a> {
    tree: &'a mut UiTree,
    current: UiId,
}

// -----------
// Public APIs
// -----------

impl<'a, 'env> DeclContext<'a, 'env> {
    pub fn context(&mut self) -> &mut Context<'env> {
        self.context
    }

    pub fn child<V: View>(&mut self, view: &'a V) -> ChildId {
        // check if the view is already in the tree
        let key = view.ui_key();

        debug_assert!(
            matches!(key, UiKey::Type(_)) || self.queued_widgets.iter().all(|q| q.key != key),
            "explicit key {key:?} is used by more than one sibling"
        );

        let existing = self
            .old_children
            .iter()
            .position(|(k, _, _)| *k == key)
            .map(|i| {
                // Taking the next old child preserves order; skipping one changes structure
                // (a reorder or deletion), even when all retained ids are stable.
                self.structure_changed |= i != 0;
                self.old_children.remove(i)
            });

        // A reused child keeps its `ChildId`, so ids held across frames stay valid.
        let (existing_index, child_id) = match existing {
            Some((_, child_id, id)) => (Some(id), child_id),
            None => {
                self.structure_changed = true;
                (None, ChildId::new())
            }
        };

        self.queued_widgets.push(QueuedView {
            existing_index,
            key,
            view,
            child_id,
        });

        child_id
    }
}

impl Children<'_> {
    fn get(&mut self, id: ChildId) -> Option<(&mut UiNode, UiId)> {
        let child_ui_id = self.tree.node_ref_mut(self.current)?.child(id)?;
        Some((self.tree.node_ref_mut(child_ui_id)?, child_ui_id))
    }

    /// Test a point in this parent's local coordinates against a child's own shape.
    /// Returns None for an unknown child, missing placement, or unusable local coordinates.
    pub fn is_inside(&mut self, id: ChildId, point: Point, ctx: &mut Context) -> Option<bool> {
        let (node, _) = self.get(id)?;
        let mut scoped = node.scoped_context(ctx);
        let ctx = &mut scoped;
        let placement = node.placement?;
        let point = placement.local_point(point)?;

        Some(
            node.widget
                .as_ref()?
                .is_inside(placement.bounds, point, ctx),
        )
    }
}

#[cfg(test)]
mod tests;
