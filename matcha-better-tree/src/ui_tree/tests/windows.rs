use std::collections::HashMap;

use matcha_window::headless_interface::HeadlessEventLoop;
use matcha_window::window::{CursorIcon, Size, WindowConfig as NativeConfig};

use super::*;
use crate::window::{Apply, Window, WindowConfig, WindowRegistry};

fn context<'a>(tree: &UiTree, registry: &'a WindowRegistry) -> Context<'a> {
    tree.context().clone().with_window_registry(registry)
}

fn update<V: View>(tree: &mut UiTree, registry: &WindowRegistry, view: &V) {
    tree.update_with_context(view, context(tree, registry));
}

fn clear(tree: &mut UiTree, registry: &WindowRegistry) {
    tree.clear_with_context(context(tree, registry));
}

fn create(registry: &WindowRegistry, event_loop: &HeadlessEventLoop) {
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
    let registry = WindowRegistry::default();
    update(
        &mut tree,
        &registry,
        &Window::new(probe()).with_label("main"),
    );
    let root = tree.root.unwrap();
    let child = tree.node_ref(root).unwrap().children[0];
    assert_eq!(tree.nodes.iter().count(), 2);
    assert_eq!(tree.node_ref(root).unwrap().label.as_deref(), Some("main"));
    assert_eq!(tree.node_ref(child.1).unwrap().parent, Some(root));

    let id = registry.ids()[0];
    assert_eq!(tree.node_ref(root).unwrap().window, Some(id));
    assert!(registry.get(id).unwrap().window_id().is_none());
    create(&registry, &HeadlessEventLoop::new());
    let native = registry.get(id).unwrap().window_id().unwrap();
    assert_eq!(registry.id_for_window(native), Some(id));
    assert_eq!(registry.location(native), Some((root, child.0)));

    let updated = Arc::new(Mutex::new(Observations::default()));
    update(
        &mut tree,
        &registry,
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
        .input(bounds(), &mouse_event(5.0, 5.0), Context::new());
    assert_eq!(updated.lock().unwrap().events.len(), 1);
}

#[test]
fn pending_windows_use_latest_init_values_and_are_created_once() {
    let mut tree = UiTree::new();
    let registry = WindowRegistry::default();
    let view =
        |title, size| Window::new(probe()).with_config(config(title, Some(Apply::Init(size))));
    update(&mut tree, &registry, &view("first", [200, 100].into()));
    let id = registry.ids()[0];
    update(&mut tree, &registry, &view("latest", [400, 300].into()));
    let event_loop = HeadlessEventLoop::new();
    create(&registry, &event_loop);
    create(&registry, &event_loop);
    assert_eq!(event_loop.created_window_ids().len(), 1);
    let instance = registry.get(id).unwrap();
    assert_eq!(instance.window().unwrap().title(), "latest");
    assert_eq!(instance.window().unwrap().inner_size(), [400, 300]);
    assert_eq!(
        instance.context(context(&tree, &registry)).viewport_size(),
        [400.0, 300.0]
    );
}

#[test]
fn init_preserves_native_changes_and_always_reapplies_unchanged_values() {
    let mut tree = UiTree::new();
    let registry = WindowRegistry::default();
    let view = |title, size| Window::new(probe()).with_config(config(title, size));
    update(
        &mut tree,
        &registry,
        &view("initial", Some(Apply::Init([200, 100].into()))),
    );
    create(&registry, &HeadlessEventLoop::new());
    let id = registry.ids()[0];
    registry
        .get(id)
        .unwrap()
        .window()
        .unwrap()
        .request_inner_size(900, 800);
    update(
        &mut tree,
        &registry,
        &view("updated", Some(Apply::Init([400, 300].into()))),
    );
    assert_eq!(
        registry.get(id).unwrap().window().unwrap().inner_size(),
        [900, 800]
    );
    assert_eq!(
        registry.get(id).unwrap().window().unwrap().title(),
        "updated"
    );
    let controlled = Some(Apply::Always(Size::Logical {
        width: 320.0,
        height: 240.0,
    }));
    update(&mut tree, &registry, &view("updated", controlled));
    assert_eq!(
        registry.get(id).unwrap().window().unwrap().inner_size(),
        [320, 240]
    );
    registry
        .get(id)
        .unwrap()
        .window()
        .unwrap()
        .request_inner_size(900, 800);
    update(&mut tree, &registry, &view("updated", controlled));
    assert_eq!(
        registry.get(id).unwrap().window().unwrap().inner_size(),
        [320, 240]
    );
    registry
        .get(id)
        .unwrap()
        .window()
        .unwrap()
        .request_inner_size(600, 500);
    update(&mut tree, &registry, &view("unspecified", None));
    assert_eq!(
        registry.get(id).unwrap().window().unwrap().inner_size(),
        [600, 500]
    );
}

#[test]
fn clearing_unregisters_pending_and_native_windows_even_when_ids_are_retained() {
    let mut tree = UiTree::new();
    let registry = WindowRegistry::default();
    update(&mut tree, &registry, &Window::new(probe()));
    let pending = registry.ids()[0];
    clear(&mut tree, &registry);
    let event_loop = HeadlessEventLoop::new();
    create(&registry, &event_loop);
    assert!(event_loop.created_window_ids().is_empty());
    assert!(registry.get(pending).is_none());

    update(&mut tree, &registry, &Window::new(probe()));
    let live = registry.ids()[0];
    assert_ne!(pending, live);
    create(&registry, &event_loop);
    let native = registry.get(live).unwrap().window_id().unwrap();
    clear(&mut tree, &registry);
    clear(&mut tree, &registry);
    assert!(registry.get(live).is_none());
    assert!(registry.id_for_window(native).is_none());
    assert!(registry.location(native).is_none());
    assert!(registry.ids().is_empty());
}

#[test]
fn registry_owns_instances_independently_of_tree_and_id_lifetimes() {
    let registry = WindowRegistry::default();
    let id = {
        let mut tree = UiTree::new();
        update(&mut tree, &registry, &Window::new(probe()));
        create(&registry, &HeadlessEventLoop::new());
        registry.ids()[0]
    };
    // The framework must clear with Context while running; registry destruction
    // releases any remaining resources during application shutdown.
    assert!(registry.get(id).unwrap().window().is_some());
    drop(registry);
    // Keeping this Copy ID alive does not retain a window or a registry reference.
    let _retained_id = id;
}

#[test]
fn ids_never_resolve_in_another_registry_or_after_replacement() {
    let a = WindowRegistry::default();
    let b = WindowRegistry::default();
    let mut first = UiTree::new();
    let mut second = UiTree::new();
    update(&mut first, &a, &Window::new(probe()));
    update(&mut second, &b, &Window::new(probe()));
    let old = a.ids()[0];
    let other = b.ids()[0];
    assert_ne!(old, other);
    assert!(a.get(other).is_none());
    assert!(b.get(old).is_none());
    clear(&mut first, &a);
    update(&mut first, &a, &Window::new(probe()));
    assert_ne!(a.ids()[0], old);
    assert!(a.get(old).is_none());
}

#[test]
fn replacing_a_window_at_the_same_node_unregisters_only_the_old_id() {
    let mut tree = UiTree::new();
    let registry = WindowRegistry::default();
    update(&mut tree, &registry, &Window::new(probe()));
    let root = tree.root.unwrap();
    let event_loop = HeadlessEventLoop::new();
    create(&registry, &event_loop);
    let old = registry.ids()[0];
    let native = registry.get(old).unwrap().window_id().unwrap();
    update(
        &mut tree,
        &registry,
        &Window::new(ContainerView {
            child: probe(),
            child_position: position(Matrix4::identity()),
        }),
    );
    assert!(registry.get(old).is_none());
    assert!(registry.id_for_window(native).is_none());
    assert!(registry.location(native).is_none());
    assert_eq!(registry.ids().len(), 1);
    let new = registry.ids()[0];
    assert_ne!(old, new);
    assert_eq!(tree.root, Some(root));
    assert_eq!(tree.node_ref(root).unwrap().window, Some(new));
    create(&registry, &event_loop);
    assert_eq!(event_loop.created_window_ids().len(), 2);
    let new_native = registry.get(new).unwrap().window_id().unwrap();
    assert_eq!(registry.location(new_native).unwrap().0, root);
    update(&mut tree, &registry, &probe());
    assert!(registry.ids().is_empty());
    assert!(tree.node_ref(root).unwrap().window.is_none());
}

struct WindowList(Vec<Window<ProbeView>>);
struct WindowListWidget;
impl View for WindowList {
    fn label(&self) -> Option<&str> {
        None
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a>) -> impl Widget<V = Self> {
        for window in &self.0 {
            ctx.child(window);
        }
        WindowListWidget
    }
}
impl Widget for WindowListWidget {
    type V = WindowList;
    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a>) -> UpdateResult {
        for window in &view.0 {
            ctx.child(window);
        }
        UpdateResult::empty()
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: Context) -> InputResult {
        InputResult::default()
    }
    fn measure(&self, _: Constraints, _: Children<'_>, _: Context) -> Bounds {
        Bounds::default()
    }
    fn layout(&self, _: Bounds, _: Children<'_>, _: Context) {}
    fn render(&self, _: Bounds, _: Context) -> Draw {
        Draw::default()
    }
}

#[test]
fn keyed_windows_reorder_without_recreation_and_removal_invalidates_old_ids() {
    let mut tree = UiTree::new();
    let registry = WindowRegistry::default();
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
    update(&mut tree, &registry, &view(&[1, 2]));
    let event_loop = HeadlessEventLoop::new();
    create(&registry, &event_loop);
    let ids: HashMap<_, _> = registry
        .ids()
        .into_iter()
        .map(|id| {
            let instance = registry.get(id).unwrap();
            (
                instance.window().unwrap().title(),
                (id, instance.window_id().unwrap()),
            )
        })
        .collect();
    let first_location = registry.location(ids["1"].1).unwrap();
    let second_location = registry.location(ids["2"].1).unwrap();
    update(&mut tree, &registry, &view(&[2, 1]));
    create(&registry, &event_loop);
    assert_eq!(event_loop.created_window_ids().len(), 2);
    assert_eq!(registry.location(ids["1"].1), Some(first_location));
    assert_eq!(registry.location(ids["2"].1), Some(second_location));
    assert_eq!(registry.id_for_window(ids["1"].1), Some(ids["1"].0));
    let root = tree.node_ref(tree.root.unwrap()).unwrap();
    assert_eq!(root.children[0].1, second_location.0);
    assert_eq!(root.children[1].1, first_location.0);
    update(&mut tree, &registry, &view(&[2, 3]));
    create(&registry, &event_loop);
    assert_eq!(event_loop.created_window_ids().len(), 3);
    assert!(registry.get(ids["1"].0).is_none());
    assert!(registry.id_for_window(ids["1"].1).is_none());
    assert!(registry.location(ids["1"].1).is_none());
    assert_eq!(registry.location(ids["2"].1), Some(second_location));
    assert_eq!(registry.ids().len(), 2);
}

#[test]
fn nested_windows_share_resources_but_isolate_platform_state() {
    let mut tree = UiTree::new();
    let registry = WindowRegistry::default();
    let shared = tree
        .context()
        .insert_resource(String::from("application resource"));
    update(&mut tree, &registry, &Window::new(Window::new(probe())));
    assert_eq!(registry.ids().len(), 2);
    assert_eq!(tree.nodes.iter().count(), 3);
    create(&registry, &HeadlessEventLoop::new());
    let ids = registry.ids();
    let a = registry
        .get(ids[0])
        .unwrap()
        .context(context(&tree, &registry));
    let b = registry
        .get(ids[1])
        .unwrap()
        .context(context(&tree, &registry));
    assert!(std::ptr::eq(
        a.window_registry().unwrap(),
        b.window_registry().unwrap()
    ));
    assert!(Arc::ptr_eq(&a.resource::<String>().unwrap(), &shared));
    assert!(Arc::ptr_eq(&b.resource::<String>().unwrap(), &shared));
    a.set_cursor_icon(CursorIcon::Text);
    a.set_ime_allowed(true);
    a.set_ime_cursor_area([10.0, 20.0], [1.0, 12.0]);
    assert_eq!(
        registry
            .get(ids[0])
            .unwrap()
            .context(context(&tree, &registry))
            .cursor_icon(),
        CursorIcon::Text
    );
    assert!(!b.is_ime_allowed());
    assert_eq!(b.ime_cursor_area(), None);
    assert!(!tree.context().is_ime_allowed());
    for id in ids {
        let native = registry.get(id).unwrap().window_id().unwrap();
        let (node, child) = registry.location(native).unwrap();
        assert!(tree.node_ref(node).unwrap().child(child).is_some());
    }
    clear(&mut tree, &registry);
    assert!(registry.ids().is_empty());
}

#[test]
fn clear_without_window_context_fails_before_mutating_the_tree() {
    let mut tree = UiTree::new();
    let registry = WindowRegistry::default();
    update(&mut tree, &registry, &Window::new(probe()));
    let root = tree.root;
    let id = registry.ids()[0];
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tree.clear()));
    assert!(result.is_err());
    assert_eq!(tree.root, root);
    assert!(registry.get(id).is_some());
    clear(&mut tree, &registry);
    assert!(registry.get(id).is_none());
}

#[test]
fn parent_passes_do_not_visit_window_content_even_with_existing_placement() {
    let observed = Arc::new(Mutex::new(Observations::default()));
    let mut tree = UiTree::new();
    let registry = WindowRegistry::default();
    update(
        &mut tree,
        &registry,
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
    .measure(window.0, Constraints::default(), Context::new());
    assert_eq!(measured, Some(Bounds::default()));
    tree.layout(bounds(), Context::new());
    assert_eq!(observed.lock().unwrap().layout_calls, 0);
    tree.render(Context::new()).unwrap();
    Children {
        tree: &mut tree,
        current: window.1,
    }
    .layout(child.0, position(Matrix4::identity()), Context::new());
    tree.render(Context::new()).unwrap();
    let _ = tree.input(&mouse_event(5.0, 5.0), Context::new());
    assert!(observed.lock().unwrap().events.is_empty());
    assert!(tree.node_ref(child.1).unwrap().draw_cache.is_none());
}
