//! UI ownership, reconciliation, layout, and input. Storage lives in the generic arena module.
use std::any::{Any, TypeId};

use matcha_window::event::device_event::DeviceEvent;
use nalgebra::{Matrix4, Vector4};
use smallvec::SmallVec;

use crate::arena::{Arena, Id as UiId};
use crate::context::Context;
use crate::metrics::{Bounds, Constraints, Point};
use crate::widget::{
    ChildId, Draw, InputPropagation, InputResult, UiError, UpdateResult, View, Widget,
};

mod cache;
mod input;
mod layout;
mod render;

use cache::CacheEpochs;

const CHILDREN_SMALLVEC: usize = 4;

/// Owns a UI tree. Declaration/children contexts expose operations without exposing its arena.
pub struct UiTree {
    nodes: Arena<UiNode>,
    root: Option<UiId>,
    cache_epochs: CacheEpochs,
}

trait AnyView: Any {
    fn label(&self) -> Option<&str>;
    fn ui_key(&self) -> UiKey;
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a>) -> Box<dyn AnyWidget>;
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
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a>) -> Box<dyn AnyWidget> {
        Box::new(View::build(self, ctx))
    }
}

trait AnyWidget: Any {
    fn try_update<'a>(
        &mut self,
        view: &'a dyn AnyView,
        ctx: &mut DeclContext<'a>,
    ) -> Result<UpdateResult, UiInternalError>;
    fn preview_input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: Context) -> InputResult;
    fn input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: Context) -> InputResult;
    fn is_inside(&self, b: Bounds, p: Point, ctx: Context) -> bool;
    fn measure(&self, constraints: Constraints, children: Children<'_>, ctx: Context) -> Bounds;
    fn layout(&self, bounds: Bounds, children: Children<'_>, ctx: Context);
    fn uses_backdrop(&self) -> bool;
    fn render(&self, bounds: Bounds, ctx: Context) -> Draw;
}

impl<W: Widget> AnyWidget for W {
    fn try_update<'a>(
        &mut self,
        view: &'a dyn AnyView,
        ctx: &mut DeclContext<'a>,
    ) -> Result<UpdateResult, UiInternalError> {
        if let Some(view) = (view as &dyn Any).downcast_ref::<W::V>() {
            Ok(Widget::update(self, view, ctx))
        } else {
            Err(UiInternalError::WidgetUpdateFailedTypeMismatch)
        }
    }

    fn preview_input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: Context) -> InputResult {
        Widget::preview_input(self, bounds, event, ctx)
    }
    fn input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: Context) -> InputResult {
        Widget::input(self, bounds, event, ctx)
    }
    fn is_inside(&self, b: Bounds, p: Point, ctx: Context) -> bool {
        Widget::is_inside(self, b, p, ctx)
    }
    fn measure(&self, constraints: Constraints, children: Children<'_>, ctx: Context) -> Bounds {
        Widget::measure(self, constraints, children, ctx)
    }
    fn layout(&self, bounds: Bounds, children: Children<'_>, ctx: Context) {
        Widget::layout(self, bounds, children, ctx)
    }
    fn render(&self, bounds: Bounds, ctx: Context) -> Draw {
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

    // layout caches
    measure_cache: Option<(Constraints, Bounds)>,
    // Bounds for which the children's layout has been computed. None requires recomputation.
    layout_cache: Option<Bounds>,

    // draw cache
    draw_cache: Option<Draw>,
    // Environment epochs last observed by each independent cache group.
    cache_epochs: CacheEpochs,

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

impl UiNode {
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
            cache_epochs: CacheEpochs::default(),
        }
    }

    /// Reconcile the declared view tree and invalidate changed nodes and their ancestors.
    pub fn update<V: View>(&mut self, view: &V) {
        if let Some(id) = self.root {
            self.update_node(id, view);
        } else {
            // Clear storage explicitly before starting a fresh tree, including abandoned reservations.
            self.nodes.clear();
            let id = self.build_new(None, view);
            self.root = Some(id);
        }
    }

    /// Remove the tree and any outstanding reservations while retaining tree capacity.
    pub fn clear(&mut self) {
        if let Some(root) = self.root.take() {
            self.remove_reverse_scan_order(root);
        }
        self.nodes.clear();
    }

    fn build_new(&mut self, parent: Option<UiId>, view: &dyn AnyView) -> UiId {
        let id = self.nodes.reserve();
        // Explicit two-phase allocation: the parent ID exists while children are built,
        // but a reserved node is not yet observable through get/get_mut.
        let node = self.build_node(parent, id, view, None);
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
        child_candidate: Option<Vec<(UiKey, ChildId, UiId)>>,
    ) -> UiNode {
        let current_id = id;

        let mut ctx = DeclContext {
            old_children: child_candidate.unwrap_or_default(),
            queued_widgets: Vec::new(),
            structure_changed: false,
        };

        let label = view.label().map(|s| s.to_string());
        let key = view.ui_key();
        let widget = view.build(&mut ctx);
        let (children, _) = ctx.finish(self, current_id);

        UiNode {
            label,
            key,
            parent,
            children,
            placement: None,
            measure_cache: None,
            layout_cache: None,
            draw_cache: None,
            cache_epochs: self.cache_epochs,
            widget: Some(widget),
        }
    }

    // Return whether this subtree changed so callers invalidate ancestors on the way back up.
    fn update_node(&mut self, id: UiId, view: &dyn AnyView) -> bool {
        let node = self
            .node_ref_mut(id)
            .expect("`update_node` is only called with ids taken from the live tree.");

        let mut ctx = DeclContext {
            old_children: std::mem::take(&mut node.children)
                .into_iter()
                .map(|(child_id, id)| {
                    let key = self
                        .node_ref(id)
                        .expect("A node's children are freed only together with the node itself.")
                        .key;
                    (key, child_id, id)
                })
                .collect::<Vec<_>>(),
            queued_widgets: Vec::new(),
            structure_changed: false,
        };

        match self
            .node_ref_mut(id)
            .unwrap()
            .widget
            .as_mut()
            .expect("there is a bug in `Option` management of `widget` in `UiNode`.")
            .try_update(view, &mut ctx)
        {
            Ok(result) => {
                let (children, children_changed) = ctx.finish(self, id);
                let changed = result.contains(UpdateResult::LAYOUT) || children_changed;
                let key = view.ui_key();
                // re-get the node after finish to avoid borrow checker issues
                let node = self.node_ref_mut(id).unwrap();
                // compare first: this runs for every node on every frame, and labels rarely change
                let label = view.label();
                if node.label.as_deref() != label {
                    node.label = label.map(str::to_owned);
                }
                node.key = key;
                node.children = children;
                if result.contains(UpdateResult::RENDER) {
                    node.draw_cache = None;
                }
                if changed {
                    node.measure_cache = None;
                    node.layout_cache = None;
                }
                changed
            }
            Err(UiInternalError::WidgetUpdateFailedTypeMismatch) => {
                let parent = self.node_ref_mut(id).unwrap().parent;
                // rebuild
                let node = self.build_node(parent, id, view, Some(ctx.old_children));
                let node_mut = self.node_ref_mut(id).unwrap();
                *node_mut = node;
                true
            }
        }
    }

    fn remove_reverse_scan_order(&mut self, id: UiId) {
        let Some(node) = self.node_ref_mut(id) else {
            return;
        };
        let children = std::mem::take(&mut node.children);
        for (_, child) in children.into_iter().rev() {
            self.remove_reverse_scan_order(child);
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

pub struct DeclContext<'a> {
    // Children of the previous frame not yet claimed by `DeclContext::child`.
    old_children: Vec<(UiKey, ChildId, UiId)>,
    queued_widgets: Vec<QueuedView<'a>>,
    // New children or a departure from the previous sibling order were declared.
    structure_changed: bool,
}

struct QueuedView<'a> {
    // Some: the view is already in the tree, and this is its index
    // None: the view is new, and needs to be built and added to the tree
    existing_index: Option<UiId>,
    key: UiKey,
    view: &'a dyn AnyView,
    child_id: ChildId,
}

