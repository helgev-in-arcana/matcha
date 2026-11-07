use std::collections::HashMap;

use matcha_window::headless_interface::HeadlessEventLoop;
use matcha_window::window::{CursorIcon, Size, Theme, WindowConfig as NativeConfig};

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
    let child = child_at(&tree, root, 0);
    assert_eq!(tree.nodes.iter().count(), 2);
    assert_eq!(tree.node_ref(root).unwrap().label.as_deref(), Some("main"));
    assert_eq!(tree.node_ref(child).unwrap().parent, Some(root));

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
    assert_eq!(child_at(&tree, root, 0), child);
    assert_eq!(registry.id_for_window(native), Some(id));
    let _ = tree
        .node_ref_mut(child)
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
    type ChildData = ();
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
    assert_eq!(root.child_ids().next().unwrap(), second_location);
    assert_eq!(root.child_ids().nth(1).unwrap(), first_location);
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
    let inner = child_at(&tree, outer, 0);
    {
        let mut a = tree.node_ref(outer).unwrap().scoped_context(&mut context);
        assert!(Arc::ptr_eq(&a.resource::<String>().unwrap(), &shared));
        a.set_cursor_icon(CursorIcon::Text);
        a.set_ime_allowed(true);
        {
            let b = tree.node_ref(inner).unwrap().scoped_context(&mut a);
            assert!(Arc::ptr_eq(&b.resource::<String>().unwrap(), &shared));
            assert!(!b.is_ime_allowed());
        }
        assert!(a.is_ime_allowed());
    }
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
    let window = child_at(&tree, root, 0);
    let child = child_at(&tree, window, 0);
    let measured = tree.measure_node(
        window,
        Constraints::default(),
        &mut Context::new(&mut registry),
    );
    assert_eq!(measured, Bounds::default());
    tree.layout(bounds(), &mut Context::new(&mut registry));
    assert_eq!(observed.lock().unwrap().layout_calls, 0);
    tree.render(&mut Context::new(&mut registry)).unwrap();
    tree.layout_node(
        child,
        position(Matrix4::identity()),
        &mut Context::new(&mut registry),
    );
    tree.render(&mut Context::new(&mut registry)).unwrap();
    let _ = tree.input(&mouse_event(5.0, 5.0), &mut Context::new(&mut registry));
    assert!(observed.lock().unwrap().events.is_empty());
    assert!(
        tree.node_ref(child)
            .unwrap()
            .render_cache
            .get(&())
            .is_none()
    );
}

fn update<V: View>(tree: &mut UiTree, registry: &mut WindowRegistry, view: &V) {
    tree.update(view, &mut Context::new(registry));
}

fn clear(tree: &mut UiTree, registry: &mut WindowRegistry) {
    tree.clear(&mut Context::new(registry));
}

fn native_id(tree: &UiTree, node: UiId, ctx: &mut Context) -> matcha_window::window::WindowId {
    let binding = tree.node_ref(node).unwrap().window.unwrap();
    ctx.window_registry()
        .get_mut(binding.id())
        .unwrap()
        .window_id()
        .unwrap()
}

fn measure_content(tree: &mut UiTree, window: UiId, ctx: &mut Context) -> Bounds {
    let node = tree.node_ref(window).unwrap();
    let child = node.child_ids().next().unwrap();
    let mut scoped = node.scoped_context(ctx);
    tree.measure_node(child, Constraints::default(), &mut scoped)
}

