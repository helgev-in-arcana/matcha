use super::*;
use crate::ui_tree::cache_epoch::Cached;
use crate::{CacheEpoch, ui_tree::window::WindowRegistry};

fn measure_child(tree: &mut UiTree, root: UiId, child: ChildId, ctx: &mut Context) -> Bounds {
    Children {
        tree,
        current: root,
    }
    .measure(child, Constraints::default(), ctx)
    .unwrap()
}

#[test]
fn layout_epoch_is_lazy_and_each_cache_records_only_actual_recomputation() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ProbeView(Arc::clone(&observations)));
    let id = tree.node_ref(root).unwrap().child(child).unwrap();
    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    let mut epoch = context.cache_epoch();
    measure_child(&mut tree, root, child, &mut context);
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    let before = tree.node_ref(id).unwrap().cache.measure().stamp();
    epoch.advance(UpdateResult::LAYOUT);
    let mut context = context.with_cache_epoch(epoch).with_dpi(2.0);
    assert_eq!(tree.node_ref(id).unwrap().cache.measure().stamp(), before);
    assert!(matches!(
        tree.render(&mut context),
        Err(UiError::LayoutRequired)
    ));
    // Layout does not measure: retain the old value and stamp, but reject it for reuse.
    tree.layout(bounds(), &mut context);
    let node = tree.node_ref(id).unwrap();
    assert_eq!(node.cache.measure().stamp(), before);
    assert_eq!(node.cache.layout().stamp(), Some(epoch.stamp()));
    assert_eq!(node.cache.draw().stamp(), before);
    assert_eq!(measure_child(&mut tree, root, child, &mut context).x, 20.0);
    assert_eq!(measure_child(&mut tree, root, child, &mut context).x, 20.0);
    tree.render(&mut context).unwrap();
    assert_eq!(tree.node_ref(id).unwrap().cache.draw().stamp(), before);
    assert_eq!(observations.lock().unwrap().measure_calls, 2);
    assert_eq!(observations.lock().unwrap().layout_calls, 2);
}

#[test]
fn measurement_cannot_make_old_layout_valid() {
    let (mut tree, root, child) = setup(ProbeView(Arc::default()));
    let id = tree.node_ref(root).unwrap().child(child).unwrap();
    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    tree.layout(bounds(), &mut context);
    let old_layout = tree.node_ref(id).unwrap().cache.layout().stamp();
    let mut epoch = context.cache_epoch();
    epoch.advance(UpdateResult::LAYOUT);
    let mut context = context.with_cache_epoch(epoch);
    measure_child(&mut tree, root, child, &mut context);
    assert_eq!(
        tree.node_ref(id).unwrap().cache.measure().stamp(),
        Some(epoch.stamp())
    );
    assert_eq!(
        tree.node_ref(id).unwrap().cache.layout().stamp(),
        old_layout
    );
    assert!(!tree.node_ref(id).unwrap().cache.laid_out(bounds(), epoch));
    assert!(matches!(
        tree.render(&mut context),
        Err(UiError::LayoutRequired)
    ));
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
}

#[test]
fn render_only_epoch_reuses_older_layout_without_restamping_it() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ProbeView(Arc::clone(&observations)));
    let id = tree.node_ref(root).unwrap().child(child).unwrap();
    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    let old_layout = tree.node_ref(id).unwrap().cache.layout().stamp();
    let mut epoch = context.cache_epoch();
    epoch.advance(UpdateResult::RENDER);
    epoch.advance(UpdateResult::empty());
    let mut context = context.with_cache_epoch(epoch);
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    assert_eq!(observations.lock().unwrap().layout_calls, 1);
    assert_eq!(
        tree.node_ref(id).unwrap().cache.layout().stamp(),
        old_layout
    );
    assert_eq!(
        tree.node_ref(id).unwrap().cache.draw().stamp(),
        Some(epoch.stamp())
    );
}

#[test]
fn clearing_one_group_preserves_other_groups_pending_invalidation() {
    let (mut tree, root, child) = setup(ProbeView(Arc::default()));
    let id = tree.node_ref(root).unwrap().child(child).unwrap();
    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    measure_child(&mut tree, root, child, &mut context);
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    let placement = tree.node_ref(id).unwrap().placement.unwrap();
    let mut epoch = context.cache_epoch();
    epoch.advance(UpdateResult::LAYOUT | UpdateResult::RENDER);
    tree.clear_caches(UpdateResult::RENDER);
    assert!(
        tree.nodes
            .iter()
            .all(|(_, node)| node.cache.draw().value().is_none())
    );
    assert!(tree.node_ref(id).unwrap().cache.measure().value().is_some());
    assert_eq!(
        tree.node_ref(id).unwrap().placement.unwrap().affine,
        placement.affine
    );
    let mut context = context.with_cache_epoch(epoch);
    assert!(matches!(
        tree.render(&mut context),
        Err(UiError::LayoutRequired)
    ));
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    tree.clear_caches(UpdateResult::LAYOUT);
    assert!(
        tree.nodes
            .iter()
            .all(|(_, node)| node.cache.measure().value().is_none()
                && node.cache.layout().value().is_none())
    );
    assert!(tree.node_ref(id).unwrap().cache.draw().value().is_some());
}

