//! UI ownership, reconciliation, layout, and input. Storage lives in the generic arena module.
use std::any::{Any, TypeId};

use matcha_window::event::device_event::DeviceEvent;
use matcha_window::event::window_event::WindowEvent;
use matcha_window::window::{WindowError, WindowId};
use nalgebra::{Matrix4, Vector4};

use self::context::Context;
use self::widget::{
    Draw, InputPropagation, InputResult, PollResult, UiError, UpdateResult, View, Widget,
};
use crate::arena::{Arena, Id as UiId};
use crate::metrics::{Bounds, Constraints, Point};

mod cache;
pub mod context;
mod env_reads;
mod input;
mod layout;
mod poll;
mod render;
pub mod widget;
pub mod window;

use cache::Cache;
use env_reads::{EnvReads, NodeEnvReads, ReadPhase};

trait AnyView: Any {
    fn label(&self) -> Option<&str>;
    fn ui_key(&self) -> UiKey;
    fn build_node(
        &self,
        tree: &mut UiTree,
        parent: Option<UiId>,
        id: UiId,
        old_children: Vec<(UiKey, UiId)>,
        context: &mut Context,
    ) -> UiNode;
}

impl<V: View> AnyView for V {
    fn label(&self) -> Option<&str> {
        View::label(self)
    }

    fn ui_key(&self) -> UiKey {
        match View::key(self) {
            Some(key) => UiKey::Key(key),
            None => UiKey::Type(TypeId::of::<V>()),
        }
    }

    fn build_node(
        &self,
        tree: &mut UiTree,
        parent: Option<UiId>,
        id: UiId,
        old_children: Vec<(UiKey, UiId)>,
        context: &mut Context,
    ) -> UiNode {
        let label = View::label(self).map(str::to_owned);
        let key = self.ui_key();
        let mut declaration = Declaration::new(id, old_children);
        let (body, reads) = context.observe_env_reads(|context| {
            let widget = self.build(&mut declaration.context(context));
            let mut children = Vec::with_capacity(declaration.queued.len());
            declaration.finish(tree, context, &mut children);
            Mounted { widget, children }.into_body()
        });
        let mut env_reads = NodeEnvReads::default();
        env_reads.record(ReadPhase::Build, reads);
        UiNode {
            label,
            key,
            parent,
            placement: None,
            measure_cache: Cache::default(),
            layout_cache: Cache::default(),
            render_cache: Cache::default(),
            env_reads,
            body: Some(body),
        }
    }
}

/// The arena erases the widget and its typed child links together.
struct Mounted<W: Widget> {
    widget: W,
    children: Vec<ChildLink<<W::V as View>::ChildData>>,
}

impl<W: Widget> Mounted<W> {
    fn into_body(self) -> NodeBody {
        (W::mount_kind().wrap)(self)
    }
}

/// Selects how the framework stores a widget together with its reconciled children.
/// Widget implementations inherit ordinary widget storage by default.
pub struct MountKind<W: Widget> {
    wrap: fn(Mounted<W>) -> NodeBody,
}

impl<W: Widget> MountKind<W> {
    fn widget() -> Self {
        Self {
            wrap: |mounted| NodeBody::Widget(Box::new(mounted)),
        }
    }
}

enum NodeBody {
    Widget(Box<dyn AnyWidget>),
    Window(Box<dyn AnyWindow>),
}

impl NodeBody {
    fn as_widget(&self) -> &dyn AnyWidget {
        match self {
            Self::Widget(widget) => widget.as_ref(),
            Self::Window(window) => window.as_ref(),
        }
    }

    fn as_widget_mut(&mut self) -> &mut dyn AnyWidget {
        match self {
            Self::Widget(widget) => widget.as_mut(),
            Self::Window(window) => window.as_mut(),
        }
    }

    fn is_window(&self) -> bool {
        matches!(self, Self::Window(_))
    }
}