#[test]
fn native_events_refresh_only_the_target_window_and_selected_caches() {
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
    let mut tree = UiTree::new();
    tree.update(&view, &mut ctx);
    create(ctx.window_registry(), &HeadlessEventLoop::new());
    let nodes = tree
        .node_ref(tree.root.unwrap())
        .unwrap()
        .child_ids()
        .collect::<Vec<_>>();
    let native_a = native_id(&tree, nodes[0], &mut ctx);
    let native_b = native_id(&tree, nodes[1], &mut ctx);
    for native in [native_a, native_b] {
        assert!(tree.layout_window(native, &mut ctx));
        assert!(tree.render_window(native, &mut ctx).unwrap().is_some());
    }
    let child_a = child_at(&tree, nodes[0], 0);
    let child_b = child_at(&tree, nodes[1], 0);
    assert_eq!(
        tree.window_event(
            native_a,
            &WindowEvent::ScaleFactorChanged { scale_factor: 2.0 },
            &mut ctx
        ),
        Some(UpdateResult::LAYOUT | UpdateResult::RENDER)
    );
    assert!(
        tree.node_ref(child_a)
            .unwrap()
            .layout_cache
            .value()
            .is_none()
    );
    assert!(
        !tree
            .node_ref(child_a)
            .unwrap()
            .render_cache
            .get(&())
            .is_some()
    );
    assert!(
        tree.node_ref(child_b)
            .unwrap()
            .layout_cache
            .value()
            .is_some()
    );
    assert!(
        tree.node_ref(child_b)
            .unwrap()
            .render_cache
            .get(&())
            .is_some()
    );
    tree.update(&view, &mut ctx);
    assert!(matches!(
        tree.render_window(native_a, &mut ctx),
        Err(UiError::LayoutRequired)
    ));
    for native in [native_a, native_b] {
        tree.layout_window(native, &mut ctx);
        tree.render_window(native, &mut ctx).unwrap();
    }
    assert_eq!(a.lock().unwrap().layout_calls, 2);
    assert_eq!(a.lock().unwrap().render_calls, 2);
    assert_eq!(b.lock().unwrap().layout_calls, 1);
    assert_eq!(b.lock().unwrap().render_calls, 1);
    assert_eq!(
        tree.window_event(
            native_a,
            &WindowEvent::ScaleFactorChanged { scale_factor: 2.0 },
            &mut ctx
        ),
        Some(UpdateResult::empty())
    );
    tree.layout_window(native_a, &mut ctx);
    tree.render_window(native_a, &mut ctx).unwrap();
    assert_eq!(a.lock().unwrap().layout_calls, 2);
    assert_eq!(a.lock().unwrap().render_calls, 2);
    let _ = tree.input_window(native_a, &mouse_event(5.0, 5.0), &mut ctx);
    let scopes = a.lock().unwrap().scopes.clone();
    for stage in ["update", "layout", "render", "input"] {
        assert_eq!(
            scopes
                .iter()
                .rev()
                .find(|entry| entry.0 == stage)
                .unwrap()
                .1,
            2.0
        );
    }
    assert_eq!(ctx.dpi(), 1.0);
    assert!(b.lock().unwrap().events.is_empty());

    tree.window_event(native_a, &WindowEvent::Theme(Theme::Dark), &mut ctx);
    assert!(
        tree.node_ref(child_a)
            .unwrap()
            .layout_cache
            .value()
            .is_some()
    );
    assert!(
        !tree
            .node_ref(child_a)
            .unwrap()
            .render_cache
            .get(&())
            .is_some()
    );
    tree.render_window(native_a, &mut ctx).unwrap();
    assert_eq!(a.lock().unwrap().layout_calls, 2);
    assert_eq!(a.lock().unwrap().render_calls, 3);
    assert!(tree.invalidate_window_caches(native_a, UpdateResult::empty(), &mut ctx));
    tree.render_window(native_a, &mut ctx).unwrap();
    assert_eq!(a.lock().unwrap().render_calls, 3);
    for _ in 0..2 {
        assert!(tree.invalidate_window_caches(native_a, UpdateResult::RENDER, &mut ctx));
    }
    tree.render_window(native_a, &mut ctx).unwrap();
    tree.render_window(native_a, &mut ctx).unwrap();
    assert_eq!(a.lock().unwrap().render_calls, 4);
    tree.clear(&mut ctx);
    assert_eq!(
        tree.window_event(native_a, &WindowEvent::CloseRequested, &mut ctx),
        None
    );
    assert!(!tree.invalidate_window_caches(native_a, UpdateResult::LAYOUT, &mut ctx));
    assert!(!tree.layout_window(native_a, &mut ctx));
    assert!(tree.render_window(native_a, &mut ctx).unwrap().is_none());
}

