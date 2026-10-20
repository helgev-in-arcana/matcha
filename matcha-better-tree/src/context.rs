use std::any::{Any, TypeId};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use dashmap::DashMap;
use matcha_window::clipboard::Clipboard;
use matcha_window::window::{CursorIcon, Window, WindowConfig};
use parking_lot::Mutex;

use crate::window::WindowRegistry;

#[derive(Default)]
pub(crate) struct PlatformState {
    cursor_icon: CursorIcon,
    pushed_cursor_icon: CursorIcon,

    ime_allowed: bool,
    pushed_ime_allowed: bool,

    ime_cursor_area: Option<([f32; 2], [f32; 2])>,
    pushed_ime_cursor_area: Option<([f32; 2], [f32; 2])>,
}

struct SharedContext {
    clipboard: OnceLock<Clipboard>,
    resources: DashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl Default for SharedContext {
    fn default() -> Self {
        Self {
            clipboard: OnceLock::new(),
            resources: DashMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct Context<'a> {
    windows: Option<&'a WindowRegistry>,
    viewport_size: [f32; 2],
    dpi: f64,
    frame_time: Instant,
    window_config: Option<Arc<WindowConfig>>,
    platform: Arc<Mutex<PlatformState>>,
    shared: Arc<SharedContext>,
}

impl Default for Context<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Context<'a> {
    pub fn new() -> Self {
        Self {
            windows: None,
            viewport_size: [0.0, 0.0],
            dpi: 1.0,
            frame_time: Instant::now(),
            window_config: None,
            platform: Arc::new(Mutex::new(PlatformState::default())),
            shared: Arc::new(SharedContext::default()),
        }
    }

    pub fn with_window_config(mut self, config: WindowConfig) -> Self {
        self.window_config = Some(Arc::new(config));
        self
    }

    /// Borrow the application's registry for the duration of a tree operation.
    /// Context clones copy this reference; they do not share ownership of it.
    ///
    /// A borrowed context cannot outlive the application's registry:
    /// ```compile_fail
    /// use matcha_better_tree::{context::Context, window::WindowRegistry};
    /// let context;
    /// {
    ///     let registry = WindowRegistry::default();
    ///     context = Context::new().with_window_registry(&registry);
    /// }
    /// assert!(context.window_registry().is_some());
    /// ```
    pub fn with_window_registry<'b>(self, windows: &'b WindowRegistry) -> Context<'b> {
        Context {
            windows: Some(windows),
            viewport_size: self.viewport_size,
            dpi: self.dpi,
            frame_time: self.frame_time,
            window_config: self.window_config,
            platform: self.platform,
            shared: self.shared,
        }
    }

    /// None for a standalone/offscreen context without application window services.
    pub fn window_registry(&self) -> Option<&'a WindowRegistry> {
        self.windows
    }

    pub(crate) fn for_window(&self, platform: Arc<Mutex<PlatformState>>) -> Self {
        Self {
            viewport_size: [0.0, 0.0],
            dpi: 1.0,
            window_config: None,
            platform,
            ..self.clone()
        }
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
    pub fn sync_to_window(&self, window: &Window) {
        let mut state = self.platform.lock();

        if state.cursor_icon != state.pushed_cursor_icon {
            window.set_cursor_icon(state.cursor_icon);
            state.pushed_cursor_icon = state.cursor_icon;
        }

        let allowed_changed = state.ime_allowed != state.pushed_ime_allowed;
        let area_changed = state.ime_cursor_area != state.pushed_ime_cursor_area;

        if allowed_changed {
            window.set_ime_allowed(state.ime_allowed);
            state.pushed_ime_allowed = state.ime_allowed;
        }

        if state.ime_allowed
            && (allowed_changed || area_changed)
            && let Some((pos, size)) = state.ime_cursor_area
        {
            window.set_ime_cursor_area(pos, size);
            state.pushed_ime_cursor_area = state.ime_cursor_area;
        } else if !state.ime_allowed && area_changed {
            state.pushed_ime_cursor_area = state.ime_cursor_area;
        }
    }
}

impl Context<'_> {
    // --- Window & Metrics ---

    pub fn viewport_size(&self) -> [f32; 2] {
        self.viewport_size
    }

    pub fn dpi(&self) -> f64 {
        self.dpi
    }

    pub fn scale_factor(&self) -> f32 {
        self.dpi as f32
    }

    pub fn frame_time(&self) -> Instant {
        self.frame_time
    }

    pub fn window_config(&self) -> Option<&WindowConfig> {
        self.window_config.as_deref()
    }

    // --- Cursor ---

    pub fn set_cursor_icon(&self, icon: CursorIcon) {
        self.platform.lock().cursor_icon = icon;
    }

    pub fn cursor_icon(&self) -> CursorIcon {
        self.platform.lock().cursor_icon
    }

    // --- IME ---

    pub fn set_ime_allowed(&self, allowed: bool) {
        self.platform.lock().ime_allowed = allowed;
    }

    pub fn is_ime_allowed(&self) -> bool {
        self.platform.lock().ime_allowed
    }

    pub fn set_ime_cursor_area(&self, position: [f32; 2], size: [f32; 2]) {
        self.platform.lock().ime_cursor_area = Some((position, size));
    }

    pub fn ime_cursor_area(&self) -> Option<([f32; 2], [f32; 2])> {
        self.platform.lock().ime_cursor_area
    }

    // --- Clipboard ---

    /// Lazily initializes and returns the system clipboard handle.
    pub fn clipboard(&self) -> &Clipboard {
        self.shared.clipboard.get_or_init(Clipboard::new)
    }

    // --- Shared Resources ---

    pub fn insert_resource<R: Any + Send + Sync>(&self, resource: R) -> Arc<R> {
        let arc = Arc::new(resource);
        self.shared
            .resources
            .insert(TypeId::of::<R>(), Arc::clone(&arc) as Arc<dyn Any + Send + Sync>);
        arc
    }

    pub fn resource<R: Any + Send + Sync>(&self) -> Option<Arc<R>> {
        self.shared
            .resources
            .get(&TypeId::of::<R>())
            .and_then(|entry| Arc::clone(entry.value()).downcast::<R>().ok())
    }

    pub fn get_or_init_resource<R: Any + Send + Sync>(
        &self,
        init: impl FnOnce() -> R,
    ) -> Arc<R> {
        let entry = self
            .shared
            .resources
            .entry(TypeId::of::<R>())
            .or_insert_with(|| Arc::new(init()));
        Arc::clone(entry.value())
            .downcast::<R>()
            .expect("resource entry matches TypeId::of::<R>()")
    }
}