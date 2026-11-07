use super::*;

#[derive(Default)]
struct Calls {
    updates: usize,
    measures: usize,
    layouts: usize,
}

#[derive(Clone)]
struct NodeView {
    key: u64,
    width: f32,
    children: Vec<NodeView>,
    alternate_child: Option<AlternateView>,
    calls: Arc<Mutex<Calls>>,
}

impl NodeView {
    fn new(key: u64, width: f32, children: Vec<NodeView>) -> Self {
        Self {
            key,
            width,
            children,
            alternate_child: None,
            calls: Arc::default(),
        }
    }

    fn declare_children<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) {
        for child in &self.children {
            ctx.child(child);
        }
        if let Some(child) = &self.alternate_child {
            ctx.child(child);
        }
    }
}

struct NodeWidget {
    width: f32,
    calls: Arc<Mutex<Calls>>,
}

impl View for NodeView {
    type ChildData = ();
    fn label(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> Option<u64> {
        Some(self.key)
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        self.declare_children(ctx);
        NodeWidget {
            width: self.width,
            calls: Arc::clone(&self.calls),
        }
    }
}

impl Widget for NodeWidget {
    type V = NodeView;
    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        self.calls.lock().unwrap().updates += 1;
        let changed = self.width != view.width;
        self.width = view.width;
        // Deliberately report only local changes: the framework must detect child changes.
        view.declare_children(ctx);
        if changed {
            UpdateResult::LAYOUT
        } else {
            UpdateResult::empty()
        }
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }
    fn measure(
        &self,
        constraints: Constraints,
        mut children: Children<'_>,
        ctx: &mut Context,
    ) -> Bounds {
        self.calls.lock().unwrap().measures += 1;
        let mut x = self.width;
        children.for_each(|_, mut child| x += child.measure(constraints, ctx).x);
        Bounds { x, ..bounds() }
    }
    fn layout(&self, assigned: Bounds, mut children: Children<'_>, ctx: &mut Context) {
        self.calls.lock().unwrap().layouts += 1;
        let width = assigned.x / children.len().max(1) as f32;
        let mut index = 0;
        children.for_each(|_, mut child| {
            child.layout(
                Position {
                    bounds: Bounds {
                        x: width,
                        ..assigned
                    },
                    affine: translation(index as f32 * width, 0.0, 0.0),
                },
                ctx,
            );
            index += 1;
        });
    }
    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        Draw {
            color: SmallVec::new(),
            mask: None,
        }
    }
}

