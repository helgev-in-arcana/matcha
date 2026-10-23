use std::collections::HashMap;

use matcha_window::headless_interface::HeadlessEventLoop;
use matcha_window::window::{CursorIcon, Size, WindowConfig as NativeConfig};

use super::*;
use crate::ui_tree::window::{Apply, Window, WindowConfig, WindowRegistry};

fn create(registry: &mut WindowRegistry, event_loop: &HeadlessEventLoop) {
    registry
        .create_pending(event_loop, &NativeConfig::default().surface_config)
        .unwrap();
}

fn probe() -> ProbeView {
    ProbeView(Arc::new(Mutex::new(Observations::default())))
}

fn config(title: &str, size: Option<Apply<Size>>) -> WindowConfig {
    WindowConfig {
        title: title.into(),
        inner_size: size,
        ..WindowConfig::default()
    }
}

#[test]
fn window_content_is_reconciled_in_the_same_arena() {
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    update(
        &mut tree,
        &mut registry,
        &Window::new(probe()).with_label("main"),
    );
    let root = tree.root.unwrap();
    let child = tree.node_ref(root).unwrap().children[0];
    assert_eq!(tree.nodes.iter().count(), 2);
    assert_eq!(tree.node_ref(root).unwrap().label.as_deref(), Some("main"));
    assert_eq!(tree.node_ref(child.1).unwrap().parent, Some(root));

    let id = registry.ids()[0];
    assert_eq!(
        tree.node_ref(root).unwrap().window.map(|w| w.id()),
        Some(id)
    );
    assert!(registry.get_mut(id).unwrap().window_id().is_none());
    create(&mut registry, &HeadlessEventLoop::new());
    let native = registry.get_mut(id).unwrap().window_id().unwrap();
    assert_eq!(registry.id_for_window(native), Some(id));
    assert_eq!(registry.location(native), Some(root));

    let updated = Arc::new(Mutex::new(Observations::default()));
    update(
        &mut tree,
        &mut registry,
        &Window::new(ProbeView(updated.clone())).with_label("renamed"),
    );
    assert_eq!(tree.node_ref(root).unwrap().children[0], child);
    assert_eq!(registry.id_for_window(native), Some(id));
    let _ = tree
        .node_ref_mut(child.1)
        .unwrap()
        .widget
        .as_mut()
        .unwrap()
        .input(
            bounds(),
            &mouse_event(5.0, 5.0),
            &mut Context::new(&mut registry),
        );
    assert_eq!(updated.lock().unwrap().events.len(), 1);
}

#[test]
fn pending_windows_use_latest_init_values_and_are_created_once() {
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    let view =
        |title, size| Window::new(probe()).with_config(config(title, Some(Apply::Init(size))));
    update(&mut tree, &mut registry, &view("first", [200, 100].into()));
    let id = registry.ids()[0];
    update(&mut tree, &mut registry, &view("latest", [400, 300].into()));
    let event_loop = HeadlessEventLoop::new();
    create(&mut registry, &event_loop);
    create(&mut registry, &event_loop);
    assert_eq!(event_loop.created_window_ids().len(), 1);
    let instance = registry.get_mut(id).unwrap();
    assert_eq!(instance.window().unwrap().title(), "latest");
    assert_eq!(instance.window().unwrap().inner_size(), [400, 300]);
    assert_eq!(
        tree.node_ref(tree.root.unwrap())
            .unwrap()
            .scoped_context(&mut Context::new(&mut registry))
            .viewport_size(),
        [400.0, 300.0]
    );
}