trait AnyWindow: AnyWidget {
    fn window_event(&self, event: &WindowEvent) -> UpdateResult;
}

trait AnyWidget: utils::MaybeSendSync {
    fn child_count(&self) -> usize;
    fn child_id(&self, index: usize) -> UiId;
    fn try_update(
        &mut self,
        view: &dyn AnyView,
        tree: &mut UiTree,
        id: UiId,
        context: &mut Context,
    ) -> Option<(UpdateResult, EnvReads)>;
    fn preview_input(
        &mut self,
        bounds: Bounds,
        event: &DeviceEvent,
        ctx: &mut Context,
    ) -> InputResult;
    fn input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: &mut Context) -> InputResult;
    fn poll(&mut self, ctx: &mut Context) -> PollResult;
    fn is_inside(&self, b: Bounds, p: Point, ctx: &mut Context) -> bool;
    fn measure(&self, constraints: Constraints, tree: &mut UiTree, ctx: &mut Context) -> Bounds;
    fn layout(&self, bounds: Bounds, tree: &mut UiTree, ctx: &mut Context);
    fn uses_backdrop(&self) -> bool;
    fn render(&self, bounds: Bounds, ctx: &mut Context) -> Draw;
}

impl dyn AnyWidget + '_ {
    fn child_ids(&self) -> impl ExactSizeIterator<Item = UiId> + DoubleEndedIterator {
        (0..self.child_count()).map(|index| self.child_id(index))
    }
}

impl<W: Widget> AnyWidget for Mounted<W> {
    fn child_count(&self) -> usize {
        self.children.len()
    }

    fn child_id(&self, index: usize) -> UiId {
        self.children[index].node
    }

    fn try_update(
        &mut self,
        view: &dyn AnyView,
        tree: &mut UiTree,
        id: UiId,
        context: &mut Context,
    ) -> Option<(UpdateResult, EnvReads)> {
        let view = (view as &dyn Any).downcast_ref::<W::V>()?;
        let old_children = tree.child_candidates(self);
        let mut declaration = Declaration::new(id, old_children);
        let (mut result, reads) = context
            .observe_env_reads(|ctx| self.widget.update(view, &mut declaration.context(ctx)));
        if declaration.finish(tree, context, &mut self.children) {
            result |= UpdateResult::LAYOUT;
        }
        Some((result, reads))
    }

    fn preview_input(
        &mut self,
        bounds: Bounds,
        event: &DeviceEvent,
        ctx: &mut Context,
    ) -> InputResult {
        self.widget.preview_input(bounds, event, ctx)
    }

    fn input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: &mut Context) -> InputResult {
        self.widget.input(bounds, event, ctx)
    }

    fn poll(&mut self, ctx: &mut Context) -> PollResult {
        self.widget.poll(ctx)
    }

    fn is_inside(&self, b: Bounds, p: Point, ctx: &mut Context) -> bool {
        self.widget.is_inside(b, p, ctx)
    }

    fn measure(&self, constraints: Constraints, tree: &mut UiTree, ctx: &mut Context) -> Bounds {
        self.widget.measure(
            constraints,
            Children {
                links: &self.children,
                tree,
            },
            ctx,
        )
    }

    fn layout(&self, bounds: Bounds, tree: &mut UiTree, ctx: &mut Context) {
        self.widget.layout(
            bounds,
            Children {
                links: &self.children,
                tree,
            },
            ctx,
        )
    }

    fn render(&self, bounds: Bounds, ctx: &mut Context) -> Draw {
        self.widget.render(bounds, ctx)
    }

    fn uses_backdrop(&self) -> bool {
        self.widget.uses_backdrop()
    }
}

/// Owns a UI tree. Declaration/children contexts expose operations without exposing its arena.
/// Widget callbacks that panic can leave reconciliation incomplete. Discard the tree and its
/// window registry after catching such a panic; callbacks are not a transactional boundary.
pub struct UiTree {
    nodes: Arena<UiNode>,
    root: Option<UiId>,
}