#[test]
fn domains_and_future_results_are_not_interchangeable() {
    let original = CacheEpoch::new();
    let other = CacheEpoch::new();
    let mut latest = original;
    latest.advance(UpdateResult::empty());
    let cache = Cached::new(42, original);
    assert_eq!(
        cache.get(latest, crate::ui_tree::cache_epoch::CacheGroup::Render),
        Some(&42)
    );
    assert_eq!(
        cache.get(other, crate::ui_tree::cache_epoch::CacheGroup::Render),
        None
    );
    let future = Cached::new(43, latest);
    assert_eq!(
        future.get(original, crate::ui_tree::cache_epoch::CacheGroup::Render),
        None
    );
}

#[test]
fn local_invalidation_releases_only_the_selected_cache() {
    let (mut tree, root, child) = setup(ProbeView(Arc::default()));
    let id = tree.node_ref(root).unwrap().child(child).unwrap();
    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    let node = tree.node_ref_mut(id).unwrap();
    assert!(node.cache.draw().value().is_some());
    node.cache.invalidate(UpdateResult::RENDER);
    assert!(node.cache.draw().value().is_none());
    assert_eq!(node.cache.draw().stamp(), None);
    assert!(!node.cache.drawn(context.cache_epoch()));
    assert!(node.cache.laid_out(bounds(), context.cache_epoch()));
    tree.render(&mut context).unwrap();
    assert!(
        tree.node_ref(id)
            .unwrap()
            .cache
            .drawn(context.cache_epoch())
    );
}

#[test]
fn failed_drawing_replacement_retains_the_previous_value_and_creation_stamp() {
    use crate::ui_tree::widget::Color;
    use render_interface::{MeshDescriptor, MeshSource, TextureDescriptor, TextureSource};

    struct DrawView(Draw);
    struct DrawWidget(Draw);
    impl View for DrawView {
        fn label(&self) -> Option<&str> {
            None
        }

        fn build<'a>(&'a self, _: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
            DrawWidget(self.0.clone())
        }
    }
    impl Widget for DrawWidget {
        type V = DrawView;

        fn update<'a>(&mut self, view: &'a DrawView, _: &mut DeclContext<'a, '_>) -> UpdateResult {
            self.0 = view.0.clone();
            // This fixture invalidates drawing through the epoch below, retaining the old entry.
            UpdateResult::empty()
        }

        fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
            InputResult::default()
        }

        fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
            bounds()
        }

        fn layout(&self, _: Bounds, _: Children<'_>, _: &mut Context) {}

        fn render(&self, _: Bounds, _: &mut Context) -> Draw {
            self.0.clone()
        }
    }

    let mesh = MeshSource::new(MeshDescriptor::triangles(3, 0), |_| Ok(()));
    let texture = TextureSource::new(
        TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
        |_| Ok(()),
    );
    let valid = Draw {
        color: vec![Color {
            mesh: mesh.clone(),
            texture,
            transform: Matrix4::identity(),
            opacity: 1.0,
        }]
        .into(),
        mask: None,
    };
    let mut conflicting = valid.clone();
    let mut second = conflicting.color[0].clone();
    second.mesh = MeshSource::with_id(mesh.id(), MeshDescriptor::triangles(6, 0), |_| Ok(()));
    conflicting.color.push(second);

    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    let mut tree = UiTree::new();
    tree.update(&DrawView(valid.clone()), &mut context);
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    let id = tree.root.unwrap();
    let previous = tree.node_ref(id).unwrap().cache.draw();
    let previous_stamp = previous.stamp();
    let previous_mesh = previous.value().unwrap().color[0].mesh.id();

    let mut epoch = context.cache_epoch();
    epoch.advance(UpdateResult::RENDER);
    let mut context = context.with_cache_epoch(epoch);
    tree.update(&DrawView(conflicting), &mut context);
    for _ in 0..2 {
        assert!(matches!(
            tree.render(&mut context),
            Err(UiError::Resource(_))
        ));
        let node = tree.node_ref(id).unwrap();
        let retained = node.cache.draw();
        assert_eq!(retained.stamp(), previous_stamp);
        assert_eq!(retained.value().unwrap().color.len(), 1);
        assert_eq!(retained.value().unwrap().color[0].mesh.id(), previous_mesh);
        assert!(!node.cache.drawn(epoch));
    }

    tree.update(&DrawView(valid), &mut context);
    tree.render(&mut context).unwrap();
    let replaced = tree.node_ref(id).unwrap().cache.draw();
    assert_eq!(replaced.stamp(), Some(epoch.stamp()));
    assert_ne!(replaced.value().unwrap().color[0].mesh.id(), previous_mesh);
    assert!(tree.node_ref(id).unwrap().cache.drawn(epoch));
}
