use super::*;
use crate::ui_tree::window::WindowRegistry;

fn measure_child(tree: &mut UiTree, child: UiId, ctx: &mut Context) -> Bounds {
    tree.measure_node(child, Constraints::default(), ctx)
}

#[test]
fn layout_invalidation_recomputes_measurement_and_layout_but_keeps_drawing() {
    let observations = Arc::default();
    let (mut tree, _, child) = setup(ProbeView(Arc::clone(&observations)));
    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    assert_eq!(measure_child(&mut tree, child, &mut context).x, 10.0);
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();

    tree.clear_caches(UpdateResult::LAYOUT);
    let mut context = context.reborrow().with_dpi(2.0);
    assert!(
        tree.node_ref(child)
            .unwrap()
            .measure_cache
            .value()
            .is_none()
    );
    assert!(matches!(
        tree.render(&mut context),
        Err(UiError::LayoutRequired)
    ));
    // Recomputing measurement must not make layout valid again.
    assert_eq!(measure_child(&mut tree, child, &mut context).x, 20.0);
    assert!(matches!(
        tree.render(&mut context),
        Err(UiError::LayoutRequired)
    ));
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    assert_eq!(measure_child(&mut tree, child, &mut context).x, 20.0);
    let observed = observations.lock().unwrap();
    assert_eq!(observed.measure_calls, 2);
    assert_eq!(observed.layout_calls, 2);
    assert_eq!(observed.render_calls, 1);
}

#[test]
fn clearing_selected_caches_preserves_other_results_and_placement() {
    let observations = Arc::default();
    let (mut tree, _, child) = setup(ProbeView(Arc::clone(&observations)));
    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    measure_child(&mut tree, child, &mut context);
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    let placement = tree.node_ref(child).unwrap().placement.unwrap();

    tree.clear_caches(UpdateResult::RENDER);
    assert!(
        tree.nodes
            .iter()
            .all(|(_, node)| node.render_cache.get(&()).is_none())
    );
    assert!(
        tree.node_ref(child)
            .unwrap()
            .measure_cache
            .value()
            .is_some()
    );
    assert_eq!(
        tree.node_ref(child).unwrap().placement.unwrap().affine,
        placement.affine
    );
    tree.layout(bounds(), &mut context);
    tree.render(&mut context).unwrap();
    assert_eq!(observations.lock().unwrap().layout_calls, 1);
    assert_eq!(observations.lock().unwrap().render_calls, 2);

    tree.clear_caches(UpdateResult::LAYOUT);
    assert!(tree.nodes.iter().all(
        |(_, node)| node.measure_cache.value().is_none() && node.layout_cache.value().is_none()
    ));
    assert!(
        tree.node_ref(child)
            .unwrap()
            .render_cache
            .get(&())
            .is_some()
    );
    tree.clear_caches(UpdateResult::RENDER);
    tree.clear_caches(UpdateResult::empty());
    assert!(matches!(
        tree.render(&mut context),
        Err(UiError::LayoutRequired)
    ));
}

#[test]
fn generic_cache_matches_keys_reuses_values_and_releases_replaced_entries() {
    use std::rc::Rc;

    #[derive(PartialEq)]
    struct Key(u8);
    struct Value;
    // Keys need neither Default nor Clone, and values need neither Default nor equality.
    let first = Rc::new(Value);
    let second = Rc::new(Value);
    let mut cache = Cache::<Key, Rc<Value>>::default();
    assert!(cache.get(&Key(1)).is_none());
    cache.store(Key(1), Rc::clone(&first));
    assert_eq!(Rc::strong_count(&first), 2);
    assert!(cache.get(&Key(2)).is_none());
    let reused = cache
        .get_or_try_insert_with(Key(1), false, || Err::<Rc<Value>, _>("must reuse"))
        .unwrap();
    assert!(Rc::ptr_eq(reused, &first));

    let replaced = cache
        .get_or_try_insert_with(Key(2), false, || Ok::<_, ()>(Rc::clone(&second)))
        .unwrap();
    assert!(Rc::ptr_eq(replaced, &second));
    assert!(cache.get(&Key(1)).is_none());
    assert_eq!(Rc::strong_count(&first), 1);
    assert_eq!(Rc::strong_count(&second), 2);
    let refreshed = cache
        .get_or_try_insert_with(Key(2), true, || Ok::<_, ()>(Rc::clone(&first)))
        .unwrap();
    assert!(Rc::ptr_eq(refreshed, &first));
    assert_eq!(Rc::strong_count(&second), 1);
    cache.invalidate();
    cache.invalidate();
    assert!(cache.get(&Key(2)).is_none());
    assert_eq!(Rc::strong_count(&first), 1);
}

#[test]
fn failed_generation_keeps_the_previous_key_and_value_and_can_be_retried() {
    let mut cache = Cache::<u32, String>::default();
    assert!(
        cache
            .get_or_try_insert_with(1, false, || Err::<String, _>("failed"))
            .is_err()
    );
    assert!(cache.value().is_none());
    cache.store(1, String::from("old"));

    // Both a changed key and a forced refresh must retain the original entry on failure.
    for (key, refresh) in [(2, false), (1, true), (2, false)] {
        assert!(
            cache
                .get_or_try_insert_with(key, refresh, || Err::<String, _>("failed"))
                .is_err()
        );
        assert_eq!(cache.get(&1).map(String::as_str), Some("old"));
        assert!(cache.get(&2).is_none());
    }
    assert_eq!(
        cache
            .get_or_try_insert_with(2, false, || Ok::<_, ()>(String::from("new")))
            .unwrap(),
        "new"
    );
    assert!(cache.get(&1).is_none());
    assert_eq!(cache.get(&2).map(String::as_str), Some("new"));
    cache.invalidate();
    assert!(
        cache
            .get_or_try_insert_with(2, true, || Err::<String, _>("failed"))
            .is_err()
    );
    assert!(cache.value().is_none());
}