/// Identity of a node among its siblings, used to match a new view against the old children.
///
/// - `Key`: the view returned an explicit key. Matched by key alone, regardless of type; if the
///   type changed, the node is rebuilt in place and keeps its `UiId` and the
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

    // Current placement assigned by the parent, independent of layout cache validity.
    placement: Option<Placement>,

    // cache
    measure_cache: Cache<Constraints, Bounds>,
    layout_cache: Cache<Bounds, ()>,
    render_cache: Cache<(), Draw>,
    env_reads: NodeEnvReads,

    // Taken during recursive callbacks to borrow the child links independently of the arena.
    body: Option<NodeBody>,
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
    fn body(&self) -> &NodeBody {
        self.body.as_ref().expect("live node has a mounted body")
    }

    fn invalidate_caches(&mut self, flags: UpdateResult) {
        if flags.contains(UpdateResult::LAYOUT) {
            self.measure_cache.invalidate();
            self.layout_cache.invalidate();
        }
        if flags.contains(UpdateResult::RENDER) {
            self.render_cache.invalidate();
        }
    }

    fn child_ids(&self) -> impl ExactSizeIterator<Item = UiId> + DoubleEndedIterator {
        self.body().as_widget().child_ids()
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
        if let Err(error) = self.try_update(view, context) {
            log::error!("Opening declared window: {error}");
        }
    }

    /// Reconcile all declarations and report the first window that failed to open.
    /// Successfully mounted siblings are retained; a subsequent update retries
    /// any failed window using the config supplied in that update.
    pub fn try_update<V: View>(
        &mut self,
        view: &V,
        context: &mut Context<'_>,
    ) -> Result<(), WindowError> {
        context.capture_window_errors(|context| {
            if let Some(id) = self.root {
                self.update_node(id, view, context);
            } else {
                self.nodes.clear();
                self.root = Some(self.build_new(None, view, context));
            }
        })
    }

    /// Remove nodes and unregister their windows using borrowed application services.
    pub fn clear(&mut self, context: &mut Context<'_>) {
        if let Some(root) = self.root.take() {
            self.remove_reverse_scan_order(root, context);
        }
        self.nodes.clear();
    }

    /// Immediately release selected CPU cache values without changing placement.
    /// Returned Scenes may still share their resources; GPU residency belongs to the renderer.
    pub fn clear_caches(&mut self, flags: UpdateResult) {
        if !flags.is_empty() {
            for (_, node) in self.nodes.iter_mut() {
                node.invalidate_caches(flags);
            }
        }
    }

    /// Apply a mounted widget's update outside reconciliation. Drawing is local;
    /// measurement and layout propagate to the nearest native window or the root.
    fn invalidate_node(&mut self, id: UiId, mut flags: UpdateResult) {
        let mut current = Some(id);
        while let Some(id) = current {
            let node = self.node_ref_mut(id).expect("live invalidation target");
            node.invalidate_caches(flags);
            if !flags.contains(UpdateResult::LAYOUT) || node.body().is_window() {
                break;
            }
            current = node.parent;
            flags = UpdateResult::LAYOUT;
        }
    }

    /// Invalidate one environment scope, stopping at nested native windows.
    fn invalidate_caches_from(&mut self, id: UiId, flags: UpdateResult) {
        if flags.is_empty() {
            return;
        }
        let node = self.node_ref_mut(id).expect("live invalidation root");
        node.invalidate_caches(flags);
        let child_count = node.child_ids().len();
        for index in 0..child_count {
            let child = self
                .node_ref(id)
                .expect("live invalidation root")
                .child_ids()
                .nth(index)
                .expect("live child");
            if !self.node_ref(child).expect("live child").body().is_window() {
                self.invalidate_caches_from(child, flags);
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
        let node = view.build_node(self, parent, id, Vec::new(), context);
        self.nodes
            .insert_reserved(id, node)
            .unwrap_or_else(|_| panic!("new node must fill its current reservation"));
        id
    }

    fn child_candidates(&self, widget: &dyn AnyWidget) -> Vec<(UiKey, UiId)> {
        // Matching normally consumes siblings in declaration order. Keep the next
        // candidate at the end so claiming an unchanged child takes constant time.
        widget
            .child_ids()
            .rev()
            .map(|id| (self.node_ref(id).expect("live child").key, id))
            .collect()
    }

    // Return whether this subtree changed so callers invalidate ancestors on the way back up.
    fn update_node(&mut self, id: UiId, view: &dyn AnyView, context: &mut Context<'_>) -> bool {
        let node = self.node_ref_mut(id).expect("live update node");
        let mut body = node.body.take().expect("body restored after callbacks");
        let was_window = body.is_window();
        if let Some((result, reads)) = body.as_widget_mut().try_update(view, self, id, context) {
            let node = self.node_ref_mut(id).expect("live update node");
            node.env_reads.record(ReadPhase::Update, reads);
            if node.label.as_deref() != view.label() {
                node.label = view.label().map(str::to_owned);
            }
            node.key = view.ui_key();
            node.body = Some(body);
            node.invalidate_caches(result);
            // A native window's content cannot affect its parent's size.
            result.contains(UpdateResult::LAYOUT) && !was_window
        } else {
            if was_window {
                context.window_registry().remove_node(id);
            }
            // A replacement takes over child identities and declares its own typed properties.
            let old_children = self.child_candidates(body.as_widget());
            let parent = self.node_ref(id).expect("live replacement node").parent;
            let node = view.build_node(self, parent, id, old_children, context);
            let environment_changed = was_window || node.body().is_window();
            *self.node_ref_mut(id).expect("live replacement node") = node;
            if environment_changed {
                self.invalidate_caches_from(id, UpdateResult::LAYOUT | UpdateResult::RENDER);
            }
            true
        }
    }

    fn remove_reverse_scan_order(&mut self, id: UiId, context: &mut Context<'_>) {
        let Some(node) = self.nodes.remove(id) else {
            return;
        };
        for child in node.child_ids().rev() {
            self.remove_reverse_scan_order(child, context);
        }
        if node.body().is_window() {
            context.window_registry().remove_node(id);
        }
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
//   `DeclContext::child_with`; in release the first remaining old child with that key wins.
// - Unkeyed siblings of the same type are matched by order of appearance. Reordering them moves
//   their state with the position, not with the view; give them explicit keys to reorder.

/// Declared views live for `'a`, independently of the callback's service borrow (`'env`).
/// Only declaration data survives the callback; its observed Context cannot escape.
pub struct DeclContext<'a, 'env, P = ()> {
    declaration: &'env mut Declaration<'a, P>,
    context: Context<'env>,
}

/// Context-free reconciliation data, retained after the observed callback returns.
struct Declaration<'a, P> {
    current: UiId,
    old_children: Vec<(UiKey, UiId)>,
    queued: Vec<QueuedView<'a, P>>,
    // Only used while reconciling this declaration's children into their native window.
    child_window: Option<WindowId>,
}

struct QueuedView<'a, P> {
    key: UiKey,
    existing: Option<UiId>,
    view: &'a dyn AnyView,
    data: P,
}

#[derive(PartialEq)]
struct ChildLink<P> {
    node: UiId,
    data: P,
}

impl<'a, P> Declaration<'a, P> {
    fn new(current: UiId, old_children: Vec<(UiKey, UiId)>) -> Self {
        Self {
            current,
            old_children,
            queued: Vec::new(),
            child_window: None,
        }
    }

    fn context<'env>(&'env mut self, context: &'env mut Context) -> DeclContext<'a, 'env, P> {
        DeclContext {
            declaration: self,
            context: context.reborrow(),
        }
    }

    /// Reconcile into the parent's existing link buffer. The result includes both link changes
    /// and layout changes reported by retained descendants.
    fn finish(self, tree: &mut UiTree, context: &mut Context, links: &mut Vec<ChildLink<P>>) -> bool
    where
        P: PartialEq,
    {
        let mut scoped = context.unobserved();
        if let Some(id) = self.child_window {
            scoped = scoped.with_window(id);
        }
        for (_, id) in self.old_children {
            tree.remove_reverse_scan_order(id, &mut scoped);
        }

        let mut changed = links.len() != self.queued.len();
        links.truncate(self.queued.len());
        for (index, child) in self.queued.into_iter().enumerate() {
            let node = match child.existing {
                Some(id) => {
                    changed |= tree.update_node(id, child.view, &mut scoped);
                    id
                }
                None => tree.build_new(Some(self.current), child.view, &mut scoped),
            };
            let link = ChildLink {
                node,
                data: child.data,
            };
            if let Some(old) = links.get_mut(index) {
                changed = changed || *old != link;
                *old = link;
            } else {
                links.push(link);
            }
        }
        changed
    }
}

