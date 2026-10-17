use super::*;

#[derive(Debug, Clone, PartialEq)]
struct Delivery {
    key: u64,
    preview: bool,
    mouse: Option<[f32; 2]>,
}

#[derive(Clone)]
struct InputView {
    key: u64,
    placement: Position,
    children: Vec<InputView>,
    preview: InputResult,
    input: InputResult,
    hit_self: bool,
    log: Arc<Mutex<Vec<Delivery>>>,
}

impl InputView {
    fn node(key: u64, log: &Arc<Mutex<Vec<Delivery>>>, children: Vec<Self>) -> Self {
        Self {
            key,
            placement: position(Matrix4::identity()),
            children,
            preview: InputResult::default(),
            input: InputResult::default(),
            hit_self: true,
            log: Arc::clone(log),
        }
    }
}

struct InputWidget {
    view: InputView,
    children: Vec<ChildId>,
    width: f32,
}

impl View for InputView {
    fn label(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> Option<u64> {
        Some(self.key)
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a>) -> impl Widget<V = Self> {
        InputWidget {
            view: self.clone(),
            children: self.children.iter().map(|child| ctx.child(child)).collect(),
            width: 10.0,
        }
    }
}

impl InputWidget {
    fn record(&mut self, preview: bool, event: &DeviceEvent) -> InputResult {
        self.view.log.lock().unwrap().push(Delivery {
            key: self.view.key,
            preview,
            mouse: event.mouse_position(),
        });
        let result = if preview {
            self.view.preview
        } else {
            self.view.input
        };
        if result.update.contains(UpdateResult::LAYOUT) {
            self.width += 1.0;
        }
        result
    }
}

impl Widget for InputWidget {
    type V = InputView;
    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a>) -> UpdateResult {
        self.view = view.clone();
        self.children = view.children.iter().map(|child| ctx.child(child)).collect();
        UpdateResult::empty()
    }
    fn preview_input(&mut self, _: Bounds, event: &DeviceEvent, _: Context) -> InputResult {
        self.record(true, event)
    }
    fn input(&mut self, _: Bounds, event: &DeviceEvent, _: Context) -> InputResult {
        self.record(false, event)
    }
    fn is_inside(&self, b: Bounds, p: Point, _: Context) -> bool {
        self.view.hit_self
            && (0.0..=b.x).contains(&p.x)
            && (0.0..=b.y).contains(&p.y)
            && (0.0..=b.z).contains(&p.z)
    }
    fn measure(
        &self,
        constraints: Constraints,
        mut children: Children<'_>,
        ctx: Context,
    ) -> Bounds {
        let mut width = self.width;
        for child in &self.children {
            width += children
                .measure(*child, constraints, ctx.clone())
                .unwrap()
                .x;
        }
        Bounds {
            x: width,
            ..bounds()
        }
    }
    fn layout(&self, _: Bounds, mut children: Children<'_>, ctx: Context) {
        for (id, child) in self.children.iter().zip(&self.view.children) {
            children.layout(*id, child.placement, ctx.clone());
        }
    }
    fn render(&self, _: Bounds, _: Context) -> Draw {
        Draw {
            color: SmallVec::new(),
            mask: None,
        }
    }
}

fn tree() -> InputView {
    let log = Arc::default();
    let leaf = InputView::node(3, &log, vec![]);
    let branch = InputView::node(2, &log, vec![leaf]);
    let mut sibling = InputView::node(4, &log, vec![]);
    sibling.placement.affine = translation(100.0, 0.0, 0.0);
    InputView::node(1, &log, vec![branch, sibling])
}

fn ready(view: &InputView) -> UiTree {
    let mut tree = UiTree::new();
    tree.update(view);
    // Warm measurement through the existing widget entry point; root measurement API is
    // separate work. Root layout and input use the actual framework entry points below.
    let root = tree.root.unwrap();
    let widget = tree.node_ref_mut(root).unwrap().widget.take().unwrap();
    let measured = widget.measure(
        Constraints::default(),
        Children {
            tree: &mut tree,
            current: root,
        },
        Context::new(),
    );
    let node = tree.node_ref_mut(root).unwrap();
    node.widget = Some(widget);
    node.measure_cache = Some((Constraints::default(), measured));
    tree.layout(bounds(), Context::new());
    tree
}

