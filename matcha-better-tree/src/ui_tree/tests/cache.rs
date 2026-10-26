use super::*;
use crate::ui_tree::window::WindowRegistry;

fn measure_child(tree: &mut UiTree, root: UiId, child: ChildId, ctx: &mut Context) -> Bounds {
    Children {
        tree,
        current: root,
    }
    .measure(child, Constraints::default(), ctx)
    .unwrap()
}

#[test]
fn layout_invalidation_recomputes_measurement_and_layout_but_keeps_drawing() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ProbeView(Arc::clone(&observations)));
    let id = tree.node_ref(root).unwrap().child(child).unwrap();
    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    assert_eq!(measure_child(&mut tree, root, child, &mut context).x, 10.0);
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();

    tree.clear_caches(UpdateResult::LAYOUT);
    let mut context = context.reborrow().with_dpi(2.0);
    assert!(tree.node_ref(id).unwrap().cache.measure().is_none());
    assert!(matches!(
        tree.render(&mut context),
        Err(UiError::LayoutRequired)
    ));
    // Recomputing measurement must not make layout valid again.
    assert_eq!(measure_child(&mut tree, root, child, &mut context).x, 20.0);
    assert!(matches!(
        tree.render(&mut context),
        Err(UiError::LayoutRequired)
    ));
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    assert_eq!(measure_child(&mut tree, root, child, &mut context).x, 20.0);
    let observed = observations.lock().unwrap();
    assert_eq!(observed.measure_calls, 2);
    assert_eq!(observed.layout_calls, 2);
    assert_eq!(observed.render_calls, 1);
}

#[test]
fn clearing_selected_caches_preserves_other_results_and_placement() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ProbeView(Arc::clone(&observations)));
    let id = tree.node_ref(root).unwrap().child(child).unwrap();
    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    measure_child(&mut tree, root, child, &mut context);
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    let placement = tree.node_ref(id).unwrap().placement.unwrap();

    tree.clear_caches(UpdateResult::RENDER);
    assert!(
        tree.nodes
            .iter()
            .all(|(_, node)| node.cache.draw().is_none())
    );
    assert!(tree.node_ref(id).unwrap().cache.measure().is_some());
    assert_eq!(
        tree.node_ref(id).unwrap().placement.unwrap().affine,
        placement.affine
    );
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    assert_eq!(observations.lock().unwrap().layout_calls, 1);
    assert_eq!(observations.lock().unwrap().render_calls, 2);

    tree.clear_caches(UpdateResult::LAYOUT);
    assert!(
        tree.nodes
            .iter()
            .all(|(_, node)| node.cache.measure().is_none() && node.cache.layout().is_none())
    );
    assert!(tree.node_ref(id).unwrap().cache.draw().is_some());
    tree.clear_caches(UpdateResult::RENDER);
    tree.clear_caches(UpdateResult::empty());
    assert!(matches!(
        tree.render(&mut context),
        Err(UiError::LayoutRequired)
    ));
}

#[test]
fn failed_drawing_generation_is_retried_and_refresh_preserves_the_previous_value() {
    use crate::ui_tree::widget::Color;
    use render_interface::{MeshDescriptor, MeshSource, TextureDescriptor, TextureSource};

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
    let mut cache = NodeCache::default();
    for _ in 0..2 {
        assert!(matches!(
            cache.drawing(false, || conflicting.clone()),
            Err(UiError::Resource(_))
        ));
        assert!(!cache.drawn());
    }
    let previous_mesh = cache.drawing(false, || valid.clone()).unwrap().color[0]
        .mesh
        .id();
    assert_eq!(
        cache.drawing(false, || panic!("must reuse")).unwrap().color[0]
            .mesh
            .id(),
        previous_mesh
    );
    for _ in 0..2 {
        assert!(matches!(
            cache.drawing(true, || conflicting.clone()),
            Err(UiError::Resource(_))
        ));
        assert_eq!(cache.draw().unwrap().color[0].mesh.id(), previous_mesh);
    }
    let replaced = cache.drawing(true, || valid.clone()).unwrap();
    assert_ne!(replaced.color[0].mesh.id(), previous_mesh);
    cache.invalidate(UpdateResult::RENDER);
    assert!(!cache.drawn());
    assert!(cache.drawing(false, || conflicting).is_err());
    assert!(!cache.drawn());
    assert!(cache.drawing(false, || valid).is_ok());
}