impl DeclContext<'_> {
    fn finish(
        self,
        tree: &mut UiTree,
        current_id: UiId,
    ) -> (SmallVec<[(ChildId, UiId); CHILDREN_SMALLVEC]>, bool) {
        let Self {
            old_children,
            queued_widgets,
            structure_changed,
        } = self;
        let mut changed = structure_changed || !old_children.is_empty();

        // remove old branch
        for (_, _, id) in old_children.iter().rev() {
            tree.remove_reverse_scan_order(*id);
        }

        let mut new_children = SmallVec::new();

        // reorder children and build new branch
        for q in &queued_widgets {
            if let Some(scoped_id) = q.existing_index {
                // update it
                // Do not short-circuit: every declared child must be updated.
                changed |= tree.update_node(scoped_id, q.view);
                new_children.push((q.child_id, scoped_id));
            } else {
                // build new
                let scoped_id = tree.build_new(Some(current_id), q.view);
                new_children.push((q.child_id, scoped_id));
            }
        }

        (new_children, changed)
    }
}

pub struct Children<'a> {
    tree: &'a mut UiTree,
    current: UiId,
}

// --------------
// Internal error
// --------------

pub(crate) enum UiInternalError {
    // AnyWidget::try_update
    WidgetUpdateFailedTypeMismatch,
}

// -----------
// Public APIs
// -----------

impl<'a> DeclContext<'a> {
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
    /// Returns None for an unknown child, missing placement, or a non-invertible relative affine.
    pub fn is_inside(&mut self, id: ChildId, point: Point, ctx: Context) -> Option<bool> {
        let (node, _) = self.get(id)?;
        let placement = node.placement?;
        let local_point = placement.affine_inverse? * Vector4::new(point.x, point.y, point.z, 1.0);
        let point = Point {
            x: local_point.x,
            y: local_point.y,
            z: local_point.z,
        };

        Some(
            node.widget
                .as_ref()?
                .is_inside(placement.bounds, point, ctx),
        )
    }
}

#[cfg(test)]
mod tests;