#[test]
fn environment_changes_reach_descendants_behind_unchanged_bounds() {
    use matcha_window::event::window_event::WindowEvent;
    let observed = Arc::new(Mutex::new(Observations::default()));
    let view = Window::new(ContainerView {
        child: ContainerView {
            child: ProbeView(observed.clone()),
            child_position: position(Matrix4::identity()),
        },
        child_position: position(Matrix4::identity()),
    })
    .with_config(config("outer", Some(Apply::Init([100, 80].into()))));
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let mut tree = UiTree::new();
    tree.update(&view, &mut ctx);
    create(ctx.window_registry(), &HeadlessEventLoop::new());
    let root = tree.root.unwrap();
    let native = native_id(&tree, root, &mut ctx);
    assert_eq!(measure_content(&mut tree, root, &mut ctx).x, 10.0);
    tree.layout_window(native, &mut ctx);
    tree.render_window(native, &mut ctx).unwrap();

    tree.window_event(
        native,
        &WindowEvent::ScaleFactorChanged { scale_factor: 2.0 },
        &mut ctx,
    );
    assert_eq!(measure_content(&mut tree, root, &mut ctx).x, 20.0);
    assert!(matches!(
        tree.render_window(native, &mut ctx),
        Err(UiError::LayoutRequired)
    ));
    tree.layout_window(native, &mut ctx);
    tree.render_window(native, &mut ctx).unwrap();
    tree.window_event(
        native,
        &WindowEvent::Resized {
            inner_size: [320.0, 240.0],
            outer_size: [320.0, 240.0],
        },
        &mut ctx,
    );
    assert_eq!(measure_content(&mut tree, root, &mut ctx).x, 20.0);
    tree.layout_window(native, &mut ctx);
    tree.render_window(native, &mut ctx).unwrap();
    let observations = observed.lock().unwrap();
    assert_eq!(observations.measure_calls, 3);
    assert_eq!(observations.layout_calls, 3);
    assert_eq!(observations.render_calls, 3);
    assert!(observations.layout_bounds.iter().all(|b| *b == bounds()));
    for stage in ["measure", "layout", "render"] {
        let scope = observations
            .scopes
            .iter()
            .rev()
            .find(|scope| scope.0 == stage)
            .unwrap();
        assert_eq!((scope.1, scope.2), (2.0, [320.0, 240.0]));
    }
}

#[test]
fn outer_window_invalidation_preserves_nested_window_caches() {
    use matcha_window::event::window_event::WindowEvent;
    let observed = Arc::new(Mutex::new(Observations::default()));
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let mut tree = UiTree::new();
    tree.update(
        &Window::new(ContainerView {
            child: Window::new(ProbeView(observed.clone())),
            child_position: position(Matrix4::identity()),
        }),
        &mut ctx,
    );
    create(ctx.window_registry(), &HeadlessEventLoop::new());
    let outer = tree.root.unwrap();
    let content = child_at(&tree, outer, 0);
    let inner = child_at(&tree, content, 0);
    let leaf = child_at(&tree, inner, 0);
    let native_outer = native_id(&tree, outer, &mut ctx);
    let native_inner = native_id(&tree, inner, &mut ctx);
    for native in [native_outer, native_inner] {
        tree.layout_window(native, &mut ctx);
        tree.render_window(native, &mut ctx).unwrap();
    }
    tree.window_event(
        native_outer,
        &WindowEvent::ScaleFactorChanged { scale_factor: 2.0 },
        &mut ctx,
    );
    tree.invalidate_window_caches(
        native_outer,
        UpdateResult::LAYOUT | UpdateResult::RENDER,
        &mut ctx,
    );
    assert!(
        tree.node_ref(content)
            .unwrap()
            .layout_cache
            .value()
            .is_none()
    );
    assert!(tree.node_ref(inner).unwrap().layout_cache.value().is_some());
    assert!(tree.node_ref(leaf).unwrap().layout_cache.value().is_some());
    assert!(tree.node_ref(leaf).unwrap().render_cache.get(&()).is_some());
    tree.layout_window(native_inner, &mut ctx);
    tree.render_window(native_inner, &mut ctx).unwrap();
    assert_eq!(observed.lock().unwrap().layout_calls, 1);
    assert_eq!(observed.lock().unwrap().render_calls, 1);

    // Removing the outer window retains the inner native window and its environment.
    tree.update(
        &ContainerView {
            child: ContainerView {
                child: Window::new(ProbeView(observed.clone())),
                child_position: position(Matrix4::identity()),
            },
            child_position: position(Matrix4::identity()),
        },
        &mut ctx,
    );
    assert_eq!(native_id(&tree, inner, &mut ctx), native_inner);
    tree.layout_window(native_inner, &mut ctx);
    tree.render_window(native_inner, &mut ctx).unwrap();
    assert_eq!(observed.lock().unwrap().layout_calls, 1);
    assert_eq!(observed.lock().unwrap().render_calls, 1);
}

