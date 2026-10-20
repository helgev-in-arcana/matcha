//! Registry-owned window instances, native lifecycle, and presentation.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use matcha_window::adapter::EventLoop;
use matcha_window::window::{Window as OsWindow, WindowError, WindowId};
use parking_lot::{MappedMutexGuard, Mutex, MutexGuard};
use plain_renderer::{PlainRenderer, PlainTarget};

use super::WindowConfig;
use crate::arena::Id as UiId;
use crate::context::{Context, PlatformState};
use crate::widget::ChildId;

/// Opaque runtime ID, available before native creation and stable until removal.
/// IDs are never reused, including across registries, so stale IDs cannot resolve
/// to a replacement or to a window in a different application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowInstanceId(u64);

impl WindowInstanceId {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(
            NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
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
    entries: Mutex<WindowEntries>,
}

#[derive(Default)]
struct WindowEntries {
    instances: HashMap<WindowInstanceId, WindowEntry>,
    native: HashMap<WindowId, WindowInstanceId>,
}

struct WindowEntry {
    node: UiId,
    instance: WindowInstance,
}

impl WindowRegistry {
    pub(crate) fn register(
        &self,
        node: UiId,
        config: WindowConfig,
        content: ChildId,
    ) -> WindowInstanceId {
        let id = WindowInstanceId::new();
        let instance = WindowInstance {
            config,
            content,
            window: None,
            platform: Arc::new(Mutex::new(PlatformState::default())),
            renderer: None,
        };
        self.entries
            .lock()
            .instances
            .insert(id, WindowEntry { node, instance });
        id
    }

    /// Called by UiTree on removal/replacement while application Context is available.
    pub(crate) fn unregister(&self, id: WindowInstanceId) {
        let removed = {
            let mut entries = self.entries.lock();
            let removed = entries.instances.remove(&id);
            if let Some(window_id) = removed
                .as_ref()
                .and_then(|entry| entry.instance.window_id())
            {
                entries.native.remove(&window_id);
            }
            removed
        };
        // Release backend resources after releasing the registry lock.
        drop(removed);
    }

    /// Snapshot of registered IDs, including windows awaiting native creation.
    pub fn ids(&self) -> Vec<WindowInstanceId> {
        self.entries.lock().instances.keys().copied().collect()
    }

    /// Borrow an instance from the registry. Drop the guard before calling widget
    /// callbacks or accessing the registry again; it holds the registry's lock.
    /// The borrowed instance cannot be retained independently of its registry:
    /// ```compile_fail
    /// use matcha_better_tree::window::{WindowInstance, WindowInstanceId, WindowRegistry};
    /// use parking_lot::MappedMutexGuard;
    /// fn detach(registry: &WindowRegistry, id: WindowInstanceId)
    ///     -> MappedMutexGuard<'static, WindowInstance>
    /// {
    ///     registry.get(id).unwrap()
    /// }
    /// ```
    pub fn get(&self, id: WindowInstanceId) -> Option<MappedMutexGuard<'_, WindowInstance>> {
        MutexGuard::try_map(self.entries.lock(), |entries| {
            entries
                .instances
                .get_mut(&id)
                .map(|entry| &mut entry.instance)
        })
        .ok()
    }

    /// Translate a native event's ID into the ID stored by the Window widget.
    pub fn id_for_window(&self, window_id: WindowId) -> Option<WindowInstanceId> {
        self.entries.lock().native.get(&window_id).copied()
    }

    /// Resolve the declaration node and its content, without assuming arena order.
    /// TODO(app): use this for window-local layout, rendering and input traversal.
    pub(crate) fn location(&self, window_id: WindowId) -> Option<(UiId, ChildId)> {
        let entries = self.entries.lock();
        let entry = entries.instances.get(entries.native.get(&window_id)?)?;
        Some((entry.node, entry.instance.content))
    }

    /// Create pending native windows with application-supplied surface settings.
    /// Existing windows are left intact. On error, successfully created windows
    /// remain registered and a later call retries the remaining pending windows.
    /// Surface attachment is a separate application lifecycle operation.
    pub fn create_pending(
        &self,
        event_loop: &dyn EventLoop,
        surface_config: &wgpu::SurfaceConfiguration,
    ) -> Result<(), WindowError> {
        let mut entries = self.entries.lock();
        let WindowEntries { instances, native } = &mut *entries;
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
    pub(super) content: ChildId,
    window: Option<OsWindow>,
    platform: Arc<Mutex<PlatformState>>,
    renderer: Option<PlainRenderer>,
}

impl WindowInstance {
    pub fn window_id(&self) -> Option<WindowId> {
        self.window.as_ref().map(OsWindow::id)
    }

    pub fn window(&self) -> Option<&OsWindow> {
        self.window.as_ref()
    }

    pub fn config(&self) -> &WindowConfig {
        &self.config
    }

    /// Current physical viewport and DPI, sharing resources with the application
    /// but keeping cursor/IME state local to this window. The caller lends the
    /// registry and supplies frame time through parent; neither is stored here.
    pub fn context<'a>(&self, parent: Context<'a>) -> Context<'a> {
        let mut ctx = parent.for_window(Arc::clone(&self.platform));
        if let Some(window) = &self.window {
            let [width, height] = window.inner_size();
            ctx = ctx
                .with_viewport_size([width as f32, height as f32])
                .with_dpi(window.dpi())
                .with_window_config(window.surface().get_config());
        }
        ctx
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

    /// Present a Scene assembled from this window's content in the shared UiTree.
    /// Returns false for a pending, zero-size or currently surfaceless window.
    /// App remains responsible for layout, Scene assembly and cache invalidation.
    pub fn render(
        &mut self,
        scene: &render_interface::Scene,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<bool, WindowRenderError> {
        let Some(window) = &self.window else {
            return Ok(false);
        };
        let size = window.inner_size();
        if size.contains(&0) {
            return Ok(false);
        }
        let surface = window.surface();
        let Some(texture) = surface
            .get_surface_texture(device)
            .map_err(|error| WindowRenderError::Surface(error.to_string()))?
        else {
            return Ok(false);
        };
        let view = surface.create_render_view(&texture.texture);
        let region = render_interface::TextureRegion::whole(&view, window.render_format())
            .map_err(|error| plain_renderer::PlainError::Invalid(error.to_string()))?;
        let renderer = self
            .renderer
            .get_or_insert_with(|| PlainRenderer::new(device, queue));
        renderer.render(
            scene,
            PlainTarget {
                region,
                logical_size: [size[0] as f32, size[1] as f32],
                clear: wgpu::Color {
                    r: 0.1,
                    g: 0.1,
                    b: 0.1,
                    a: 1.0,
                },
                initial: None,
            },
        )?;
        texture.present();
        Ok(true)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WindowRenderError {
    #[error("window surface unavailable: {0}")]
    Surface(String),
    #[error(transparent)]
    Renderer(#[from] plain_renderer::PlainError),
}
