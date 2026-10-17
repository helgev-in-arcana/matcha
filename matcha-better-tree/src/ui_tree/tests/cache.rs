use super::*;

#[test]
fn layout_invalidation_is_lazy_and_does_not_resurrect_an_unmeasured_cache() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ProbeView(Arc::clone(&observations)));
    let child_id = tree.node_ref(root).unwrap().child(child).unwrap();
    assert_eq!(
        Children {
            tree: &mut tree,
            current: root
        }
        .measure(child, Constraints::default(), Context::new())
        .unwrap()
        .x,
        10.0
    );
    tree.layout(bounds(), Context::new());
    tree.render(Context::new()).unwrap();
    let before = tree.node_ref(child_id).unwrap().cache_epochs;
    tree.invalidate_caches(UpdateResult::LAYOUT);
    // The invalidation request changes no node and executes no callbacks.
    assert_eq!(tree.node_ref(child_id).unwrap().cache_epochs, before);
    assert!(tree.node_ref(child_id).unwrap().measure_cache.is_some());
    assert_eq!(observations.lock().unwrap().layout_calls, 1);
    assert!(matches!(
        tree.render(Context::new()),
        Err(UiError::LayoutRequired)
    ));

    // ContainerWidget's layout visits the child's layout but does not measure it.
    let new_context = Context::new().with_dpi(2.0);
    tree.layout(bounds(), new_context.clone());
    assert_eq!(observations.lock().unwrap().layout_calls, 2);
    assert!(tree.node_ref(child_id).unwrap().measure_cache.is_none());
    assert!(tree.node_ref(child_id).unwrap().draw_cache.is_some());
    let mut children = Children {
        tree: &mut tree,
        current: root,
    };
    for _ in 0..2 {
        assert_eq!(
            children
                .measure(child, Constraints::default(), new_context.clone())
                .unwrap()
                .x,
            20.0
        );
    }
    assert_eq!(observations.lock().unwrap().measure_calls, 2);
}

#[test]
fn measuring_in_a_new_epoch_keeps_layout_invalid_until_it_runs() {
    let observations = Arc::default();
    let (mut tree, root, child) = setup(ProbeView(Arc::clone(&observations)));
    let child_id = tree.node_ref(root).unwrap().child(child).unwrap();
    tree.layout(bounds(), Context::new());
    Children {
        tree: &mut tree,
        current: root,
    }
    .measure(child, Constraints::default(), Context::new())
    .unwrap();
    tree.invalidate_caches(UpdateResult::LAYOUT);
    let context = Context::new().with_dpi(2.0);
    assert_eq!(
        Children {
            tree: &mut tree,
            current: root
        }
        .measure(child, Constraints::default(), context.clone())
        .unwrap()
        .x,
        20.0
    );
    let node = tree.node_ref(child_id).unwrap();
    assert_eq!(node.cache_epochs.layout, tree.cache_epochs.layout);
    assert!(node.layout_cache.is_none());
    assert!(matches!(
        tree.render(context.clone()),
        Err(UiError::LayoutRequired)
    ));
    tree.layout(bounds(), context.clone());
    Children {
        tree: &mut tree,
        current: root,
    }
    .measure(child, Constraints::default(), context.clone())
    .unwrap();
    assert_eq!(observations.lock().unwrap().layout_calls, 2);
    assert_eq!(observations.lock().unwrap().measure_calls, 2);
    tree.render(context).unwrap();
}

#[test]
fn eager_clear_drops_only_selected_caches_and_keeps_placement() {
    let (mut tree, root, child) = setup(ProbeView(Arc::default()));
    tree.layout(bounds(), Context::new());
    Children {
        tree: &mut tree,
        current: root,
    }
    .measure(child, Constraints::default(), Context::new())
    .unwrap();
    tree.render(Context::new()).unwrap();
    let child_id = tree.node_ref(root).unwrap().child(child).unwrap();
    let placement = tree.node_ref(child_id).unwrap().placement.unwrap();
    tree.clear_caches(UpdateResult::RENDER);
    for (_, node) in tree.nodes.iter() {
        assert!(node.draw_cache.is_none());
        assert!(node.layout_cache.is_some());
        assert!(node.placement.is_some());
    }
    assert!(tree.node_ref(child_id).unwrap().measure_cache.is_some());
    tree.render(Context::new()).unwrap();
    tree.clear_caches(UpdateResult::LAYOUT);
    for (_, node) in tree.nodes.iter() {
        assert!(node.draw_cache.is_some());
        assert!(node.measure_cache.is_none());
        assert!(node.layout_cache.is_none());
    }
    assert_eq!(
        tree.node_ref(child_id).unwrap().placement.unwrap().affine,
        placement.affine
    );
    tree.clear_caches(UpdateResult::RENDER);
    assert!(tree.nodes.iter().all(|(_, node)| node.draw_cache.is_none()));
}

#[test]
fn clearing_one_group_preserves_pending_invalidation_of_the_other() {
    let (mut tree, root, child) = setup(ProbeView(Arc::default()));
    tree.layout(bounds(), Context::new());
    tree.render(Context::new()).unwrap();
    let child_id = tree.node_ref(root).unwrap().child(child).unwrap();
    let old_layout_epoch = tree.node_ref(child_id).unwrap().cache_epochs.layout;
    tree.invalidate_caches(UpdateResult::LAYOUT | UpdateResult::RENDER);
    tree.clear_caches(UpdateResult::RENDER);
    assert_eq!(
        tree.node_ref(child_id).unwrap().cache_epochs.layout,
        old_layout_epoch
    );
    assert!(matches!(
        tree.render(Context::new()),
        Err(UiError::LayoutRequired)
    ));
    tree.layout(bounds(), Context::new());
    tree.render(Context::new()).unwrap();
    assert_eq!(
        tree.node_ref(child_id).unwrap().cache_epochs,
        tree.cache_epochs
    );
}