#[test]
fn entering_and_leaving_a_window_invalidates_reused_descendants() {
    use matcha_window::event::window_event::WindowEvent;
    let observed = Arc::new(Mutex::new(Observations::default()));
    let content = || ContainerView {
        child: ProbeView(observed.clone()),
        child_position: position(Matrix4::identity()),
    };
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let mut tree = UiTree::new();
    tree.update(&Window::new(content()), &mut ctx);
    create(ctx.window_registry(), &HeadlessEventLoop::new());
    let root = tree.root.unwrap();
    let child = child_at(&tree, root, 0);
    let leaf = child_at(&tree, child, 0);
    let old_native = native_id(&tree, root, &mut ctx);
    tree.window_event(
        old_native,
        &WindowEvent::ScaleFactorChanged { scale_factor: 2.0 },
        &mut ctx,
    );
    assert_eq!(measure_content(&mut tree, root, &mut ctx).x, 20.0);
    tree.layout_window(old_native, &mut ctx);
    tree.render_window(old_native, &mut ctx).unwrap();

    tree.update(
        &ContainerView {
            child: content(),
            child_position: position(Matrix4::identity()),
        },
        &mut ctx,
    );
    assert_eq!(tree.root.unwrap(), root);
    assert_eq!(child_at(&tree, root, 0), child);
    for id in [child, leaf] {
        let node = tree.node_ref(id).unwrap();
        assert!(node.measure_cache.value().is_none());
        assert!(node.layout_cache.value().is_none());
        assert!(node.render_cache.get(&()).is_none());
    }
    assert_eq!(
        tree.window_event(old_native, &WindowEvent::CloseRequested, &mut ctx),
        None
    );
    assert_eq!(
        tree.measure_node(child, Constraints::default(), &mut ctx).x,
        10.0
    );
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    assert_eq!(observed.lock().unwrap().layout_calls, 2);
    assert_eq!(observed.lock().unwrap().render_calls, 2);

    tree.update(&Window::new(content()), &mut ctx);
    assert_eq!(child_at(&tree, root, 0), child);
    assert!(tree.node_ref(leaf).unwrap().layout_cache.value().is_none());
    assert!(tree.node_ref(leaf).unwrap().measure_cache.value().is_none());
    assert!(!tree.node_ref(leaf).unwrap().render_cache.get(&()).is_some());
    create(ctx.window_registry(), &HeadlessEventLoop::new());
    let new_native = native_id(&tree, root, &mut ctx);
    assert_ne!(old_native, new_native);
    tree.layout_window(new_native, &mut ctx);
    tree.render_window(new_native, &mut ctx).unwrap();
    assert_eq!(observed.lock().unwrap().layout_calls, 3);
    assert_eq!(observed.lock().unwrap().render_calls, 3);
}
