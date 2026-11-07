use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use matcha_window::clipboard::Clipboard;
use matcha_window::window::{CursorIcon, Window};

use super::env_reads::EnvReads;
use crate::ui_tree::window::{WindowInstanceId, WindowRegistry};

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
struct Services {
    platform: PlatformState,
    clipboard: OnceLock<Clipboard>,
    resources: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

/// A root Context owns services; nested scopes exclusively reborrow that same storage.
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

/// Environment getters take exclusive access to record the current callback's reads.
pub struct Context<'a> {
    windows: &'a mut WindowRegistry,
    viewport_size: [f32; 2],
    dpi: f64,
    frame_time: Instant,
    window: Option<WindowInstanceId>,
    services: ServicesStorage<'a>,
    env_reads: Option<&'a mut EnvReads>,
}

impl<'a> Context<'a> {
    pub fn new(windows: &'a mut WindowRegistry) -> Self {
        Self {
            windows,
            viewport_size: [0.0, 0.0],
            dpi: 1.0,
            frame_time: Instant::now(),
            window: None,
            services: ServicesStorage::Owned(Services::default()),
            env_reads: None,
        }
    }

    /// Borrow the application's window registry for this operation.
    pub fn window_registry(&mut self) -> &mut WindowRegistry {
        self.windows
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
            window: self.window,
            services: ServicesStorage::Borrowed(self.services.get_mut()),
            env_reads: self.env_reads.as_deref_mut(),
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

    pub(crate) fn with_window(mut self, window: WindowInstanceId) -> Self {
        self.window = Some(window);
        self
    }

    pub fn with_viewport_size(mut self, viewport_size: [f32; 2]) -> Self {
        self.viewport_size = viewport_size;
        self
    }

    pub fn with_dpi(mut self, dpi: f64) -> Self {
        self.dpi = dpi;
        self
    }

    pub fn with_frame_time(mut self, frame_time: Instant) -> Self {
        self.frame_time = frame_time;
        self
    }

    /// Synchronize any changed cursor icon or IME state to the OS window.
    /// For a declared Window, use [`crate::ui_tree::window::WindowInstance::sync_to_window`]
    /// after a tree pass instead.
    pub fn sync_to_window(&mut self, window: &Window) {
        self.platform_mut().sync_to_window(window);
    }

    fn platform(&self) -> &PlatformState {
        match self.window {
            Some(id) => self.windows.platform(id),
            None => &self.services.get().platform,
        }
    }

    fn platform_mut(&mut self) -> &mut PlatformState {
        match self.window {
            Some(id) => self.windows.platform_mut(id),
            None => &mut self.services.get_mut().platform,
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
    use crate::ui_tree::window::WindowConfig;
    use matcha_window::headless_interface::HeadlessEventLoop;

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
        let a = registry.register(arena.reserve(), WindowConfig::default());
        let b = registry.register(arena.reserve(), WindowConfig::default());
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

        // Sync before native creation must preserve pending changes.
        registry.get_mut(a).unwrap().sync_to_window();
        assert!(!registry.platform(a).pushed_ime_allowed);
        registry
            .create_pending(
                &HeadlessEventLoop::new(),
                &matcha_window::window::WindowConfig::default().surface_config,
            )
            .unwrap();
        registry.get_mut(a).unwrap().sync_to_window();
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
