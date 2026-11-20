use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use web_time::Instant;

use matcha_window::clipboard::Clipboard;
use matcha_window::window::{CursorIcon, Window, WindowError, WindowId};

use super::env_reads::EnvReads;
use crate::ui_tree::window::{WindowConfig, WindowRegistry};

#[derive(Default)]
pub(crate) struct PlatformState {
    cursor_icon: CursorIcon,
    pushed_cursor_icon: CursorIcon,

    ime_allowed: bool,
    pushed_ime_allowed: bool,

    ime_cursor_area: Option<([f32; 2], [f32; 2])>,
    pushed_ime_cursor_area: Option<([f32; 2], [f32; 2])>,
}
impl PlatformState {
    /// Apply pending platform changes and remember which values were issued.
    pub(crate) fn sync_to_window(&mut self, window: &Window) {
        if self.cursor_icon != self.pushed_cursor_icon {
            window.set_cursor_icon(self.cursor_icon);
            self.pushed_cursor_icon = self.cursor_icon;
        }

        let allowed_changed = self.ime_allowed != self.pushed_ime_allowed;
        let area_changed = self.ime_cursor_area != self.pushed_ime_cursor_area;

        if allowed_changed {
            window.set_ime_allowed(self.ime_allowed);
            self.pushed_ime_allowed = self.ime_allowed;
        }

        if self.ime_allowed
            && (allowed_changed || area_changed)
            && let Some((pos, size)) = self.ime_cursor_area
        {
            window.set_ime_cursor_area(pos, size);
            self.pushed_ime_cursor_area = self.ime_cursor_area;
        } else if !self.ime_allowed && area_changed {
            self.pushed_ime_cursor_area = self.ime_cursor_area;
        }
    }
}

#[derive(Default)]
pub(crate) struct Services {
    platform: PlatformState,
    clipboard: OnceLock<Clipboard>,
    resources: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

/// Standalone contexts own services. App callbacks and nested scopes borrow persistent storage.
enum ServicesStorage<'a> {
    Owned(Services),
    Borrowed(&'a mut Services),
}

impl ServicesStorage<'_> {
    fn get(&self) -> &Services {
        match self {
            Self::Owned(services) => services,
            Self::Borrowed(services) => services,
        }
    }

    fn get_mut(&mut self) -> &mut Services {
        match self {
            Self::Owned(services) => services,
            Self::Borrowed(services) => services,
        }
    }
}

type WindowFactory<'a> = dyn FnMut(&WindowConfig) -> Result<Window, WindowError> + 'a;

#[derive(Clone, Copy)]
enum TreeScope {
    Standalone,
    /// App drives polling from each native window, never from this declaration root.
    WindowDeclarations,
    Window(WindowId),
}

/// Extra geometry cache inputs supplied by a widget when it calls into a child.
/// Ordinary pass metrics are invalidated by the owner of the tree or native window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GeometryEnvironment {
    overridden: EnvReads,
    dpi_bits: u64,
    viewport_bits: [u32; 2],
    frame_time: Instant,
}

impl GeometryEnvironment {
    pub(super) fn matches(&self, other: &Self, dependencies: EnvReads) -> bool {
        // The pass owner handles inherited changes. Compare the effective values of
        // inputs supplied locally on either call, if this result actually used them.
        let local = (self.overridden | other.overridden) & dependencies;
        (!local.contains(EnvReads::DPI) || self.dpi_bits == other.dpi_bits)
            && (!local.contains(EnvReads::VIEWPORT_SIZE)
                || self.viewport_bits == other.viewport_bits)
            && (!local.contains(EnvReads::FRAME_TIME) || self.frame_time == other.frame_time)
    }
}

/// Environment getters take exclusive access to record the current callback's reads.
pub struct Context<'a> {
    windows: &'a mut WindowRegistry,
    viewport_size: [f32; 2],
    dpi: f64,
    frame_time: Instant,
    scope: TreeScope,
    window_factory: Option<&'a mut WindowFactory<'a>>,
    window_error: Option<&'a mut Option<WindowError>>,
    services: ServicesStorage<'a>,
    env_reads: Option<&'a mut EnvReads>,
    overrides: EnvReads,
}

