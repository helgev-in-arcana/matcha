use std::any::{Any, TypeId};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::metrics::{Bounds, Constraints, Point, Position};

// -----------
// Type eraser
// -----------

trait AnyView: Any {
    fn label(&self) -> Option<&str>;
    fn ui_key(&self) -> UiKey;
    fn build<'a>(&'a self, ctx: &mut Context<'a>) -> Box<dyn AnyWidget>;
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
    fn build<'a>(&'a self, ctx: &mut Context<'a>) -> Box<dyn AnyWidget> {
        Box::new(View::build(self, ctx))
    }
}

trait AnyWidget: Any {
    fn try_update<'a>(
        &mut self,
        view: &'a dyn AnyView,
        ctx: &mut Context<'a>,
    ) -> Result<(), UiInternalError>;
    fn input(&mut self, bounds: Bounds, event: (), ctx: ());
    fn is_inside(&self, b: Bounds, p: Point, ctx: ()) -> bool;
    fn measure<'a>(&'a self, constraints: Constraints, ctx: ()) -> Layout<'a>;
    fn layout<'a>(&'a self, bounds: Bounds, ctx: ()) -> Layout<'a>;
    fn render(&self, bounds: Bounds, ctx: ());
}

impl<T: Widget> AnyWidget for T {
    fn try_update<'a>(
        &mut self,
        view: &'a dyn AnyView,
        ctx: &mut Context<'a>,
    ) -> Result<(), UiInternalError> {
        if let Some(view) = (view as &dyn Any).downcast_ref::<T::V>() {
            Widget::update(self, view, ctx);
            Ok(())
        } else {
            Err(UiInternalError::WidgetUpdateFailedTypeMismatch)
        }
    }

    fn input(&mut self, bounds: Bounds, event: (), ctx: ()) {
        Widget::input(self, bounds, event, ctx);
    }
    fn is_inside(&self, b: Bounds, p: Point, ctx: ()) -> bool {
        Widget::is_inside(self, b, p, ctx)
    }
    fn measure<'a>(&'a self, constraints: Constraints, ctx: ()) -> Layout<'a> {
        Widget::measure(self, constraints, ctx)
    }
    fn layout<'a>(&'a self, bounds: Bounds, ctx: ()) -> Layout<'a> {
        Widget::layout(self, bounds, ctx)
    }
    fn render(&self, bounds: Bounds, ctx: ()) {
        Widget::render(self, bounds, ctx)
    }
}

// -----
// Arena
// -----

pub(crate) struct UiArena {
    nodes: Vec<UiSlot>,
    root: Option<UiId>,
    first_free: usize,
}

