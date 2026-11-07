use super::*;
use crate::ui_tree::env_reads::{EnvReads, ReadPhase};
use crate::ui_tree::window::{Window, WindowRegistry};
use std::time::Instant;

const PHASES: [ReadPhase; 5] = [
    ReadPhase::Build,
    ReadPhase::Update,
    ReadPhase::Measure,
    ReadPhase::Layout,
    ReadPhase::Render,
];

fn read(flags: EnvReads, ctx: &mut Context) {
    // Helpers may freely reborrow the callback's context without losing attribution.
    let mut nested = ctx.reborrow();
    if flags.contains(EnvReads::DPI) {
        let _ = nested.scale_factor();
    }
    if flags.contains(EnvReads::VIEWPORT_SIZE) {
        let _ = nested.viewport_size();
    }
    if flags.contains(EnvReads::FRAME_TIME) {
        let _ = nested.frame_time();
    }
}

#[derive(Clone)]
struct ReadView {
    key: u64,
    reads: [EnvReads; 5],
    children: Vec<ReadView>,
    draw: Draw,
    backdrop: bool,
}

impl ReadView {
    fn new(key: u64, reads: [EnvReads; 5], children: Vec<Self>) -> Self {
        Self {
            key,
            reads,
            children,
            draw: Draw::default(),
            backdrop: false,
        }
    }
}

struct ReadWidget {
    children: Vec<ChildId>,
    reads: [EnvReads; 5],
    draw: Draw,
    backdrop: bool,
}

impl View for ReadView {
    fn label(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> Option<u64> {
        Some(self.key)
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        read(self.reads[0], ctx.context());
        let children = self.children.iter().map(|child| ctx.child(child)).collect();
        ReadWidget {
            children,
            reads: self.reads,
            draw: self.draw.clone(),
            backdrop: self.backdrop,
        }
    }
}

impl Widget for ReadWidget {
    type V = ReadView;
    fn update<'a>(&mut self, view: &'a ReadView, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        self.reads = view.reads;
        self.children = view.children.iter().map(|child| ctx.child(child)).collect();
        self.draw = view.draw.clone();
        self.backdrop = view.backdrop;
        read(self.reads[1], ctx.context());
        // Changing which getters this fixture reads must not change cache policy.
        UpdateResult::empty()
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, ctx: &mut Context) -> InputResult {
        read(EnvReads::all(), ctx);
        InputResult::default()
    }
    fn is_inside(&self, _: Bounds, _: Point, ctx: &mut Context) -> bool {
        read(EnvReads::all(), ctx);
        true
    }
    fn measure(
        &self,
        constraints: Constraints,
        mut children: Children<'_>,
        ctx: &mut Context,
    ) -> Bounds {
        read(self.reads[2] & !EnvReads::FRAME_TIME, ctx);
        for &child in &self.children {
            children.measure(child, constraints, ctx).unwrap();
        }
        read(self.reads[2] & EnvReads::FRAME_TIME, ctx);
        bounds()
    }
    fn layout(&self, _: Bounds, mut children: Children<'_>, ctx: &mut Context) {
        read(self.reads[3] & !EnvReads::FRAME_TIME, ctx);
        for &child in &self.children {
            children
                .measure(child, Constraints::default(), ctx)
                .unwrap();
            children.layout(child, position(Matrix4::identity()), ctx);
        }
        read(self.reads[3] & EnvReads::FRAME_TIME, ctx);
    }
    fn uses_backdrop(&self) -> bool {
        self.backdrop
    }
    fn render(&self, _: Bounds, ctx: &mut Context) -> Draw {
        read(self.reads[4], ctx);
        self.draw.clone()
    }
}

fn find(tree: &UiTree, key: u64) -> UiId {
    tree.nodes
        .iter()
        .find_map(|(id, node)| (node.key == UiKey::Key(key)).then_some(id))
        .unwrap()
}

fn assert_reads(tree: &UiTree, view: &ReadView) {
    let observed = &tree.node_ref(find(tree, view.key)).unwrap().env_reads;
    for (index, phase) in PHASES.into_iter().enumerate() {
        assert_eq!(
            observed.get(phase),
            Some(view.reads[index]),
            "key {}, phase {index}",
            view.key
        );
    }
    for child in &view.children {
        assert_reads(tree, child);
    }
}