#[test]
fn init_preserves_native_changes_and_always_reapplies_unchanged_values() {
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    let view = |title, size| Window::new(probe()).with_config(config(title, size));
    update(
        &mut tree,
        &mut registry,
        &view("initial", Some(Apply::Init([200, 100].into()))),
    );
    create(&mut registry, &HeadlessEventLoop::new());
    let id = registry.ids()[0];
    registry
        .get_mut(id)
        .unwrap()
        .window()
        .unwrap()
        .request_inner_size(900, 800);
    update(
        &mut tree,
        &mut registry,
        &view("updated", Some(Apply::Init([400, 300].into()))),
    );
    assert_eq!(
        registry.get_mut(id).unwrap().window().unwrap().inner_size(),
        [900, 800]
    );
    assert_eq!(
        registry.get_mut(id).unwrap().window().unwrap().title(),
        "updated"
    );
    let controlled = Some(Apply::Always(Size::Logical {
        width: 320.0,
        height: 240.0,
    }));
    update(&mut tree, &mut registry, &view("updated", controlled));
    assert_eq!(
        registry.get_mut(id).unwrap().window().unwrap().inner_size(),
        [320, 240]
    );
    registry
        .get_mut(id)
        .unwrap()
        .window()
        .unwrap()
        .request_inner_size(900, 800);
    update(&mut tree, &mut registry, &view("updated", controlled));
    assert_eq!(
        registry.get_mut(id).unwrap().window().unwrap().inner_size(),
        [320, 240]
    );
    registry
        .get_mut(id)
        .unwrap()
        .window()
        .unwrap()
        .request_inner_size(600, 500);
    update(&mut tree, &mut registry, &view("unspecified", None));
    assert_eq!(
        registry.get_mut(id).unwrap().window().unwrap().inner_size(),
        [600, 500]
    );
}

#[test]
fn clearing_unregisters_pending_and_native_windows_even_when_ids_are_retained() {
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    update(&mut tree, &mut registry, &Window::new(probe()));
    let pending = registry.ids()[0];
    clear(&mut tree, &mut registry);
    let event_loop = HeadlessEventLoop::new();
    create(&mut registry, &event_loop);
    assert!(event_loop.created_window_ids().is_empty());
    assert!(registry.get_mut(pending).is_none());

    update(&mut tree, &mut registry, &Window::new(probe()));
    let live = registry.ids()[0];
    assert_ne!(pending, live);
    create(&mut registry, &event_loop);
    let native = registry.get_mut(live).unwrap().window_id().unwrap();
    clear(&mut tree, &mut registry);
    clear(&mut tree, &mut registry);
    assert!(registry.get_mut(live).is_none());
    assert!(registry.id_for_window(native).is_none());
    assert!(registry.location(native).is_none());
    assert!(registry.ids().is_empty());
}

#[test]
fn registry_owns_instances_independently_of_tree_and_id_lifetimes() {
    let mut registry = WindowRegistry::default();
    let id = {
        let mut tree = UiTree::new();
        update(&mut tree, &mut registry, &Window::new(probe()));
        create(&mut registry, &HeadlessEventLoop::new());
        registry.ids()[0]
    };
    // The framework must clear with Context while running; registry destruction
    // releases any remaining resources during application shutdown.
    assert!(registry.get_mut(id).unwrap().window().is_some());
    drop(registry);
    // Keeping this Copy ID alive does not retain a window or a registry reference.
    let _retained_id = id;
}

#[test]
fn ids_never_resolve_in_another_registry_or_after_replacement() {
    let mut a = WindowRegistry::default();
    let mut b = WindowRegistry::default();
    let mut first = UiTree::new();
    let mut second = UiTree::new();
    update(&mut first, &mut a, &Window::new(probe()));
    update(&mut second, &mut b, &Window::new(probe()));
    let old = a.ids()[0];
    let other = b.ids()[0];
    assert_ne!(old, other);
    assert!(a.get_mut(other).is_none());
    assert!(b.get_mut(old).is_none());
    clear(&mut first, &mut a);
    update(&mut first, &mut a, &Window::new(probe()));
    assert_ne!(a.ids()[0], old);
    assert!(a.get_mut(old).is_none());
}