impl<'a, 'env, P> DeclContext<'a, 'env, P> {
    pub fn context(&mut self) -> &mut Context<'env> {
        &mut self.context
    }

    /// Declare a child and the properties this parent uses for its measurement and layout.
    /// A change to the ordered child links invalidates this parent's measure and layout caches.
    pub fn child_with<V: View>(&mut self, view: &'a V, data: P) {
        let key = view.ui_key();
        let declaration = &mut *self.declaration;
        debug_assert!(
            matches!(key, UiKey::Type(_)) || declaration.queued.iter().all(|q| q.key != key),
            "explicit key {key:?} is used by more than one sibling"
        );
        let existing = declaration
            .old_children
            .iter()
            .rposition(|(k, _)| *k == key)
            .map(|index| declaration.old_children.remove(index).1);
        declaration.queued.push(QueuedView {
            key,
            existing,
            view,
            data,
        });
    }
}

impl<'a> DeclContext<'a, '_> {
    /// Declare a child of a parent whose child properties are unit.
    pub fn child<V: View>(&mut self, view: &'a V) {
        self.child_with(view, ());
    }
}

/// This parent's children in declaration order. Each visit borrows one link for the call.
pub struct Children<'a, P = ()> {
    links: &'a [ChildLink<P>],
    tree: &'a mut UiTree,
}