fn find(tree: &UiTree, key: u64) -> UiId {
    tree.nodes
        .iter()
        .find_map(|(id, node)| (node.key == UiKey::Key(key)).then_some(id))
        .unwrap()
}

fn assert_invalidated(tree: &UiTree, changed_keys: &[u64]) {
    for (_, node) in tree.nodes.iter() {
        let UiKey::Key(key) = node.key else {
            panic!("test widgets are keyed")
        };
        assert_eq!(
            node.measure_cache.is_none(),
            changed_keys.contains(&key),
            "measure: {key}"
        );
        assert_eq!(
            node.layout_cache.is_none(),
            changed_keys.contains(&key),
            "layout: {key}"
        );
        assert!(node.placement.is_some());
    }
}

fn order(view: &InputView) -> Vec<(u64, bool)> {
    view.log
        .lock()
        .unwrap()
        .iter()
        .map(|d| (d.key, d.preview))
        .collect()
}

#[test]
fn preview_descends_and_normal_input_bubbles_on_one_path() {
    let view = tree();
    let mut tree = ready(&view);
    let result = tree.input(&mouse_event(5.0, 5.0), Context::new());
    assert_eq!(result, InputResult::default());
    assert_eq!(
        order(&view),
        vec![
            (1, true),
            (2, true),
            (3, true),
            (3, false),
            (2, false),
            (1, false)
        ]
    );
    assert_invalidated(&tree, &[]);
}

#[test]
fn preview_stop_prevents_target_delivery_but_invalidates_ancestors() {
    let mut view = tree();
    view.children[0].preview = InputResult {
        update: UpdateResult::LAYOUT,
        propagation: InputPropagation::Stop,
    };
    let mut tree = ready(&view);
    assert_eq!(
        tree.input(&mouse_event(5.0, 5.0), Context::new()),
        view.children[0].preview
    );
    assert_eq!(order(&view), vec![(1, true), (2, true)]);
    assert_invalidated(&tree, &[1, 2]);
}

#[test]
fn target_stop_does_not_stop_cache_invalidation_to_root() {
    let mut view = tree();
    view.children[0].children[0].input = InputResult {
        update: UpdateResult::LAYOUT,
        propagation: InputPropagation::Stop,
    };
    let mut tree = ready(&view);
    let result = tree.input(&mouse_event(5.0, 5.0), Context::new());
    assert_eq!(result, view.children[0].children[0].input);
    assert_eq!(
        order(&view),
        vec![(1, true), (2, true), (3, true), (3, false)]
    );
    assert_invalidated(&tree, &[1, 2, 3]);
    // The actual mutated size must be observed under the same constraints.
    let parent = find(&tree, 2);
    let leaf = tree.node_ref(parent).unwrap().children[0].0;
    assert_eq!(
        Children {
            tree: &mut tree,
            current: parent
        }
        .measure(leaf, Constraints::default(), Context::new())
        .unwrap()
        .x,
        11.0
    );
}

#[test]
fn changes_in_preview_survive_a_later_unchanged_stop() {
    let mut view = tree();
    view.preview.update = UpdateResult::LAYOUT;
    view.children[0].children[0].input.propagation = InputPropagation::Stop;
    let mut tree = ready(&view);
    let result = tree.input(&mouse_event(5.0, 5.0), Context::new());
    assert_eq!(
        result,
        InputResult {
            update: UpdateResult::LAYOUT,
            propagation: InputPropagation::Stop
        }
    );
    assert_invalidated(&tree, &[1]);
}

#[test]
fn unchanged_stop_preserves_all_caches() {
    let mut view = tree();
    view.children[0].input.propagation = InputPropagation::Stop;
    let mut tree = ready(&view);
    assert_eq!(
        tree.input(&mouse_event(5.0, 5.0), Context::new()),
        view.children[0].input
    );
    assert_eq!(
        order(&view),
        vec![(1, true), (2, true), (3, true), (3, false), (2, false)]
    );
    assert_invalidated(&tree, &[]);
}

#[test]
fn changed_continue_reaches_root_and_preserves_unvisited_sibling() {
    let mut view = tree();
    view.children[0].children[0].input.update = UpdateResult::LAYOUT;
    let mut tree = ready(&view);
    let result = tree.input(&mouse_event(5.0, 5.0), Context::new());
    assert_eq!(result, view.children[0].children[0].input);
    assert_eq!(order(&view).last(), Some(&(1, false)));
    assert_invalidated(&tree, &[1, 2, 3]);
}

