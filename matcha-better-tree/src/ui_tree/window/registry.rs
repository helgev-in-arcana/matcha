//! Registry-owned native windows, registration identity, and scoped platform services.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use matcha_window::adapter::EventLoop;
use matcha_window::window::{Window as OsWindow, WindowError, WindowId};

use super::WindowConfig;
use crate::CacheEpoch;
use crate::arena::Id as UiId;
use crate::ui_tree::context::{Context, PlatformState};

/// Opaque runtime ID, available before native creation and stable until removal.
/// IDs are never reused, including across registries, so stale IDs cannot resolve
/// to a replacement or to a window in a different application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowInstanceId(u64);

impl WindowInstanceId {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(
            NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("window instance ID exhausted"),
        )
    }
}

/// Framework-owned storage, reached by widgets through their declaration Context.
///
/// App owns this value and lends it to Context during tree operations. One
/// registry serves one UiTree because arena IDs are tree-local. Removing a node
/// unregisters its window through Context. Dropping the registry releases every
/// remaining window; dropping an ID or UiTree alone does not release these resources.
#[derive(Default)]
pub struct WindowRegistry {
    instances: HashMap<WindowInstanceId, WindowEntry>,
    native: HashMap<WindowId, WindowInstanceId>,
}

struct WindowEntry {
    node: UiId,
    instance: WindowInstance,
}

impl WindowRegistry {
    pub(crate) fn register(&mut self, node: UiId, config: WindowConfig) -> WindowInstanceId {
        let id = WindowInstanceId::new();
        let instance = WindowInstance {
            config,
            window: None,
            platform: PlatformState::default(),
        };
        self.instances.insert(id, WindowEntry { node, instance });
        id
    }
    /// Called by UiTree on removal/replacement while application Context is available.
    pub(crate) fn unregister(&mut self, id: WindowInstanceId) {
        if let Some(entry) = self.instances.remove(&id)
            && let Some(native) = entry.instance.window_id()
        {
            self.native.remove(&native);
        }
    }
    /// Snapshot of registered IDs, including windows awaiting native creation.
    pub fn ids(&self) -> Vec<WindowInstanceId> {
        self.instances.keys().copied().collect()
    }

    /// Borrow an instance from the registry. End the borrow before widget callbacks
    /// that may access the registry again.
    /// The borrowed instance cannot be retained independently of its registry:
    /// ```compile_fail
    /// use matcha_better_tree::ui_tree::window::{WindowInstance, WindowInstanceId, WindowRegistry};
    /// fn detach(registry: &mut WindowRegistry, id: WindowInstanceId)
    ///     -> &'static mut WindowInstance
    /// {
    ///     registry.get_mut(id).unwrap()
    /// }
    /// ```
    pub fn get_mut(&mut self, id: WindowInstanceId) -> Option<&mut WindowInstance> {
        self.instances.get_mut(&id).map(|entry| &mut entry.instance)
    }

    pub(crate) fn platform(&self, id: WindowInstanceId) -> &PlatformState {
        &self
            .instances
            .get(&id)
            .expect("live window scope")
            .instance
            .platform
    }

    pub(crate) fn platform_mut(&mut self, id: WindowInstanceId) -> &mut PlatformState {
        &mut self
            .instances
            .get_mut(&id)
            .expect("live window scope")
            .instance
            .platform
    }

    /// Translate a native event's ID into the ID stored by the Window widget.
    pub fn id_for_window(&self, window_id: WindowId) -> Option<WindowInstanceId> {
        self.native.get(&window_id).copied()
    }

    /// Resolve the declaration node without assuming arena order.
    #[cfg(test)]
    pub(crate) fn location(&self, window_id: WindowId) -> Option<UiId> {
        self.route(window_id).map(|(node, _)| node)
    }

    pub(crate) fn route(&self, window_id: WindowId) -> Option<(UiId, WindowInstanceId)> {
        let id = *self.native.get(&window_id)?;
        let entry = self.instances.get(&id)?;
        Some((entry.node, id))
    }
    /// Create pending native windows with application-supplied surface settings.
    /// Existing windows are left intact. On error, successfully created windows
    /// remain registered and a later call retries the remaining pending windows.
    /// Surface attachment is a separate application lifecycle operation.
    pub fn create_pending(
        &mut self,
        event_loop: &dyn EventLoop,
        surface_config: &wgpu::SurfaceConfiguration,
    ) -> Result<(), WindowError> {
        let Self { instances, native } = self;
        for (&id, entry) in instances.iter_mut() {
            let state = &mut entry.instance;
            if state.window.is_none() {
                state.create_window(event_loop, surface_config.clone())?;
                let window_id = state
                    .window_id()
                    .expect("successful native window creation");
                native.insert(window_id, id);
            }
        }
        Ok(())
    }
}

/// Per-window runtime state. There is no nested UiTree or owned child widget here.
/// Focus/capture state will also live here when the application input route is added.
pub struct WindowInstance {
    config: WindowConfig,
    window: Option<OsWindow>,
    platform: PlatformState,
}

impl WindowInstance {
    /// Snapshot native metrics, then select this instance's platform state in a reborrowed Context.
    pub(super) fn scoped_context<'a>(
        id: WindowInstanceId,
        epoch: CacheEpoch,
        dpi: Option<f64>,
        viewport: Option<[f32; 2]>,
        parent: &'a mut Context<'_>,
    ) -> Context<'a> {
        let instance = parent
            .window_registry()
            .get_mut(id)
            .expect("window node belongs to this context's registry");
        let dpi = dpi.unwrap_or_else(|| instance.window().map_or(1.0, |w| w.dpi()));
        let viewport = viewport.unwrap_or_else(|| {
            instance
                .window()
                .map_or([0.0; 2], |w| w.inner_size().map(|n| n as f32))
        });
        parent
            .with_cache_epoch(epoch)
            .with_dpi(dpi)
            .with_viewport_size(viewport)
            .with_window(id)
    }

    pub fn window_id(&self) -> Option<WindowId> {
        self.window.as_ref().map(OsWindow::id)
    }

    pub fn window(&self) -> Option<&OsWindow> {
        self.window.as_ref()
    }

    pub fn config(&self) -> &WindowConfig {
        &self.config
    }

    fn create_window(
        &mut self,
        event_loop: &dyn EventLoop,
        surface_config: wgpu::SurfaceConfiguration,
    ) -> Result<(), WindowError> {
        let config = self.config.clone().into_inner_config(surface_config);
        let window = OsWindow::new(&config, event_loop)?;
        window.request_redraw();
        self.window = Some(window);
        Ok(())
    }

    pub(super) fn update_config(&mut self, config: WindowConfig) {
        if let Some(window) = &self.window {
            config.apply_updates(window);
        }
        // Pending creation uses the latest declaration, including Init values.
        self.config = config;
    }

    pub fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Apply cursor and IME changes accumulated by this window's widgets.
    /// The application calls this after its tree passes, before yielding to the event loop.
    /// Pending windows retain the desired state until a native window exists.
    pub fn sync_to_window(&mut self) {
        if let Some(window) = &self.window {
            self.platform.sync_to_window(window);
        }
    }

    pub fn create_surface(
        &mut self,
        instance: &wgpu::Instance,
        device: &wgpu::Device,
    ) -> Result<(), WindowError> {
        if let Some(window) = &mut self.window {
            window.create_surface(instance, device)?;
            window.request_redraw();
        }
        Ok(())
    }

    pub fn destroy_surface(&mut self) {
        if let Some(window) = &mut self.window {
            window.destroy_surface();
        }
    }
}