#[test]
fn replacing_a_window_at_the_same_node_unregisters_only_the_old_id() {
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    update(&mut tree, &mut registry, &Window::new(probe()));
    let root = tree.root.unwrap();
    let event_loop = HeadlessEventLoop::new();
    create(&mut registry, &event_loop);
    let old = registry.ids()[0];
    let native = registry.get_mut(old).unwrap().window_id().unwrap();
    update(
        &mut tree,
        &mut registry,
        &Window::new(ContainerView {
            child: probe(),
            child_position: position(Matrix4::identity()),
        }),
    );
    assert!(registry.get_mut(old).is_none());
    assert!(registry.id_for_window(native).is_none());
    assert!(registry.location(native).is_none());
    assert_eq!(registry.ids().len(), 1);
    let new = registry.ids()[0];
    assert_ne!(old, new);
    assert_eq!(tree.root, Some(root));
    assert_eq!(
        tree.node_ref(root).unwrap().window.map(|w| w.id()),
        Some(new)
    );
    create(&mut registry, &event_loop);
    assert_eq!(event_loop.created_window_ids().len(), 2);
    let new_native = registry.get_mut(new).unwrap().window_id().unwrap();
    assert_eq!(registry.location(new_native).unwrap(), root);
    update(&mut tree, &mut registry, &probe());
    assert!(registry.ids().is_empty());
    assert!(tree.node_ref(root).unwrap().window.is_none());
}

struct WindowList(Vec<Window<ProbeView>>);
struct WindowListWidget;
impl View for WindowList {
    fn label(&self) -> Option<&str> {
        None
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        for window in &self.0 {
            ctx.child(window);
        }
        WindowListWidget
    }
}
impl Widget for WindowListWidget {
    type V = WindowList;
    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        for window in &view.0 {
            ctx.child(window);
        }
        UpdateResult::empty()
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }
    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds::default()
    }
    fn layout(&self, _: Bounds, _: Children<'_>, _: &mut Context) {}
    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        Draw::default()
    }
}

#[test]
fn keyed_windows_reorder_without_recreation_and_removal_invalidates_old_ids() {
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    let view = |keys: &[u64]| {
        WindowList(
            keys.iter()
                .map(|&key| {
                    Window::new(probe())
                        .with_key(key)
                        .with_config(config(&key.to_string(), None))
                })
                .collect(),
        )
    };
    update(&mut tree, &mut registry, &view(&[1, 2]));
    let event_loop = HeadlessEventLoop::new();
    create(&mut registry, &event_loop);
    let ids: HashMap<_, _> = registry
        .ids()
        .into_iter()
        .map(|id| {
            let instance = registry.get_mut(id).unwrap();
            (
                instance.window().unwrap().title(),
                (id, instance.window_id().unwrap()),
            )
        })
        .collect();
    let first_location = registry.location(ids["1"].1).unwrap();
    let second_location = registry.location(ids["2"].1).unwrap();
    update(&mut tree, &mut registry, &view(&[2, 1]));
    create(&mut registry, &event_loop);
    assert_eq!(event_loop.created_window_ids().len(), 2);
    assert_eq!(registry.location(ids["1"].1), Some(first_location));
    assert_eq!(registry.location(ids["2"].1), Some(second_location));
    assert_eq!(registry.id_for_window(ids["1"].1), Some(ids["1"].0));
    let root = tree.node_ref(tree.root.unwrap()).unwrap();
    assert_eq!(root.children[0].1, second_location);
    assert_eq!(root.children[1].1, first_location);
    update(&mut tree, &mut registry, &view(&[2, 3]));
    create(&mut registry, &event_loop);
    assert_eq!(event_loop.created_window_ids().len(), 3);
    assert!(registry.get_mut(ids["1"].0).is_none());
    assert!(registry.id_for_window(ids["1"].1).is_none());
    assert!(registry.location(ids["1"].1).is_none());
    assert_eq!(registry.location(ids["2"].1), Some(second_location));
    assert_eq!(registry.ids().len(), 2);
}