pub(crate) enum UiSlot {
    Free { next: usize, generation: u64 },
    Node { node: UiNode, generation: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct UiId {
    index: usize,
    generation: u64,
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

pub(crate) struct UiNode {
    // identify
    label: Option<String>,
    key: UiKey,

    // tree
    parent: Option<UiId>,
    children: Vec<(ChildId, UiId)>,

    // caches
    measure_cache: Option<(Constraints, Bounds)>,
    layout_cache: Option<Position>,

    // flags
    relayout_needed: bool,
    redraw_needed: bool,

    // widget main body
    widget: Box<dyn AnyWidget>,
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

impl UiArena {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            root: None,
            first_free: 0,
        }
    }

    /// `f` builds the node (and, recursively, its subtree) for the reserved `id`. While `f` runs
    /// the slot is not yet a `Node`, so `node_ref_mut(id)` returns `None` for that id.
    fn alloc(&mut self, f: impl FnOnce(&mut Self, UiId) -> UiNode) -> UiId {
        let alloc = self.first_free;

        if self.nodes.len() == alloc {
            let generation = 0;

            // update with UiSlot::Node later
            self.nodes.push(UiSlot::Free {
                next: 0,
                generation: 0,
            });
            self.first_free = alloc + 1;

            let id = UiId {
                index: alloc,
                generation,
            };
            let node = f(self, id);
            self.nodes[alloc] = UiSlot::Node { node, generation };

            id
        } else {
            match self.nodes[alloc] {
                UiSlot::Node { .. } => {
                    unreachable!("There is a bug if you see this.")
                }
                UiSlot::Free { next, generation } => {
                    self.first_free = next;

                    let generation = generation + 1;
                    let id = UiId {
                        index: alloc,
                        generation,
                    };
                    let node = f(self, id);
                    self.nodes[alloc] = UiSlot::Node { node, generation };

                    id
                }
            }
        }
    }

    fn free(&mut self, id: UiId) {
        let generation = match &mut self.nodes[id.index] {
            UiSlot::Node { generation, .. } => generation,
            UiSlot::Free { .. } => return,
        };

        if *generation == id.generation {
            self.nodes[id.index] = UiSlot::Free {
                next: self.first_free,
                generation: *generation,
            };
            self.first_free = id.index;
        }
    }

    fn remove_reverse_scan_order(&mut self, remove_from: UiId) {
        let Some(node) = &mut self.node_ref_mut(remove_from) else {
            return;
        };

        let children = std::mem::take(&mut node.children);

        for (_, child_id) in children.into_iter().rev() {
            self.remove_reverse_scan_order(child_id);
        }

        self.free(remove_from);
    }

    fn node_ref_mut(&mut self, id: UiId) -> Option<&mut UiNode> {
        match self.nodes.get_mut(id.index)? {
            UiSlot::Node { node, generation } => {
                if *generation == id.generation {
                    Some(node)
                } else {
                    None
                }
            }
            UiSlot::Free { .. } => None,
        }
    }
}

// some typed function called by the context.
impl UiArena {}

// -------------------
// UI Build and Update
// -------------------

// Matching contract (optimisation contract: violating it never corrupts the tree, it only makes
// state follow the wrong view):
// - Explicit keys must be unique among siblings. Checked by a `debug_assert!` in
//   `Context::child`; in release the first remaining old child with that key wins.
// - Unkeyed siblings of the same type are matched by order of appearance. Reordering them moves
//   their state with the position, not with the view; give them explicit keys to reorder.

pub struct Context<'a> {
    // Children of the previous frame not yet claimed by `Context::child`.
    old_children: Vec<(UiKey, ChildId, UiId)>,
    queued_widgets: Vec<QueuedView<'a>>,
}

struct QueuedView<'a> {
    // Some: the view is already in the tree, and this is its index
    // None: the view is new, and needs to be built and added to the tree
    existing_index: Option<UiId>,
    key: UiKey,
    view: &'a dyn AnyView,
    child_id: ChildId,
}

impl Context<'_> {
    fn finish(self, arena: &mut UiArena, current_id: UiId) -> Vec<(ChildId, UiId)> {
        let Self {
            old_children,
            queued_widgets,
        } = self;

        // remove old branch
        for (_, _, id) in old_children.iter().rev() {
            arena.remove_reverse_scan_order(*id);
        }

        let mut new_children = Vec::new();

        // reorder children and build new branch
        for q in &queued_widgets {
            if let Some(scoped_id) = q.existing_index {
                // update it
                update(arena, scoped_id, q.view);
                new_children.push((q.child_id, scoped_id));
            } else {
                // build new
                let scoped_id =
                    arena.alloc(|arena, id| build(arena, Some(current_id), id, q.view, None));
                new_children.push((q.child_id, scoped_id));
            }
        }

        new_children
    }
}

// ----------
// Operations
// ----------

pub(crate) fn update_all<V: View>(arena: &mut UiArena, view: &V) {
    if let Some(id) = arena.root {
        update(arena, id, view);
    } else {
        // here is no valid ui tree.
        // reset vec for linear free list
        for (index, slot) in arena.nodes.iter_mut().enumerate() {
            *slot = match slot {
                UiSlot::Free { generation, .. } | UiSlot::Node { generation, .. } => UiSlot::Free {
                    next: index + 1,
                    generation: *generation,
                },
            };
        }
        arena.root = None;
        arena.first_free = 0;
        // build new widget tree
        let id = arena.alloc(|arena, id| build(arena, None, id, view, None));
        arena.root = Some(id);
    }
}