#[test]
fn overflowing_child_is_targeted_and_each_phase_gets_local_coordinates() {
    let mut view = tree();
    view.children.pop();
    view.children[0].placement.affine = translation(100.0, 0.0, 0.0);
    view.children[0].children[0].placement.affine = translation(20.0, 0.0, 0.0);
    let mut tree = ready(&view);
    let _ = tree.input(&mouse_event(125.0, 5.0), Context::new());
    assert_eq!(
        order(&view),
        vec![
            (1, true),
            (2, true),
            (3, true),
            (3, false),
            (2, false),
            (1, false)
        ]
    );
    let points: Vec<_> = view
        .log
        .lock()
        .unwrap()
        .iter()
        .map(|d| d.mouse.unwrap()[0])
        .collect();
    assert_eq!(points, vec![125.0, 25.0, 5.0, 5.0, 25.0, 125.0]);
}

#[test]
fn overlapping_siblings_follow_declaration_order_even_after_slot_preserving_reorder() {
    let log = Arc::default();
    let mut view = InputView::node(
        1,
        &log,
        vec![
            InputView::node(2, &log, vec![]),
            InputView::node(3, &log, vec![]),
        ],
    );
    let mut tree = ready(&view);
    let original_front = find(&tree, 3);
    let _ = tree.input(&mouse_event(5.0, 5.0), Context::new());
    // Continue bubbles to the parent, not through to the sibling behind it.
    assert_eq!(
        order(&view),
        vec![(1, true), (3, true), (3, false), (1, false)]
    );
    view.log.lock().unwrap().clear();
    view.children.swap(0, 1);
    tree.update(&view);
    tree.layout(bounds(), Context::new());
    assert_eq!(find(&tree, 3), original_front);
    let _ = tree.input(&mouse_event(5.0, 5.0), Context::new());
    assert_eq!(
        order(&view),
        vec![(1, true), (2, true), (2, false), (1, false)]
    );
}

#[test]
fn singular_and_non_hitting_front_widgets_allow_searching_behind_them() {
    for singular in [false, true] {
        let log = Arc::default();
        let back = InputView::node(2, &log, vec![]);
        let mut front = InputView::node(3, &log, vec![]);
        if singular {
            front.placement.affine = Matrix4::new_scaling(0.0);
        } else {
            front.hit_self = false;
        }
        let view = InputView::node(1, &log, vec![back, front]);
        let mut tree = ready(&view);
        let _ = tree.input(&mouse_event(5.0, 5.0), Context::new());
        assert_eq!(
            order(&view),
            vec![(1, true), (2, true), (2, false), (1, false)]
        );
    }
}

#[test]
fn empty_unplaced_and_missed_trees_produce_no_callbacks() {
    let view = tree();
    let mut tree = UiTree::new();
    let event = mouse_event(500.0, 500.0);
    assert_eq!(tree.input(&event, Context::new()), InputResult::default());
    tree.update(&view);
    assert_eq!(tree.input(&event, Context::new()), InputResult::default());
    tree.layout(bounds(), Context::new());
    assert_eq!(tree.input(&event, Context::new()), InputResult::default());
    assert!(view.log.lock().unwrap().is_empty());
}

#[test]
fn preview_and_bubble_union_flags_without_cross_invalidating_drawing() {
    let mut view = tree();
    view.preview.update = UpdateResult::RENDER;
    view.children[0].children[0].input = InputResult {
        update: UpdateResult::LAYOUT,
        propagation: InputPropagation::Stop,
    };
    let mut tree = ready(&view);
    tree.render(Context::new()).unwrap();
    assert_eq!(
        tree.input(&mouse_event(5.0, 5.0), Context::new()),
        InputResult {
            update: UpdateResult::LAYOUT | UpdateResult::RENDER,
            propagation: InputPropagation::Stop,
        }
    );
    assert_invalidated(&tree, &[1, 2, 3]);
    assert!(tree.node_ref(find(&tree, 1)).unwrap().draw_cache.is_none());
    for key in [2, 3, 4] {
        assert!(
            tree.node_ref(find(&tree, key))
                .unwrap()
                .draw_cache
                .is_some()
        );
    }
}