#[test]
fn nested_windows_share_resources_but_isolate_context_scopes() {
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    let mut context = Context::new(&mut registry);
    let shared = context.insert_resource(String::from("application resource"));
    tree.update(&Window::new(Window::new(probe())), &mut context);
    let outer = tree.root.unwrap();
    let inner = tree.node_ref(outer).unwrap().children[0].1;
    let parent_epoch = context.cache_epoch();
    let outer_epoch;
    {
        let mut a = tree.node_ref(outer).unwrap().scoped_context(&mut context);
        outer_epoch = a.cache_epoch();
        assert_ne!(outer_epoch, parent_epoch);
        assert!(Arc::ptr_eq(&a.resource::<String>().unwrap(), &shared));
        a.set_cursor_icon(CursorIcon::Text);
        a.set_ime_allowed(true);
        {
            let b = tree.node_ref(inner).unwrap().scoped_context(&mut a);
            assert_ne!(b.cache_epoch(), outer_epoch);
            assert!(Arc::ptr_eq(&b.resource::<String>().unwrap(), &shared));
            assert!(!b.is_ime_allowed());
        }
        assert_eq!(a.cache_epoch(), outer_epoch);
        assert!(a.is_ime_allowed());
    }
    assert_eq!(context.cache_epoch(), parent_epoch);
    assert!(!context.is_ime_allowed());
    assert!(
        tree.node_ref(outer)
            .unwrap()
            .scoped_context(&mut context)
            .is_ime_allowed()
    );
    tree.clear(&mut context);
    assert!(context.window_registry().ids().is_empty());
}
#[test]
fn parent_passes_do_not_visit_window_content_even_with_existing_placement() {
    let observed = Arc::new(Mutex::new(Observations::default()));
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    update(
        &mut tree,
        &mut registry,
        &ContainerView {
            child: Window::new(ProbeView(observed.clone())),
            child_position: position(Matrix4::identity()),
        },
    );
    let root = tree.root.unwrap();
    let window = tree.node_ref(root).unwrap().children[0];
    let child = tree.node_ref(window.1).unwrap().children[0];
    let measured = Children {
        tree: &mut tree,
        current: root,
    }
    .measure(
        window.0,
        Constraints::default(),
        &mut Context::new(&mut registry),
    );
    assert_eq!(measured, Some(Bounds::default()));
    tree.layout(bounds(), &mut Context::new(&mut registry));
    assert_eq!(observed.lock().unwrap().layout_calls, 0);
    tree.render(&mut Context::new(&mut registry)).unwrap();
    Children {
        tree: &mut tree,
        current: window.1,
    }
    .layout(
        child.0,
        position(Matrix4::identity()),
        &mut Context::new(&mut registry),
    );
    tree.render(&mut Context::new(&mut registry)).unwrap();
    let _ = tree.input(&mouse_event(5.0, 5.0), &mut Context::new(&mut registry));
    assert!(observed.lock().unwrap().events.is_empty());
    assert!(
        tree.node_ref(child.1)
            .unwrap()
            .cache
            .draw()
            .value()
            .is_none()
    );
}

fn update<V: View>(tree: &mut UiTree, registry: &mut WindowRegistry, view: &V) {
    tree.update(view, &mut Context::new(registry));
}

fn clear(tree: &mut UiTree, registry: &mut WindowRegistry) {
    tree.clear(&mut Context::new(registry));
}

