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
    calls: Arc<Mutex<Calls>>,
}

impl NodeView {
    fn new(key: u64, width: f32, children: Vec<NodeView>) -> Self {
        Self {
            key,
            width,
            children,
            calls: Arc::default(),
        }
    }
}

struct NodeWidget {
    width: f32,
    children: Vec<ChildId>,
    calls: Arc<Mutex<Calls>>,
}

impl View for NodeView {
    fn label(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> Option<u64> {
        Some(self.key)
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a>) -> impl Widget<V = Self> {
        NodeWidget {
            width: self.width,
            children: self.children.iter().map(|child| ctx.child(child)).collect(),
            calls: Arc::clone(&self.calls),
        }
    }
}

impl Widget for NodeWidget {
    type V = NodeView;
    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a>) -> UpdateResult {
        self.calls.lock().unwrap().updates += 1;
        let changed = self.width != view.width;
        self.width = view.width;
        // Deliberately report only local changes: the framework must detect child changes.
        self.children = view.children.iter().map(|child| ctx.child(child)).collect();
        if changed {
            UpdateResult::LAYOUT
        } else {
            UpdateResult::empty()
        }
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: Context) -> InputResult {
        InputResult::default()
    }
    fn measure(
        &self,
        constraints: Constraints,
        mut children: Children<'_>,
        ctx: Context,
    ) -> Bounds {
        self.calls.lock().unwrap().measures += 1;
        let x = self.width
            + self
                .children
                .iter()
                .map(|id| children.measure(*id, constraints, ctx.clone()).unwrap().x)
                .sum::<f32>();
        Bounds { x, ..bounds() }
    }
    fn layout(&self, assigned: Bounds, mut children: Children<'_>, ctx: Context) {
        self.calls.lock().unwrap().layouts += 1;
        let width = assigned.x / self.children.len().max(1) as f32;
        for (index, id) in self.children.iter().enumerate() {
            children.layout(
                *id,
                Position {
                    bounds: Bounds {
                        x: width,
                        ..assigned
                    },
                    affine: translation(index as f32 * width, 0.0, 0.0),
                },
                ctx.clone(),
            );
        }
    }
    fn render(&self, _: Bounds, _: Context) -> Draw {
        Draw {
            color: SmallVec::new(),
            mask: None,
        }
    }
}

fn refresh(tree: &mut UiTree, view: &NodeView) {
    tree.update(&ContainerView {
        child: view.clone(),
        child_position: position(Matrix4::identity()),
    });
}

fn warm(tree: &mut UiTree, root: UiId, child: ChildId) -> Bounds {
    let mut children = Children {
        tree,
        current: root,
    };
    let measured = children
        .measure(child, Constraints::default(), Context::new())
        .unwrap();
    children.layout(child, position(Matrix4::identity()), Context::new());
    measured
}

fn assert_cached(tree: &UiTree, id: UiId, expected: bool) {
    let node = tree.node_ref(id).unwrap();
    assert_eq!(node.measure_cache.is_some(), expected);
    assert_eq!(node.layout_cache.is_some(), expected);
}

#[test]
fn unchanged_update_reuses_caches_but_still_updates_descendants() {
    let view = NodeView::new(0, 0.0, vec![NodeView::new(1, 10.0, vec![])]);
    let (mut tree, root, child) = setup(view.clone());
    warm(&mut tree, root, child);
    refresh(&mut tree, &view);
    assert_eq!(warm(&mut tree, root, child).x, 10.0);
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
    let (mut tree, root, child) = setup(view.clone());
    assert_eq!(warm(&mut tree, root, child).x, 15.0);
    let top = tree.node_ref(root).unwrap().child(child).unwrap();
    let branch = tree.node_ref(top).unwrap().children[0].1;
    let leaf = tree.node_ref(branch).unwrap().children[0].1;
    let sibling = tree.node_ref(top).unwrap().children[1].1;
    let placement = tree.node_ref(leaf).unwrap().placement.unwrap();
    // Root operations are not exposed yet; seed its caches to check propagation to the root.
    tree.node_ref_mut(root).unwrap().measure_cache = Some((Constraints::default(), bounds()));
    tree.node_ref_mut(root).unwrap().layout_cache = Some(bounds());
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
    assert_eq!(warm(&mut tree, root, child).x, 25.0);
    let sibling_calls = view.children[1].calls.lock().unwrap();
    assert_eq!(sibling_calls.updates, 1); // A changed first child must not skip later children.
    assert_eq!(sibling_calls.measures, 1);
    assert_eq!(sibling_calls.layouts, 1);
}