fn refresh(tree: &mut UiTree, view: &NodeView) {
    tree.update(
        &ContainerView {
            child: view.clone(),
            child_position: position(Matrix4::identity()),
        },
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
}

fn warm(tree: &mut UiTree, child: UiId) -> Bounds {
    let mut registry = crate::ui_tree::window::WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    let measured = tree.measure_node(child, Constraints::default(), &mut context);
    tree.layout_node(child, position(Matrix4::identity()), &mut context);
    measured
}

fn assert_cached(tree: &UiTree, id: UiId, expected: bool) {
    let node = tree.node_ref(id).unwrap();
    assert_eq!(node.measure_cache.value().is_some(), expected);
    assert_eq!(node.layout_cache.value().is_some(), expected);
}

#[test]
fn unchanged_update_reuses_caches_but_still_updates_descendants() {
    let view = NodeView::new(0, 0.0, vec![NodeView::new(1, 10.0, vec![])]);
    let (mut tree, _, child) = setup(view.clone());
    warm(&mut tree, child);
    refresh(&mut tree, &view);
    assert_eq!(warm(&mut tree, child).x, 10.0);
    for view in [&view, &view.children[0]] {
        let calls = view.calls.lock().unwrap();
        assert_eq!(calls.updates, 1);
        assert_eq!(calls.measures, 1);
        assert_eq!(calls.layouts, 1);
    }
}

#[test]
fn leaf_change_invalidates_all_ancestors_but_preserves_sibling_and_placement() {
    let mut view = NodeView::new(
        0,
        0.0,
        vec![
            NodeView::new(1, 0.0, vec![NodeView::new(2, 10.0, vec![])]),
            NodeView::new(3, 5.0, vec![]),
        ],
    );
    let (mut tree, root, top) = setup(view.clone());
    assert_eq!(warm(&mut tree, top).x, 15.0);
    let branch = child_at(&tree, top, 0);
    let leaf = child_at(&tree, branch, 0);
    let sibling = child_at(&tree, top, 1);
    let placement = tree.node_ref(leaf).unwrap().placement.unwrap();
    warm(&mut tree, root);
    view.children[0].children[0].width = 20.0;
    refresh(&mut tree, &view);
    for id in [root, top, branch, leaf] {
        assert_cached(&tree, id, false);
    }
    assert_cached(&tree, sibling, true);
    let retained = tree.node_ref(leaf).unwrap().placement.unwrap();
    assert_eq!(retained.bounds, placement.bounds);
    assert_eq!(retained.affine, placement.affine);
    assert_eq!(retained.affine_inverse, placement.affine_inverse);
    assert_eq!(warm(&mut tree, top).x, 25.0);
    let sibling_calls = view.children[1].calls.lock().unwrap();
    assert_eq!(sibling_calls.updates, 1); // A changed first child must not skip later children.
    assert_eq!(sibling_calls.measures, 1);
    assert_eq!(sibling_calls.layouts, 1);
}

#[test]
fn parent_change_retains_descendant_caches_until_bounds_change() {
    let mut view = NodeView::new(0, 0.0, vec![NodeView::new(1, 10.0, vec![])]);
    let (mut tree, _, parent) = setup(view.clone());
    warm(&mut tree, parent);
    let leaf = child_at(&tree, parent, 0);
    view.width = 2.0;
    refresh(&mut tree, &view);
    assert_cached(&tree, parent, false);
    assert_cached(&tree, leaf, true);
    assert_eq!(warm(&mut tree, parent).x, 12.0);
    assert_eq!(view.calls.lock().unwrap().layouts, 2);
    assert_eq!(view.children[0].calls.lock().unwrap().layouts, 1);
    let mut larger = position(Matrix4::identity());
    larger.bounds.x = 20.0;
    tree.layout_node(
        parent,
        larger,
        &mut Context::new(&mut crate::ui_tree::window::WindowRegistry::default()),
    );
    assert_eq!(view.children[0].calls.lock().unwrap().layouts, 2);
    assert_eq!(view.children[0].calls.lock().unwrap().measures, 1);
}

#[test]
fn adding_and_removing_children_invalidates_unchanged_parent() {
    let mut view = NodeView::new(0, 0.0, vec![NodeView::new(1, 10.0, vec![])]);
    let (mut tree, _, parent) = setup(view.clone());
    warm(&mut tree, parent);
    view.children.push(NodeView::new(2, 5.0, vec![]));
    refresh(&mut tree, &view);
    assert_cached(&tree, parent, false);
    assert_eq!(warm(&mut tree, parent).x, 15.0);
    let removed = child_at(&tree, parent, 1);
    assert!(tree.node_ref(removed).unwrap().placement.is_some());
    // Removing the last child is detected at finish, with no out-of-order child declarations.
    view.children.pop();
    refresh(&mut tree, &view);
    assert_cached(&tree, parent, false);
    assert!(tree.node_ref(removed).is_none());
    assert_eq!(warm(&mut tree, parent).x, 10.0);
    view.children.clear();
    refresh(&mut tree, &view);
    assert_cached(&tree, parent, false);
    assert_eq!(warm(&mut tree, parent).x, 0.0);
}

#[test]
fn reordering_keyed_children_repositions_them_without_relaying_out_children() {
    let mut view = NodeView::new(
        0,
        0.0,
        vec![
            NodeView::new(1, 10.0, vec![]),
            NodeView::new(2, 5.0, vec![]),
        ],
    );
    let (mut tree, _, parent) = setup(view.clone());
    warm(&mut tree, parent);
    let previous = tree
        .node_ref(parent)
        .unwrap()
        .child_ids()
        .collect::<Vec<_>>();
    view.children.swap(0, 1);
    refresh(&mut tree, &view);
    assert_cached(&tree, parent, false);
    assert_eq!(
        tree.node_ref(parent)
            .unwrap()
            .child_ids()
            .collect::<Vec<_>>(),
        [previous[1], previous[0]]
    );
    for id in &previous {
        assert_cached(&tree, *id, true);
    }
    warm(&mut tree, parent);
    assert_eq!(
        tree.node_ref(previous[0])
            .unwrap()
            .placement
            .unwrap()
            .affine,
        translation(5.0, 0.0, 0.0)
    );
    for child in &view.children {
        assert_eq!(child.calls.lock().unwrap().layouts, 1);
    }
}

#[derive(Clone)]
struct AlternateView {
    key: u64,
    width: f32,
}

struct AlternateWidget(f32);

impl View for AlternateView {
    type ChildData = ();
    fn label(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> Option<u64> {
        Some(self.key)
    }
    fn build<'a>(&'a self, _: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        AlternateWidget(self.width)
    }
}

impl Widget for AlternateWidget {
    type V = AlternateView;
    fn update<'a>(&mut self, view: &'a Self::V, _: &mut DeclContext<'a, '_>) -> UpdateResult {
        let changed = self.0 != view.width;
        self.0 = view.width;
        if changed {
            UpdateResult::LAYOUT
        } else {
            UpdateResult::empty()
        }
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }
    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds {
            x: self.0,
            ..bounds()
        }
    }
    fn layout(&self, _: Bounds, _: Children<'_>, _: &mut Context) {}
    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        Draw {
            color: SmallVec::new(),
            mask: None,
        }
    }
}

#[test]
fn same_key_type_rebuild_reports_layout_change_at_the_reused_arena_slot() {
    let mut view = NodeView::new(0, 0.0, vec![NodeView::new(1, 10.0, vec![])]);
    let (mut tree, root, parent) = setup(view.clone());
    let child = child_at(&tree, parent, 0);
    assert_eq!(warm(&mut tree, root).x, 10.0);
    view.children.clear();
    view.alternate_child = Some(AlternateView {
        key: 1,
        width: 20.0,
    });
    refresh(&mut tree, &view);
    assert_eq!(child_at(&tree, parent, 0), child);
    for id in [root, parent, child] {
        assert_cached(&tree, id, false);
    }
    assert_eq!(warm(&mut tree, root).x, 20.0);
}