#[test]
fn native_events_scope_epochs_and_refresh_only_the_target_window() {
    use matcha_window::event::window_event::WindowEvent;
    let a = Arc::new(Mutex::new(Observations::default()));
    let b = Arc::new(Mutex::new(Observations::default()));
    let view = WindowList(vec![
        Window::new(ProbeView(a.clone()))
            .with_key(1)
            .with_config(config("a", Some(Apply::Init([100, 80].into())))),
        Window::new(ProbeView(b.clone()))
            .with_key(2)
            .with_config(config("b", Some(Apply::Init([200, 160].into())))),
    ]);
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let parent_epoch = ctx.cache_epoch();
    let mut tree = UiTree::new();
    tree.update(&view, &mut ctx);
    create(ctx.window_registry(), &HeadlessEventLoop::new());
    let nodes = tree.node_ref(tree.root.unwrap()).unwrap().children.clone();
    let window_a = tree.node_ref(nodes[0].1).unwrap().window.unwrap();
    let window_b = tree.node_ref(nodes[1].1).unwrap().window.unwrap();
    let native_a = ctx
        .window_registry()
        .get_mut(window_a.id())
        .unwrap()
        .window_id()
        .unwrap();
    let native_b = ctx
        .window_registry()
        .get_mut(window_b.id())
        .unwrap()
        .window_id()
        .unwrap();
    let epoch_a = tree
        .node_ref(nodes[0].1)
        .unwrap()
        .scoped_context(&mut ctx)
        .cache_epoch();
    let epoch_b = tree
        .node_ref(nodes[1].1)
        .unwrap()
        .scoped_context(&mut ctx)
        .cache_epoch();
    assert_ne!(epoch_a, epoch_b);
    assert_eq!(a.lock().unwrap().scopes[0].1, epoch_a);
    assert_eq!(b.lock().unwrap().scopes[0].1, epoch_b);
    for native in [native_a, native_b] {
        assert!(tree.layout_window(native, &mut ctx));
        assert!(tree.render_window(native, &mut ctx).unwrap().is_some());
    }
    let child_a = tree.node_ref(nodes[0].1).unwrap().children[0].1;
    let child_b = tree.node_ref(nodes[1].1).unwrap().children[0].1;
    let old_a_layout = tree.node_ref(child_a).unwrap().cache.layout().stamp();
    let old_a_draw = tree.node_ref(child_a).unwrap().cache.draw().stamp();
    let old_b_layout = tree.node_ref(child_b).unwrap().cache.layout().stamp();
    // Delivery downcasts through the registry, but invalidation does not touch node caches.
    assert_eq!(
        tree.window_event(
            native_a,
            &WindowEvent::ScaleFactorChanged { scale_factor: 2.0 },
            &mut ctx
        ),
        Some(UpdateResult::LAYOUT | UpdateResult::RENDER)
    );
    assert_eq!(
        tree.node_ref(child_a).unwrap().cache.layout().stamp(),
        old_a_layout
    );
    assert_eq!(
        tree.node_ref(child_a).unwrap().cache.draw().stamp(),
        old_a_draw
    );
    tree.update(&view, &mut ctx);
    assert!(matches!(
        tree.render_window(native_a, &mut ctx),
        Err(UiError::LayoutRequired)
    ));
    tree.layout_window(native_a, &mut ctx);
    tree.render_window(native_a, &mut ctx).unwrap();
    tree.layout_window(native_b, &mut ctx);
    tree.render_window(native_b, &mut ctx).unwrap();
    assert_eq!(a.lock().unwrap().layout_calls, 2);
    assert_eq!(a.lock().unwrap().render_calls, 2);
    assert_eq!(b.lock().unwrap().layout_calls, 1);
    assert_eq!(b.lock().unwrap().render_calls, 1);
    assert_eq!(
        tree.node_ref(child_b).unwrap().cache.layout().stamp(),
        old_b_layout
    );
    let latest_a = tree
        .node_ref(nodes[0].1)
        .unwrap()
        .scoped_context(&mut ctx)
        .cache_epoch();
    assert_eq!(latest_a.revision(), epoch_a.revision() + 1);
    assert_eq!(
        tree.window_event(
            native_a,
            &WindowEvent::ScaleFactorChanged { scale_factor: 2.0 },
            &mut ctx
        ),
        Some(UpdateResult::empty())
    );
    let _ = tree.input_window(native_a, &mouse_event(5.0, 5.0), &mut ctx);
    let scopes = a.lock().unwrap().scopes.clone();
    for stage in ["update", "layout", "render", "input"] {
        let entry = scopes.iter().rev().find(|entry| entry.0 == stage).unwrap();
        assert_eq!((entry.1, entry.2), (latest_a, 2.0));
    }
    assert_eq!(ctx.cache_epoch(), parent_epoch);
    assert_eq!(ctx.dpi(), 1.0);
    assert!(b.lock().unwrap().events.is_empty());
    // Render-only changes accept old layout stamps without rewriting them.
    let layout_stamp = tree.node_ref(child_a).unwrap().cache.layout().stamp();
    tree.window_event(
        native_a,
        &WindowEvent::Theme(matcha_window::window::Theme::Dark),
        &mut ctx,
    );
    tree.render_window(native_a, &mut ctx).unwrap();
    assert_eq!(
        tree.node_ref(child_a).unwrap().cache.layout().stamp(),
        layout_stamp
    );
    assert_eq!(a.lock().unwrap().render_calls, 3);
    // A resize propagates new metrics even if the synthetic backend has not been resized.
    tree.window_event(
        native_a,
        &WindowEvent::Resized {
            inner_size: [320.0, 240.0],
            outer_size: [320.0, 240.0],
        },
        &mut ctx,
    );
    tree.layout_window(native_a, &mut ctx);
    assert_eq!(a.lock().unwrap().layout_bounds.last().unwrap().x, 320.0);
    assert_eq!(a.lock().unwrap().scopes.last().unwrap().3, [320.0, 240.0]);
    tree.render_window(native_a, &mut ctx).unwrap();
    let before = tree.node_ref(child_a).unwrap().cache.draw().stamp();
    let renders = a.lock().unwrap().render_calls;
    assert!(tree.invalidate_window_caches(native_a, UpdateResult::empty(), &mut ctx));
    tree.render_window(native_a, &mut ctx).unwrap();
    assert_eq!(tree.node_ref(child_a).unwrap().cache.draw().stamp(), before);
    for _ in 0..2 {
        assert!(tree.invalidate_window_caches(native_a, UpdateResult::RENDER, &mut ctx));
    }
    tree.render_window(native_a, &mut ctx).unwrap();
    tree.render_window(native_a, &mut ctx).unwrap();
    assert_eq!(a.lock().unwrap().render_calls, renders + 1);
    tree.clear(&mut ctx);
    assert_eq!(
        tree.window_event(native_a, &WindowEvent::CloseRequested, &mut ctx),
        None
    );
    assert!(!tree.layout_window(native_a, &mut ctx));
    assert!(tree.render_window(native_a, &mut ctx).unwrap().is_none());
}