#[test]
fn getters_share_the_callback_recorder_across_reborrows_and_isolate_nested_scopes() {
    let mut registry = WindowRegistry::default();
    let time = Instant::now();
    let mut parent = Context::new(&mut registry)
        .with_dpi(2.0)
        .with_viewport_size([120.0, 80.0])
        .with_frame_time(time);
    let (dpi, reads) = parent.observe_env_reads(|ctx| ctx.dpi());
    assert_eq!(dpi, 2.0);
    assert_eq!(reads, EnvReads::DPI);
    let (child_reads, reads) = parent.observe_env_reads(|ctx| {
        assert_eq!(ctx.scale_factor(), 2.0);
        let (frame_time, child_reads) = ctx.observe_env_reads(|child| {
            let frame_time = child.reborrow().frame_time();
            read(EnvReads::all(), &mut child.unobserved());
            frame_time
        });
        assert_eq!(frame_time, time);
        // Reading through the parent again resumes its exclusive recorder borrow.
        assert_eq!(ctx.reborrow().viewport_size(), [120.0, 80.0]);
        child_reads
    });
    assert_eq!(reads, EnvReads::DPI | EnvReads::VIEWPORT_SIZE);
    assert_eq!(child_reads, EnvReads::FRAME_TIME);
    let (resource, reads) = parent.observe_env_reads(|ctx| {
        ctx.insert_resource(42_u32);
        let resource = ctx.resource::<u32>();
        let _ = ctx.cursor_icon();
        let _ = ctx.is_ime_allowed();
        let _ = ctx.ime_cursor_area();
        let _ = ctx.window_registry();
        resource
    });
    assert_eq!(reads, EnvReads::empty());
    assert_eq!(*resource.unwrap(), 42);
    assert_eq!(*parent.resource::<u32>().unwrap(), 42);
    read(EnvReads::all(), &mut parent);
    let ((), reads) = parent.observe_env_reads(|_| ());
    assert_eq!(reads, EnvReads::empty());
}

#[test]
fn observation_keeps_parent_state_after_unwind_and_returns_reads_with_error_results() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    let ((), reads) = context.observe_env_reads(|parent| {
        let _ = parent.dpi();
        let failure = catch_unwind(AssertUnwindSafe(|| {
            let _ = parent.observe_env_reads(|child| {
                let _ = child.frame_time();
                child.insert_resource(17_u32);
                panic!("callback failed");
            });
        }));
        assert!(failure.is_err());
        let _ = parent.reborrow().viewport_size();
    });
    assert_eq!(reads, EnvReads::DPI | EnvReads::VIEWPORT_SIZE);
    // Observation isolates the recorder, not service side effects or the rest of the UI tree.
    assert_eq!(*context.resource::<u32>().unwrap(), 17);
    let (result, reads) = context.observe_env_reads(|ctx| {
        let _ = ctx.frame_time();
        Err::<(), _>("domain error")
    });
    assert_eq!(result, Err("domain error"));
    assert_eq!(reads, EnvReads::FRAME_TIME);
    let ((), reads) = context.observe_env_reads(|_| ());
    assert_eq!(reads, EnvReads::empty());
}

#[test]
fn phases_and_nodes_record_only_their_direct_reads_including_cached_child_calls() {
    use EnvReads as E;
    let view = ReadView::new(
        1,
        [
            E::DPI,
            E::VIEWPORT_SIZE,
            E::FRAME_TIME,
            E::DPI | E::FRAME_TIME,
            E::FRAME_TIME,
        ],
        vec![
            ReadView::new(
                2,
                [
                    E::VIEWPORT_SIZE,
                    E::FRAME_TIME,
                    E::DPI,
                    E::VIEWPORT_SIZE,
                    E::DPI,
                ],
                vec![ReadView::new(
                    3,
                    [
                        E::FRAME_TIME,
                        E::DPI,
                        E::VIEWPORT_SIZE,
                        E::FRAME_TIME,
                        E::VIEWPORT_SIZE,
                    ],
                    vec![],
                )],
            ),
            ReadView::new(4, [E::empty(); 5], vec![]),
        ],
    );
    let wrapper = ContainerView {
        child: view.clone(),
        child_position: position(Matrix4::identity()),
    };
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let mut tree = UiTree::new();
    tree.update(&wrapper, &mut ctx);
    let id = find(&tree, 4);
    assert_eq!(
        tree.node_ref(id).unwrap().env_reads.get(ReadPhase::Build),
        Some(E::empty())
    );
    assert_eq!(
        tree.node_ref(id).unwrap().env_reads.get(ReadPhase::Update),
        None
    );
    assert_eq!(
        tree.node_ref(id).unwrap().env_reads.get(ReadPhase::Layout),
        None
    );
    tree.update(&wrapper, &mut ctx);
    let root = tree.root.unwrap();
    let child = tree.node_ref(root).unwrap().children[0].0;
    Children {
        tree: &mut tree,
        current: root,
    }
    .measure(child, Constraints::default(), &mut ctx)
    .unwrap();
    // Layout will encounter already-cached child measurements; they must not become parent reads.
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    assert_reads(&tree, &view);
    let _ = tree.input(&mouse_event(5.0, 5.0), &mut ctx);
    assert_reads(&tree, &view);
}

