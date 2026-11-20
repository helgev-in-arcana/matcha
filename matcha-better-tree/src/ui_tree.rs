//! UI ownership, reconciliation, layout, and input. Storage lives in the generic arena module.
use std::any::{Any, TypeId};
use std::collections::HashMap;

use matcha_window::event::device_event::DeviceEvent;
use matcha_window::window::{WindowError, WindowId};
use nalgebra::{Matrix4, Vector4};

use self::context::Context;
use self::widget::{
    Draw, InputPropagation, InputResult, Lifecycle, PollInfo, UiError, UpdateResult, View, Widget,
    WidgetPhase,
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
pub use poll::PollSummary;

trait AnyView: Any {
    fn label(&self) -> Option<&str>;
    fn ui_key(&self) -> UiKey;
    fn build_node(
        &self,
        tree: &mut UiTree,
        parent: Option<UiId>,
        id: UiId,
        old_children: Vec<Candidate>,
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
        old_children: Vec<Candidate>,
        context: &mut Context,
    ) -> UiNode {
        let label = View::label(self).map(str::to_owned);
        let key = self.ui_key();
        let mut declaration = Declaration::new(id, old_children, Some(WidgetPhase::Active));
        let ((body, phase), reads) = context.observe_env_reads(|context| {
            let widget = self.build(&mut declaration.context(context));
            let phase = declaration
                .initial_phase
                .expect("build supplies initial phase");
            let mut children = Vec::new();
            declaration.finish(tree, context, &mut children);
            (Mounted { widget, children }.into_body(), phase)
        });
        let mut env_reads = NodeEnvReads::default();
        env_reads.record(ReadPhase::Build, reads);
        UiNode {
            label,
            key,
            parent,
            phase,
            active_children_changed: false,
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
    fn close_requested(&self);
}

trait AnyWidget: utils::MaybeSendSync {
    fn child_count(&self) -> usize;
    fn child_id(&self, index: usize) -> UiId;
    fn retain_children(&mut self, keep: &mut dyn FnMut(UiId) -> bool);
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
    fn poll(&mut self, info: PollInfo, ctx: &mut Context) -> Lifecycle;
    fn is_inside(&self, b: Bounds, p: Point, ctx: &mut Context) -> bool;
    fn measure(
        &self,
        constraints: Constraints,
        tree: &mut UiTree,
        ctx: &mut Context,
    ) -> (Bounds, ChildGeometry);
    fn layout(&self, bounds: Bounds, tree: &mut UiTree, ctx: &mut Context) -> EnvReads;
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

    fn retain_children(&mut self, keep: &mut dyn FnMut(UiId) -> bool) {
        self.children.retain(|link| keep(link.node));
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
        let mut declaration = Declaration::new(id, old_children, None);
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

    fn poll(&mut self, info: PollInfo, ctx: &mut Context) -> Lifecycle {
        self.widget.poll(info, ctx)
    }

    fn is_inside(&self, b: Bounds, p: Point, ctx: &mut Context) -> bool {
        self.widget.is_inside(b, p, ctx)
    }

    fn measure(
        &self,
        constraints: Constraints,
        tree: &mut UiTree,
        ctx: &mut Context,
    ) -> (Bounds, ChildGeometry) {
        let mut geometry = ChildGeometry::default();
        let bounds = self.widget.measure(
            constraints,
            Children {
                links: &self.children,
                tree,
                geometry: &mut geometry,
            },
            ctx,
        );
        (bounds, geometry)
    }

    fn layout(&self, bounds: Bounds, tree: &mut UiTree, ctx: &mut Context) -> EnvReads {
        let mut geometry = ChildGeometry::default();
        self.widget.layout(
            bounds,
            Children {
                links: &self.children,
                tree,
                geometry: &mut geometry,
            },
            ctx,
        );
        geometry.dependencies
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
    phase: WidgetPhase,
    active_children_changed: bool,

    // Current placement assigned by the parent, independent of layout cache validity.
    placement: Option<Placement>,

    // cache
    measure_cache: Cache<(Constraints, context::GeometryEnvironment), (Bounds, EnvReads)>,
    layout_cache: Cache<(Bounds, context::GeometryEnvironment), EnvReads>,
    render_cache: Cache<(), (Draw, EnvReads)>,
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

bitflags::bitflags! {
    struct InvalidatedCaches: u8 {
        const MEASURE = 1 << 0;
        const LAYOUT = 1 << 1;
        const RENDER = 1 << 2;
    }
}

impl InvalidatedCaches {
    fn update_result(self) -> UpdateResult {
        let mut update = UpdateResult::empty();
        update.set(
            UpdateResult::LAYOUT,
            self.intersects(Self::MEASURE | Self::LAYOUT),
        );
        update.set(UpdateResult::RENDER, self.contains(Self::RENDER));
        update
    }
}

impl UiNode {
    /// Each entry records the environment used to compute that particular result.
    fn invalidate_env(&mut self, changed: EnvReads, affected: &mut InvalidatedCaches) {
        if changed.is_empty() {
            return;
        }
        if self
            .measure_cache
            .invalidate_if(|(_, reads)| reads.intersects(changed))
        {
            affected.insert(InvalidatedCaches::MEASURE);
        }
        // A child's current placement can survive a more recent parent layout that did not
        // place it. Reopen the layout path even when that parent observed no dependency.
        if self.layout_cache.invalidate_if(|reads| {
            affected.contains(InvalidatedCaches::LAYOUT) || reads.intersects(changed)
        }) {
            affected.insert(InvalidatedCaches::LAYOUT);
        }
        if self
            .render_cache
            .invalidate_if(|(_, reads)| reads.intersects(changed))
        {
            affected.insert(InvalidatedCaches::RENDER);
        }
    }

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
    /// Call [`Self::poll`] (or [`Self::poll_window`] for window content) before the
    /// next layout, render or input pass to resolve widget entry and exit. Several
    /// updates may precede one poll: an exiting keyed child can still be reclaimed.
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
        if !flags.is_empty() {
            self.node_ref_mut(id)
                .expect("live invalidation root")
                .invalidate_caches(flags);
            self.for_each_child_in_scope(id, |tree, child| {
                tree.invalidate_caches_from(child, flags)
            });
        }
    }

    /// Descend within this window without holding a borrow across a recursive call.
    fn for_each_child_in_scope(&mut self, id: UiId, mut visit: impl FnMut(&mut Self, UiId)) {
        let child_count = self
            .node_ref(id)
            .expect("live invalidation root")
            .child_ids()
            .len();
        for index in 0..child_count {
            let child = self
                .node_ref(id)
                .expect("live invalidation root")
                .child_ids()
                .nth(index)
                .expect("live child");
            if !self.node_ref(child).expect("live child").body().is_window() {
                visit(self, child);
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

    fn child_candidates(&self, widget: &dyn AnyWidget) -> Vec<Candidate> {
        // Matching normally consumes siblings in declaration order. Keep the next
        // candidate at the end so claiming an unchanged child takes constant time.
        widget
            .child_ids()
            .enumerate()
            .rev()
            .map(|(index, id)| {
                let node = self.node_ref(id).expect("live child");
                Candidate {
                    node: id,
                    key: node.key,
                    index,
                    exiting: node.phase == WidgetPhase::Exiting,
                }
            })
            .collect()
    }

    // Return whether this subtree changed so callers invalidate ancestors on the way back up.
    fn update_node(
        &mut self,
        id: UiId,
        view: &dyn AnyView,
        context: &mut Context<'_>,
    ) -> Reconciled {
        let node = self.node_ref_mut(id).expect("live update node");
        let was_exiting = node.phase == WidgetPhase::Exiting;
        let mut body = node.body.take().expect("body restored after callbacks");
        let was_window = body.is_window();
        if let Some((result, reads)) = body.as_widget_mut().try_update(view, self, id, context) {
            let node = self.node_ref_mut(id).expect("live update node");
            // A Window -> ordinary host replacement may preserve its children's identities.
            // Outside a poll driver there is no callback that could finish a pending entry.
            if was_exiting || !context.can_poll_widgets() {
                node.phase = WidgetPhase::Active;
            }
            node.env_reads.record(ReadPhase::Update, reads);
            if node.label.as_deref() != view.label() {
                node.label = view.label().map(str::to_owned);
            }
            node.key = view.ui_key();
            node.body = Some(body);
            node.invalidate_caches(result);
            // A native window's content cannot affect its parent's size.
            Reconciled {
                layout_changed: result.contains(UpdateResult::LAYOUT) && !was_window,
                replaced: false,
            }
        } else {
            if was_window {
                context.window_registry().remove_node(id);
            }
            // A replacement takes over child identities and declares its own typed properties.
            let mut old_children = self.child_candidates(body.as_widget());
            // The new parent cannot own the old parent's typed links for retiring children.
            // A replaced retiree itself is never revived by reusing one of its descendants.
            old_children.retain(|child| {
                if was_exiting || child.exiting {
                    self.remove_reverse_scan_order(child.node, context);
                    false
                } else {
                    true
                }
            });
            let parent = self.node_ref(id).expect("live replacement node").parent;
            let node = view.build_node(self, parent, id, old_children, context);
            let environment_changed = was_window || node.body().is_window();
            *self.node_ref_mut(id).expect("live replacement node") = node;
            if environment_changed {
                self.invalidate_caches_from(id, UpdateResult::LAYOUT | UpdateResult::RENDER);
            }
            Reconciled {
                layout_changed: true,
                replaced: true,
            }
        }
    }

    // A native window has its own input, polling, and drawing lifetime. Close it when
    // its declaration disappears, even if an ordinary ancestor stays for its exit.
    fn remove_nested_windows(&mut self, id: UiId, ctx: &mut Context) -> bool {
        let mut body = self
            .node_ref_mut(id)
            .expect("live retiree")
            .body
            .take()
            .expect("mounted");
        let mut child_removed = false;
        let mut layout_changed = false;
        body.as_widget_mut().retain_children(&mut |child| {
            if self.node_ref(child).expect("live child").body().is_window() {
                self.remove_reverse_scan_order(child, ctx);
                child_removed = true;
                false
            } else {
                layout_changed |= self.remove_nested_windows(child, ctx);
                true
            }
        });
        let node = self.node_ref_mut(id).expect("live retiree");
        node.body = Some(body);
        node.active_children_changed |= child_removed;
        if child_removed || layout_changed {
            node.invalidate_caches(UpdateResult::LAYOUT);
            return node.phase != WidgetPhase::PendingEnter;
        }
        false
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

    fn participates(&self, id: UiId) -> bool {
        self.node_ref(id).expect("live child link").phase != WidgetPhase::PendingEnter
    }

    fn participating_ids<'a, P>(
        &'a self,
        links: &'a [ChildLink<P>],
    ) -> impl Iterator<Item = UiId> + 'a {
        links
            .iter()
            .map(|link| link.node)
            .filter(|&id| self.participates(id))
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
    old_children: Vec<Candidate>,
    queued: Vec<QueuedView<'a, P>>,
    // Only build may choose how its node first participates.
    initial_phase: Option<WidgetPhase>,
    // Only used while reconciling this declaration's children into their native window.
    child_scope: ChildScope,
}

enum ChildScope {
    Inherited,
    Native(WindowId),
    UnmountedWindow,
}

struct Candidate {
    key: UiKey,
    node: UiId,
    index: usize,
    exiting: bool,
}

struct Reconciled {
    layout_changed: bool,
    replaced: bool,
}

struct QueuedView<'a, P> {
    key: UiKey,
    existing: Option<Candidate>,
    view: &'a dyn AnyView,
    data: P,
}

#[derive(PartialEq)]
struct ChildLink<P> {
    node: UiId,
    data: P,
}

impl<'a, P> Declaration<'a, P> {
    fn new(
        current: UiId,
        old_children: Vec<Candidate>,
        initial_phase: Option<WidgetPhase>,
    ) -> Self {
        Self {
            current,
            old_children,
            queued: Vec::new(),
            initial_phase,
            child_scope: ChildScope::Inherited,
        }
    }

    fn context<'env>(&'env mut self, context: &'env mut Context) -> DeclContext<'a, 'env, P> {
        DeclContext {
            declaration: self,
            context: context.reborrow(),
        }
    }

    /// Reconcile declared children and retiring links. Only participating children affect
    /// the parent's layout, even though every retained child receives its declaration updates.
    fn finish(self, tree: &mut UiTree, context: &mut Context, links: &mut Vec<ChildLink<P>>) -> bool
    where
        P: PartialEq,
    {
        let mut scoped = context.unobserved();
        if let ChildScope::Native(id) = self.child_scope {
            scoped = scoped.with_window(id);
        }
        let previous_active: Vec<_> = tree.participating_ids(links).collect();
        let mut retained_exits = false;
        let mut layout_changed = false;
        // Only a surviving parent's ordinary child links can be kept until poll.
        // A replaced parent loses its typed links; a native Window has one content slot.
        let can_exit = self.initial_phase.is_none()
            && matches!(self.child_scope, ChildScope::Inherited)
            && scoped.can_poll_widgets();
        for old in self.old_children {
            let node = tree.node_ref_mut(old.node).expect("live omitted child");
            if !can_exit || node.body().is_window() || node.phase == WidgetPhase::PendingEnter {
                tree.remove_reverse_scan_order(old.node, &mut scoped);
                continue;
            }
            retained_exits = true;
            if node.phase != WidgetPhase::Exiting {
                node.phase = WidgetPhase::Exiting;
                layout_changed |= tree.remove_nested_windows(old.node, &mut scoped);
            }
        }

        let mut declared = Vec::with_capacity(self.queued.len());
        let mut active_changed = false;
        for child in self.queued {
            let node = match child.existing {
                Some(old) => {
                    let change = tree.update_node(old.node, child.view, &mut scoped);
                    if tree.participates(old.node) {
                        layout_changed |= change.layout_changed
                            || links
                                .get(old.index)
                                .is_some_and(|link| link.data != child.data);
                        active_changed |= change.replaced;
                    }
                    old.node
                }
                None => tree.build_new(Some(self.current), child.view, &mut scoped),
            };
            declared.push(ChildLink {
                node,
                data: child.data,
            });
        }
        if retained_exits {
            Self::merge_exiting(tree, links, declared);
        } else {
            *links = declared;
        }
        active_changed |= tree.participating_ids(links).ne(previous_active);
        if active_changed && self.initial_phase.is_none() {
            tree.node_ref_mut(self.current)
                .expect("mounted parent")
                .active_children_changed = true;
        }
        active_changed || layout_changed
    }

    // Anchor each run of retirees before its nearest surviving old successor, or at the
    // end if none remains. Declared siblings always follow their new declaration order.
    fn merge_exiting(tree: &UiTree, links: &mut Vec<ChildLink<P>>, declared: Vec<ChildLink<P>>) {
        let mut before = HashMap::new();
        let mut run = Vec::new();
        for old in links.drain(..) {
            if let Some(node) = tree.node_ref(old.node) {
                if node.phase == WidgetPhase::Exiting {
                    run.push(old);
                } else if !run.is_empty() {
                    before.insert(old.node, std::mem::take(&mut run));
                }
            }
        }
        for child in declared {
            if let Some(mut retirees) = before.remove(&child.node) {
                links.append(&mut retirees);
            }
            links.push(child);
        }
        links.append(&mut run);
    }
}

impl<'a, 'env, P> DeclContext<'a, 'env, P> {
    pub fn context(&mut self) -> &mut Context<'env> {
        &mut self.context
    }

    /// Build this widget in `PendingEnter`: it can poll but takes no part in layout, drawing,
    /// or input until it returns [`Lifecycle::Enter`]. Has no effect in `Widget::update`.
    pub fn start_inactive(&mut self) {
        if self.context.can_poll_widgets()
            && let Some(phase) = &mut self.declaration.initial_phase
        {
            *phase = WidgetPhase::PendingEnter;
        }
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
            .rposition(|old| old.key == key && (!old.exiting || matches!(key, UiKey::Key(_))))
            .map(|index| declaration.old_children.remove(index));
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

/// Results consumed and placement performed by a single geometry callback.
#[derive(Default)]
struct ChildGeometry {
    dependencies: EnvReads,
    placed: bool,
}

/// Participating children in layout order. A retiring child keeps its last declared data;
/// a child awaiting entry is not part of this collection.
pub struct Children<'a, P = ()> {
    links: &'a [ChildLink<P>],
    tree: &'a mut UiTree,
    geometry: &'a mut ChildGeometry,
}

impl<P> Children<'_, P> {
    pub fn len(&self) -> usize {
        self.tree.participating_ids(self.links).count()
    }

    pub fn is_empty(&self) -> bool {
        self.tree.participating_ids(self.links).next().is_none()
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
            if !self.tree.participates(link.node) {
                continue;
            }
            visit(
                &link.data,
                Child {
                    tree: self.tree,
                    id: link.node,
                    geometry: self.geometry,
                },
            );
        }
    }
}

/// A child's operations for the duration of a visit through [`Children::for_each`].
pub struct Child<'a> {
    tree: &'a mut UiTree,
    id: UiId,
    geometry: &'a mut ChildGeometry,
}

impl Child<'_> {
    /// Test the child's own shape in parent-local coordinates. Returns `None` when
    /// the child has no placement or the point cannot be mapped into its coordinates.
    pub fn is_inside(&mut self, point: Point, ctx: &mut Context) -> Option<bool> {
        // Placement can reflect an earlier layout, even if no child callback runs here.
        self.geometry.dependencies |= EnvReads::DPI | EnvReads::VIEWPORT_SIZE;
        let node = self.tree.node_ref(self.id).expect("live child");
        if node.phase == WidgetPhase::Exiting {
            return Some(false);
        }
        let placement = node.placement?;
        let point = placement.local_point(point)?;
        let (inside, reads) = ctx.observe_env_reads(|ctx| {
            node.body()
                .as_widget()
                .is_inside(placement.bounds, point, ctx)
        });
        self.geometry.dependencies |= reads;
        Some(inside)
    }
}

#[cfg(test)]
mod tests;