#[test]
fn window_replacement_changes_domain_and_dispatch_type_at_the_same_ui_id() {
    use matcha_window::event::window_event::WindowEvent;
    let observed = Arc::new(Mutex::new(Observations::default()));
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    tree.update(&Window::new(ProbeView(observed.clone())), &mut ctx);
    create(ctx.window_registry(), &HeadlessEventLoop::new());
    let root = tree.root.unwrap();
    let old_binding = tree.node_ref(root).unwrap().window.unwrap();
    let old_native = ctx
        .window_registry()
        .get_mut(old_binding.id())
        .unwrap()
        .window_id()
        .unwrap();
    tree.layout_window(old_native, &mut ctx);
    let old_epoch = tree
        .node_ref(root)
        .unwrap()
        .scoped_context(&mut ctx)
        .cache_epoch();
    tree.update(
        &Window::new(ContainerView {
            child: ProbeView(observed.clone()),
            child_position: position(Matrix4::identity()),
        }),
        &mut ctx,
    );
    create(ctx.window_registry(), &HeadlessEventLoop::new());
    assert_eq!(tree.root.unwrap(), root);
    let new_binding = tree.node_ref(root).unwrap().window.unwrap();
    let new_native = ctx
        .window_registry()
        .get_mut(new_binding.id())
        .unwrap()
        .window_id()
        .unwrap();
    let new_epoch = tree
        .node_ref(root)
        .unwrap()
        .scoped_context(&mut ctx)
        .cache_epoch();
    assert_ne!(old_epoch, new_epoch);
    assert_eq!(
        tree.window_event(
            old_native,
            &WindowEvent::ScaleFactorChanged { scale_factor: 3.0 },
            &mut ctx
        ),
        None
    );
    assert!(
        tree.window_event(
            new_native,
            &WindowEvent::ScaleFactorChanged { scale_factor: 2.0 },
            &mut ctx
        )
        .is_some()
    );
    tree.layout_window(new_native, &mut ctx);
    tree.render_window(new_native, &mut ctx).unwrap();
    assert_eq!(observed.lock().unwrap().scopes.last().unwrap().2, 2.0);
}