impl<'a> Context<'a> {
    pub fn new(windows: &'a mut WindowRegistry) -> Self {
        Self::from_storage(windows, ServicesStorage::Owned(Services::default()))
    }

    pub(crate) fn with_services(
        windows: &'a mut WindowRegistry,
        services: &'a mut Services,
    ) -> Self {
        Self::from_storage(windows, ServicesStorage::Borrowed(services))
    }

    fn from_storage(windows: &'a mut WindowRegistry, services: ServicesStorage<'a>) -> Self {
        Self {
            windows,
            viewport_size: [0.0, 0.0],
            dpi: 1.0,
            frame_time: Instant::now(),
            scope: TreeScope::Standalone,
            window_factory: None,
            window_error: None,
            services,
            env_reads: None,
            overrides: EnvReads::empty(),
        }
    }

    /// Borrow the application's window registry for this operation.
    pub fn window_registry(&mut self) -> &mut WindowRegistry {
        self.windows
    }

    /// Lend window creation to this reconciliation pass, including nested declarations.
    pub fn with_window_factory<'b>(&'b mut self, open: &'b mut WindowFactory<'b>) -> Context<'b> {
        let mut scoped = self.reborrow();
        scoped.window_factory = Some(open);
        scoped
    }

    pub(crate) fn for_window_declarations<'b>(
        &'b mut self,
        open: &'b mut WindowFactory<'b>,
    ) -> Context<'b> {
        let mut scoped = self.with_window_factory(open);
        scoped.scope = TreeScope::WindowDeclarations;
        scoped
    }

    pub(super) fn can_poll_widgets(&self) -> bool {
        !matches!(self.scope, TreeScope::WindowDeclarations)
    }

    /// Infallible widget callbacks can declare multiple windows. Report the first failure
    /// once reconciliation has committed the successful mounts.
    pub(super) fn capture_window_errors(
        &mut self,
        reconcile: impl FnOnce(&mut Context<'_>),
    ) -> Result<(), WindowError> {
        let mut error = None;
        {
            let mut scoped = self.reborrow();
            scoped.window_error = Some(&mut error);
            reconcile(&mut scoped);
        }
        error.map_or(Ok(()), Err)
    }

    pub(super) fn open_window(&mut self, config: &WindowConfig) -> Option<Window> {
        let result = match &mut self.window_factory {
            Some(open) => open(config),
            None => Err(WindowError::BackendError(
                "window declaration requires a window factory".into(),
            )),
        };
        match result {
            Ok(window) => Some(window),
            Err(error) => {
                self.window_error
                    .as_deref_mut()
                    .expect("window creation during reconciliation")
                    .get_or_insert(error);
                None
            }
        }
    }

    /// Reborrow services for one node. Metrics are scoped; platform and resource changes persist.
    /// Environment reads remain attributed to the current callback across reborrows.
    /// The scoped context cannot escape the parent borrow, even if the registry lives longer.
    /// ```compile_fail
    /// use matcha_better_tree::ui_tree::{context::Context, window::WindowRegistry};
    /// let mut registry = WindowRegistry::default();
    /// let mut escaped;
    /// {
    ///     let mut parent = Context::new(&mut registry);
    ///     escaped = parent.reborrow();
    /// }
    /// let _ = escaped.dpi();
    /// ```
    pub fn reborrow(&mut self) -> Context<'_> {
        Context {
            windows: &mut *self.windows,
            viewport_size: self.viewport_size,
            dpi: self.dpi,
            frame_time: self.frame_time,
            scope: self.scope,
            window_factory: self
                .window_factory
                .as_mut()
                .map(|open| &mut **open as &mut WindowFactory<'_>),
            window_error: self.window_error.as_deref_mut(),
            services: ServicesStorage::Borrowed(self.services.get_mut()),
            env_reads: self.env_reads.as_deref_mut(),
            overrides: self.overrides,
        }
    }

    /// Run one callback with a fresh recorder and return its result and direct reads together.
    /// Nested observations are isolated; ordinary reborrows retain the current recorder.
    /// The parent's recorder is never replaced, including when the callback unwinds.
    #[must_use]
    pub(super) fn observe_env_reads<R>(
        &mut self,
        callback: impl FnOnce(&mut Context<'_>) -> R,
    ) -> (R, EnvReads) {
        let mut reads = EnvReads::empty();
        let result = {
            let mut scoped = self.reborrow();
            scoped.env_reads = Some(&mut reads);
            callback(&mut scoped)
        };
        (result, reads)
    }

    /// Tree bookkeeping and unobserved callbacks must not add reads to their caller's record.
    pub(super) fn unobserved(&mut self) -> Context<'_> {
        let mut context = self.reborrow();
        context.env_reads = None;
        context
    }

    fn record_env_read(&mut self, read: EnvReads) {
        if let Some(reads) = self.env_reads.as_deref_mut() {
            *reads |= read;
        }
    }

    fn mark_override(&mut self, metric: EnvReads) {
        if self.env_reads.is_some() {
            self.overrides.insert(metric);
        }
    }

    /// A context passed to a child normally carries its parent's callback recorder.
    /// A widget may instead supply an independent Context, including its own defaults.
    pub(super) fn with_child_geometry<R>(&mut self, run: impl FnOnce(&mut Context<'_>) -> R) -> R {
        if self.env_reads.is_some() {
            return run(self);
        }
        let mut independent = self.reborrow();
        independent.overrides = EnvReads::all();
        run(&mut independent)
    }

    pub(super) fn geometry_environment(&self) -> GeometryEnvironment {
        GeometryEnvironment {
            overridden: self.overrides,
            dpi_bits: self.dpi.to_bits(),
            viewport_bits: self.viewport_size.map(f32::to_bits),
            frame_time: self.frame_time,
        }
    }

    pub(crate) fn with_window(mut self, window: WindowId) -> Self {
        let native = self.windows.window(window).expect("mounted window");
        self.dpi = native.dpi();
        self.viewport_size = native.inner_size().map(|size| size as f32);
        self.overrides
            .remove(EnvReads::DPI | EnvReads::VIEWPORT_SIZE);
        self.scope = TreeScope::Window(window);
        self
    }

    pub fn with_viewport_size(mut self, viewport_size: [f32; 2]) -> Self {
        self.viewport_size = viewport_size;
        self.mark_override(EnvReads::VIEWPORT_SIZE);
        self
    }

    pub fn with_dpi(mut self, dpi: f64) -> Self {
        self.dpi = dpi;
        self.mark_override(EnvReads::DPI);
        self
    }

    pub fn with_frame_time(mut self, frame_time: Instant) -> Self {
        self.frame_time = frame_time;
        self.mark_override(EnvReads::FRAME_TIME);
        self
    }

    /// Synchronize any changed cursor icon or IME state to the OS window.
    /// For a declared Window, use [`WindowRegistry::sync_to_window`]
    /// after a tree pass instead.
    pub fn sync_to_window(&mut self, window: &Window) {
        self.platform_mut().sync_to_window(window);
    }

    fn platform(&self) -> &PlatformState {
        match self.scope {
            TreeScope::Window(id) => self.windows.platform(id),
            TreeScope::Standalone | TreeScope::WindowDeclarations => &self.services.get().platform,
        }
    }

    fn platform_mut(&mut self) -> &mut PlatformState {
        match self.scope {
            TreeScope::Window(id) => self.windows.platform_mut(id),
            TreeScope::Standalone | TreeScope::WindowDeclarations => {
                &mut self.services.get_mut().platform
            }
        }
    }
}

impl Context<'_> {
    // --- Window & Metrics ---

    pub fn viewport_size(&mut self) -> [f32; 2] {
        self.record_env_read(EnvReads::VIEWPORT_SIZE);
        self.viewport_size
    }

    pub fn dpi(&mut self) -> f64 {
        self.record_env_read(EnvReads::DPI);
        self.dpi
    }

    pub fn scale_factor(&mut self) -> f32 {
        self.dpi() as f32
    }

    pub fn frame_time(&mut self) -> Instant {
        self.record_env_read(EnvReads::FRAME_TIME);
        self.frame_time
    }

    // --- Cursor ---

    pub fn set_cursor_icon(&mut self, icon: CursorIcon) {
        self.platform_mut().cursor_icon = icon;
    }

    pub fn cursor_icon(&self) -> CursorIcon {
        self.platform().cursor_icon
    }

    // --- IME ---

    pub fn set_ime_allowed(&mut self, allowed: bool) {
        self.platform_mut().ime_allowed = allowed;
    }

    pub fn is_ime_allowed(&self) -> bool {
        self.platform().ime_allowed
    }

    pub fn set_ime_cursor_area(&mut self, position: [f32; 2], size: [f32; 2]) {
        self.platform_mut().ime_cursor_area = Some((position, size));
    }

    pub fn ime_cursor_area(&self) -> Option<([f32; 2], [f32; 2])> {
        self.platform().ime_cursor_area
    }

    // --- Clipboard ---

    /// Lazily initializes and returns the system clipboard handle.
    pub fn clipboard(&self) -> &Clipboard {
        self.services.get().clipboard.get_or_init(Clipboard::new)
    }

    // --- Shared Resources ---

    pub fn insert_resource<R: Any + Send + Sync>(&mut self, resource: R) -> Arc<R> {
        let arc = Arc::new(resource);
        self.services.get_mut().resources.insert(
            TypeId::of::<R>(),
            Arc::clone(&arc) as Arc<dyn Any + Send + Sync>,
        );
        arc
    }

    pub fn resource<R: Any + Send + Sync>(&self) -> Option<Arc<R>> {
        self.services
            .get()
            .resources
            .get(&TypeId::of::<R>())
            .and_then(|entry| Arc::clone(entry).downcast::<R>().ok())
    }

    pub fn get_or_init_resource<R: Any + Send + Sync>(
        &mut self,
        init: impl FnOnce() -> R,
    ) -> Arc<R> {
        let entry = self
            .services
            .get_mut()
            .resources
            .entry(TypeId::of::<R>())
            .or_insert_with(|| Arc::new(init()));
        Arc::clone(entry)
            .downcast::<R>()
            .expect("resource entry matches TypeId::of::<R>()")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::Arena;
    use matcha_window::headless_interface::HeadlessEventLoop;
    use matcha_window::window::WindowConfig as OsWindowConfig;

    #[test]
    fn child_geometry_inputs_follow_widget_overrides_across_reborrows() {
        let mut registry = WindowRegistry::default();
        let time = Instant::now();
        let mut root = Context::new(&mut registry)
            .with_dpi(1.5)
            .with_viewport_size([80.0, 60.0])
            .with_frame_time(time);
        let inherited = root.geometry_environment();
        assert!(inherited.overridden.is_empty());

        let ((), reads) = root.observe_env_reads(|ctx| {
            let mut child = ctx.reborrow().with_dpi(1.5);
            let dpi = child.geometry_environment();
            assert_eq!(dpi.overridden, EnvReads::DPI);
            assert!(dpi.matches(&inherited, EnvReads::all()));
            assert_eq!(child.unobserved().geometry_environment(), dpi);
            assert_ne!(child.reborrow().with_dpi(2.0).geometry_environment(), dpi);
            let viewport = child
                .reborrow()
                .with_viewport_size([80.0, 60.0])
                .geometry_environment();
            assert_ne!(viewport, dpi);
            assert_ne!(
                child
                    .reborrow()
                    .with_frame_time(time)
                    .geometry_environment(),
                dpi
            );
            assert_eq!(child.geometry_environment(), dpi);
        });
        assert!(reads.is_empty());
        assert_eq!(root.geometry_environment(), inherited);
    }

    #[test]
    fn native_window_replaces_local_metrics_and_inherits_pass_time() {
        let mut arena = Arena::<()>::new();
        let mut registry = WindowRegistry::default();
        let event_loop = HeadlessEventLoop::new();
        let window = Window::new(&OsWindowConfig::default(), &event_loop).unwrap();
        let id = registry.register(arena.reserve(), window);
        let mut root = Context::new(&mut registry);
        let time = Instant::now();
        let ((), reads) = root.observe_env_reads(|ctx| {
            let mut scoped = ctx.reborrow().with_frame_time(time);
            let inherited_time = scoped.geometry_environment();
            let native = scoped
                .reborrow()
                .with_dpi(2.0)
                .with_viewport_size([9.0, 9.0])
                .with_window(id);
            let native = native.geometry_environment();
            assert_eq!(native.overridden, EnvReads::FRAME_TIME);
            assert!(native.matches(&inherited_time, EnvReads::all()));
        });
        assert!(reads.is_empty());
    }

    #[test]
    fn scopes_borrow_services_and_preserve_resource_updates_after_returning() {
        let mut registry = WindowRegistry::default();
        let mut parent = Context::new(&mut registry).with_dpi(1.5);
        let original = parent.insert_resource(String::from("original"));
        {
            let mut child = parent.reborrow().with_dpi(2.0);
            assert_eq!(Arc::strong_count(&original), 2);
            assert!(Arc::ptr_eq(&child.resource::<String>().unwrap(), &original));
            child.insert_resource(String::from("replacement"));
            child.set_cursor_icon(CursorIcon::Text);
            let initialized = child.get_or_init_resource(|| vec![1_u32, 2]);
            let mut nested = child.reborrow();
            assert!(Arc::ptr_eq(
                &nested.get_or_init_resource::<Vec<u32>>(|| panic!("already initialized")),
                &initialized,
            ));
            nested.set_ime_allowed(true);
        }
        assert_eq!(parent.dpi(), 1.5);
        assert_eq!(parent.cursor_icon(), CursorIcon::Text);
        assert!(parent.is_ime_allowed());
        assert_eq!(&*parent.resource::<String>().unwrap(), "replacement");
        assert_eq!(&*original, "original");
        assert_eq!(Arc::strong_count(&original), 1);
        assert_eq!(&*parent.resource::<Vec<u32>>().unwrap(), &[1, 2]);
    }

    #[test]
    fn window_platform_state_survives_contexts_and_flushes_pending_changes() {
        let mut arena = Arena::<()>::new();
        let mut registry = WindowRegistry::default();
        let event_loop = HeadlessEventLoop::new();
        let config = OsWindowConfig::default();
        let a = registry.register(arena.reserve(), Window::new(&config, &event_loop).unwrap());
        let b = registry.register(arena.reserve(), Window::new(&config, &event_loop).unwrap());
        let area = ([10.0, 20.0], [30.0, 40.0]);
        {
            let mut parent = Context::new(&mut registry);
            let mut window_a = parent.reborrow().with_window(a);
            window_a.set_cursor_icon(CursorIcon::Text);
            window_a.set_ime_allowed(true);
            window_a.set_ime_cursor_area(area.0, area.1);
            {
                let window_b = window_a.reborrow().with_window(b);
                assert_eq!(window_b.cursor_icon(), CursorIcon::Default);
                assert!(!window_b.is_ime_allowed());
                assert_eq!(window_b.ime_cursor_area(), None);
            }
            assert_eq!(window_a.cursor_icon(), CursorIcon::Text);
            assert!(window_a.is_ime_allowed());
        }

        assert!(!registry.platform(a).pushed_ime_allowed);
        registry.sync_to_window(a);
        assert_eq!(registry.platform(a).pushed_cursor_icon, CursorIcon::Text);
        assert!(registry.platform(a).pushed_ime_allowed);
        assert_eq!(registry.platform(a).pushed_ime_cursor_area, Some(area));
        assert!(!registry.platform(b).pushed_ime_allowed);
        {
            let mut parent = Context::new(&mut registry);
            assert!(!parent.is_ime_allowed());
            let window_a = parent.reborrow().with_window(a);
            assert!(window_a.is_ime_allowed());
            assert_eq!(window_a.ime_cursor_area(), Some(area));
        }
    }
}