fn build(
    arena: &mut UiArena,
    parent: Option<UiId>,
    id: UiId,
    view: &dyn AnyView,
    child_candidate: Option<Vec<(UiKey, ChildId, UiId)>>,
) -> UiNode {
    let current_id = id;

    let mut ctx = Context {
        old_children: child_candidate.unwrap_or_default(),
        queued_widgets: Vec::new(),
    };

    let label = view.label().map(|s| s.to_string());
    let key = view.ui_key();
    let widget = view.build(&mut ctx);
    let children = ctx.finish(arena, current_id);

    UiNode {
        label,
        key,
        parent,
        children,
        measure_cache: None,
        layout_cache: None,
        relayout_needed: true,
        redraw_needed: true,
        widget,
    }
}

fn update(arena: &mut UiArena, id: UiId, view: &dyn AnyView) {
    let node = arena
        .node_ref_mut(id)
        .expect("`update` is only called with ids taken from the live tree.");

    let mut ctx = Context {
        old_children: std::mem::take(&mut node.children)
            .into_iter()
            .map(|(child_id, id)| {
                let key = arena
                    .node_ref_mut(id)
                    .expect("A node's children are freed only together with the node itself.")
                    .key;
                (key, child_id, id)
            })
            .collect::<Vec<_>>(),
        queued_widgets: Vec::new(),
    };

    match arena
        .node_ref_mut(id)
        .unwrap()
        .widget
        .try_update(view, &mut ctx)
    {
        Ok(_) => {
            let children = ctx.finish(arena, id);
            let key = view.ui_key();
            // re-get the node after finish to avoid borrow checker issues
            let node = arena.node_ref_mut(id).unwrap();
            // compare first: this runs for every node on every frame, and labels rarely change
            let label = view.label();
            if node.label.as_deref() != label {
                node.label = label.map(str::to_owned);
            }
            node.key = key;
            node.children = children;
        }
        Err(UiInternalError::WidgetUpdateFailedTypeMismatch) => {
            let parent = arena.node_ref_mut(id).unwrap().parent;
            // rebuild
            let node = build(arena, parent, id, view, Some(ctx.old_children));
            let node_mut = arena.node_ref_mut(id).unwrap();
            *node_mut = node;
        }
    }
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

impl<'a> Context<'a> {
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
            .map(|i| self.old_children.remove(i));

        // A reused child keeps its `ChildId`, so ids held across frames stay valid.
        let (existing_index, child_id) = match existing {
            Some((_, child_id, id)) => (Some(id), child_id),
            None => (None, ChildId::new()),
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

/// Handle to a child, issued by `Context::child`.
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
            uid: NEXT.fetch_add(1, Ordering::Relaxed),
        }
    }
}

pub trait View: Any {
    fn label(&self) -> Option<&str>;
    /// Explicit identity among siblings. `None` matches by type and order of appearance.
    fn key(&self) -> Option<u64> {
        None
    }
    fn build<'a>(&'a self, ctx: &mut Context<'a>) -> impl Widget<V = Self>;
}

pub trait Widget: Any {
    type V: View;

    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut Context<'a>);

    fn input(&mut self, bounds: Bounds, event: (), ctx: ());

    fn is_inside(&self, b: Bounds, p: Point, ctx: ()) -> bool {
        let _ = ctx;

        (0.0 <= p.x && p.x <= b.x) && (0.0 <= p.y && p.y <= b.y) && (0.0 <= p.z && p.z <= b.z)
    }

    fn measure<'a>(&'a self, constraints: Constraints, ctx: ()) -> Layout<'a>;

    fn layout<'a>(&'a self, bounds: Bounds, ctx: ()) -> Layout<'a>;

    fn render(&self, bounds: Bounds, ctx: ());
}

pub struct Layout<'a> {
    pub bounds: Bounds,
    pub children: &'a [(ChildId, Position)],
}

pub enum UiError {}