#[test]
fn parent_change_retains_descendant_caches_until_bounds_change() {
    let mut view = NodeView::new(0, 0.0, vec![NodeView::new(1, 10.0, vec![])]);
    let (mut tree, root, child) = setup(view.clone());
    warm(&mut tree, root, child);
    let parent = tree.node_ref(root).unwrap().child(child).unwrap();
    let leaf = tree.node_ref(parent).unwrap().children[0].1;
    view.width = 2.0;
    refresh(&mut tree, &view);
    assert_cached(&tree, parent, false);
    assert_cached(&tree, leaf, true);
    assert_eq!(warm(&mut tree, root, child).x, 12.0);
    assert_eq!(view.calls.lock().unwrap().layouts, 2);
    assert_eq!(view.children[0].calls.lock().unwrap().layouts, 1);
    let mut larger = position(Matrix4::identity());
    larger.bounds.x = 20.0;
    Children {
        tree: &mut tree,
        current: root,
    }
    .layout(child, larger, Context::new());
    assert_eq!(view.children[0].calls.lock().unwrap().layouts, 2);
    assert_eq!(view.children[0].calls.lock().unwrap().measures, 1);
}

#[test]
fn adding_and_removing_children_invalidates_unchanged_parent() {
    let mut view = NodeView::new(0, 0.0, vec![NodeView::new(1, 10.0, vec![])]);
    let (mut tree, root, child) = setup(view.clone());
    warm(&mut tree, root, child);
    let parent = tree.node_ref(root).unwrap().child(child).unwrap();
    view.children.push(NodeView::new(2, 5.0, vec![]));
    refresh(&mut tree, &view);
    assert_cached(&tree, parent, false);
    assert_eq!(warm(&mut tree, root, child).x, 15.0);
    let removed = tree.node_ref(parent).unwrap().children[1].1;
    assert!(tree.node_ref(removed).unwrap().placement.is_some());
    // Removing the last child is detected at finish, with no out-of-order child declarations.
    view.children.pop();
    refresh(&mut tree, &view);
    assert_cached(&tree, parent, false);
    assert!(tree.node_ref(removed).is_none());
    assert_eq!(warm(&mut tree, root, child).x, 10.0);
    view.children.clear();
    refresh(&mut tree, &view);
    assert_cached(&tree, parent, false);
    assert_eq!(warm(&mut tree, root, child).x, 0.0);
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
    let (mut tree, root, child) = setup(view.clone());
    warm(&mut tree, root, child);
    let parent = tree.node_ref(root).unwrap().child(child).unwrap();
    let previous = tree.node_ref(parent).unwrap().children.clone();
    view.children.swap(0, 1);
    refresh(&mut tree, &view);
    assert_cached(&tree, parent, false);
    assert_eq!(
        tree.node_ref(parent).unwrap().children.as_slice(),
        &[previous[1], previous[0]]
    );
    for (_, id) in &previous {
        assert_cached(&tree, *id, true);
    }
    warm(&mut tree, root, child);
    assert_eq!(
        tree.node_ref(previous[0].1)
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

struct AlternateView {
    key: u64,
    width: f32,
}

struct AlternateWidget(f32);

impl View for AlternateView {
    fn label(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> Option<u64> {
        Some(self.key)
    }
    fn build<'a>(&'a self, _: &mut DeclContext<'a>) -> impl Widget<V = Self> {
        AlternateWidget(self.width)
    }
}

impl Widget for AlternateWidget {
    type V = AlternateView;
    fn update<'a>(&mut self, view: &'a Self::V, _: &mut DeclContext<'a>) -> UpdateResult {
        let changed = self.0 != view.width;
        self.0 = view.width;
        if changed {
            UpdateResult::LAYOUT
        } else {
            UpdateResult::empty()
        }
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: Context) -> InputResult {
        InputResult::default()
    }
    fn measure(&self, _: Constraints, _: Children<'_>, _: Context) -> Bounds {
        Bounds {
            x: self.0,
            ..bounds()
        }
    }
    fn layout(&self, _: Bounds, _: Children<'_>, _: Context) {}
    fn render(&self, _: Bounds, _: Context) -> Draw {
        Draw {
            color: SmallVec::new(),
            mask: None,
        }
    }
}

#[test]
fn same_key_type_rebuild_is_reported_even_when_child_ids_do_not_change() {
    let (mut tree, root, child) = setup(NodeView::new(1, 10.0, vec![]));
    assert_eq!(warm(&mut tree, root, child).x, 10.0);
    let old_children = std::mem::take(&mut tree.node_ref_mut(root).unwrap().children);
    let mut ctx = DeclContext {
        context: tree.context.clone(),
        current: root,
        window: None,
        old_children: old_children
            .iter()
            .map(|(child, id)| (tree.node_ref(*id).unwrap().key, *child, *id))
            .collect(),
        queued_widgets: Vec::new(),
        structure_changed: false,
    };
    let replacement = AlternateView {
        key: 1,
        width: 20.0,
    };
    assert_eq!(ctx.child(&replacement), child);
    assert!(!ctx.structure_changed);
    let (new_children, changed) = ctx.finish(&mut tree, root);
    assert!(changed);
    assert_eq!(new_children, old_children);
    assert_cached(&tree, new_children[0].1, false);
    tree.node_ref_mut(root).unwrap().children = new_children;
    assert_eq!(warm(&mut tree, root, child).x, 20.0);
}