#[test]
fn skipped_callbacks_keep_observations_and_executed_callbacks_replace_them() {
    let mut wrapper = ContainerView {
        child: ReadView::new(1, [EnvReads::all(); 5], vec![]),
        child_position: position(Matrix4::identity()),
    };
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let mut tree = UiTree::new();
    tree.update(&wrapper, &mut ctx);
    let root = tree.root.unwrap();
    let child = tree.node_ref(root).unwrap().children[0].0;
    let id = find(&tree, 1);
    Children {
        tree: &mut tree,
        current: root,
    }
    .measure(child, Constraints::default(), &mut ctx)
    .unwrap();
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    wrapper.child.reads = [EnvReads::empty(); 5];
    tree.update(&wrapper, &mut ctx);
    Children {
        tree: &mut tree,
        current: root,
    }
    .measure(child, Constraints::default(), &mut ctx)
    .unwrap();
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    let record = tree.node_ref(id).unwrap().env_reads;
    assert_eq!(record.get(ReadPhase::Build), Some(EnvReads::all()));
    assert_eq!(record.get(ReadPhase::Update), Some(EnvReads::empty()));
    for phase in [ReadPhase::Measure, ReadPhase::Layout, ReadPhase::Render] {
        assert_eq!(record.get(phase), Some(EnvReads::all()));
    }
    tree.clear_caches(UpdateResult::LAYOUT | UpdateResult::RENDER);
    assert_eq!(tree.node_ref(id).unwrap().env_reads, record);
    Children {
        tree: &mut tree,
        current: root,
    }
    .measure(child, Constraints::default(), &mut ctx)
    .unwrap();
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    let record = tree.node_ref(id).unwrap().env_reads;
    for phase in [ReadPhase::Measure, ReadPhase::Layout, ReadPhase::Render] {
        assert_eq!(record.get(phase), Some(EnvReads::empty()));
    }
    assert_eq!(record.get(ReadPhase::Build), Some(EnvReads::all()));
}

#[test]
fn reordering_keeps_node_observations_and_replacement_starts_a_new_record() {
    let mut view = ReadView::new(
        1,
        [EnvReads::DPI; 5],
        vec![
            ReadView::new(2, [EnvReads::VIEWPORT_SIZE; 5], vec![]),
            ReadView::new(3, [EnvReads::FRAME_TIME; 5], vec![]),
        ],
    );
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let mut tree = UiTree::new();
    tree.update(&view, &mut ctx);
    tree.update(&view, &mut ctx);
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    let a = find(&tree, 2);
    let b = find(&tree, 3);
    let records = [
        tree.node_ref(a).unwrap().env_reads,
        tree.node_ref(b).unwrap().env_reads,
    ];
    view.children.reverse();
    tree.update(&view, &mut ctx);
    assert_eq!((find(&tree, 2), find(&tree, 3)), (a, b));
    assert_eq!(
        [
            tree.node_ref(a).unwrap().env_reads,
            tree.node_ref(b).unwrap().env_reads
        ],
        records
    );

    let root = tree.root.unwrap();
    tree.update(&ProbeView(Arc::default()), &mut ctx);
    assert_eq!(tree.root.unwrap(), root);
    let record = &tree.node_ref(root).unwrap().env_reads;
    assert_eq!(
        record.get(ReadPhase::Build),
        Some(EnvReads::DPI | EnvReads::VIEWPORT_SIZE)
    );
    for phase in [
        ReadPhase::Update,
        ReadPhase::Measure,
        ReadPhase::Layout,
        ReadPhase::Render,
    ] {
        assert_eq!(record.get(phase), None);
    }
}