impl<P> Children<'_, P> {
    pub fn len(&self) -> usize {
        self.links.len()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// The order is stable across multiple passes within a widget callback. A child handle
    /// can only be used during its visit.
    ///
    /// ```compile_fail,E0521
    /// use matcha_better_tree::ui_tree::{Child, Children};
    ///
    /// fn save_child<'a>(mut children: Children<'a>) -> Option<Child<'a>> {
    ///     let mut saved = None;
    ///     children.for_each(|_, child| saved = Some(child));
    ///     saved
    /// }
    /// ```
    pub fn for_each(&mut self, mut visit: impl for<'child> FnMut(&'child P, Child<'child>)) {
        for link in self.links {
            visit(
                &link.data,
                Child {
                    tree: self.tree,
                    id: link.node,
                },
            );
        }
    }
}

/// A child's operations for the duration of a visit through [`Children::for_each`].
pub struct Child<'a> {
    tree: &'a mut UiTree,
    id: UiId,
}

impl Child<'_> {
    /// Test the child's own shape in parent-local coordinates. Returns `None` when
    /// the child has no placement or the point cannot be mapped into its coordinates.
    pub fn is_inside(&mut self, point: Point, ctx: &mut Context) -> Option<bool> {
        let node = self.tree.node_ref(self.id).expect("live child");
        let placement = node.placement?;
        let point = placement.local_point(point)?;
        let mut scoped = ctx.unobserved();
        Some(
            node.body()
                .as_widget()
                .is_inside(placement.bounds, point, &mut scoped),
        )
    }
}

#[cfg(test)]
mod tests;