#[test]
fn window_setup_and_tree_bookkeeping_do_not_add_widget_reads() {
    use matcha_window::headless_interface::HeadlessEventLoop;
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let view = Window::new(ContainerView {
        child: Window::new(ReadView::new(1, [EnvReads::DPI; 5], vec![])),
        child_position: position(Matrix4::identity()),
    });
    let mut tree = UiTree::new();
    let ((), outer_reads) = ctx.observe_env_reads(|ctx| {
        tree.update(&view, ctx);
        tree.update(&view, ctx);
        ctx.window_registry()
            .create_pending(
                &HeadlessEventLoop::new(),
                &matcha_window::window::WindowConfig::default().surface_config,
            )
            .unwrap();
        let outer = tree.root.unwrap();
        let container = tree.node_ref(outer).unwrap().children[0].1;
        let inner = tree.node_ref(container).unwrap().children[0].1;
        for node in [outer, inner] {
            let id = tree.node_ref(node).unwrap().window.unwrap().id();
            let native = ctx
                .window_registry()
                .get_mut(id)
                .unwrap()
                .window_id()
                .unwrap();
            tree.layout_window(native, ctx);
            tree.render_window(native, ctx).unwrap();
        }
    });
    let outer = tree.root.unwrap();
    let container = tree.node_ref(outer).unwrap().children[0].1;
    let inner = tree.node_ref(container).unwrap().children[0].1;
    for node in [outer, inner, container] {
        let observed = &tree.node_ref(node).unwrap().env_reads;
        for phase in PHASES {
            assert!(observed.get(phase).is_none_or(|reads| reads.is_empty()));
        }
    }
    let leaf = tree.node_ref(find(&tree, 1)).unwrap();
    assert_eq!(leaf.env_reads.get(ReadPhase::Layout), Some(EnvReads::DPI));
    assert_eq!(leaf.env_reads.get(ReadPhase::Render), Some(EnvReads::DPI));
    assert_eq!(outer_reads, EnvReads::empty());
}

#[test]
fn completed_render_reads_are_recorded_even_when_resource_preparation_fails() {
    use crate::ui_tree::widget::Color;
    use render_interface::{MeshDescriptor, MeshSource, TextureDescriptor, TextureSource};

    let mesh = MeshSource::new(MeshDescriptor::triangles(3, 0), |_| Ok(()));
    let texture = TextureSource::new(
        TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
        |_| Ok(()),
    );
    let mut view = ReadView::new(1, [EnvReads::DPI; 5], vec![]);
    view.backdrop = true;
    view.draw.color.push(Color {
        mesh: mesh.clone(),
        texture,
        transform: Matrix4::identity(),
        opacity: 1.0,
    });
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let mut tree = UiTree::new();
    tree.update(&view, &mut ctx);
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    let root = tree.root.unwrap();
    let old_mesh = tree
        .node_ref(root)
        .unwrap()
        .render_cache
        .get(&())
        .unwrap()
        .color[0]
        .mesh
        .id();

    let mut conflicting = view.draw.color[0].clone();
    conflicting.mesh = MeshSource::with_id(mesh.id(), MeshDescriptor::triangles(6, 0), |_| Ok(()));
    view.draw.color.push(conflicting);
    view.reads[4] = EnvReads::VIEWPORT_SIZE;
    tree.update(&view, &mut ctx);
    for _ in 0..2 {
        assert!(matches!(tree.render(&mut ctx), Err(UiError::Resource(_))));
        let node = tree.node_ref(root).unwrap();
        assert_eq!(
            node.env_reads.get(ReadPhase::Render),
            Some(EnvReads::VIEWPORT_SIZE)
        );
        assert_eq!(
            node.render_cache.get(&()).unwrap().color[0].mesh.id(),
            old_mesh
        );
    }
    view.draw.color.pop();
    view.reads[4] = EnvReads::empty();
    tree.update(&view, &mut ctx);
    tree.render(&mut ctx).unwrap();
    assert_eq!(
        tree.node_ref(root)
            .unwrap()
            .env_reads
            .get(ReadPhase::Render),
        Some(EnvReads::empty())
    );
}
