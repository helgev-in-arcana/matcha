//! Headless backend: [`HeadlessWindow`] implements [`NativeWindow`] entirely
//! in memory — no OS window, no wgpu surface. `create_wgpu_surface` returns
//! `Ok(None)`, so a headless [`WindowSurface`] stays surfaceless forever and
//! every frame is skipped through the normal `get_surface_texture -> Ok(None)`
//! path. Intended for driving the full app pipeline (input dispatch, layout,
//! model updates) in tests without a display or GPU.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::surface::{NativeWindow, WindowSurface, WindowSurfaceError};
use super::window_config::{Size, WindowConfig};

/// Mints unique headless window ids. Starts far above any plausible winit id
/// so the two kinds can never collide within one process (winit ids are small
/// on every platform in practice; headless and real windows coexisting is an
/// unsupported scenario anyway).
static NEXT_HEADLESS_ID: AtomicUsize = AtomicUsize::new(usize::MAX / 2);

/// An in-memory stand-in for an OS window (DPI fixed at 1.0, never visible).
pub(crate) struct HeadlessWindow {
    id: super::WindowId,
    title: parking_lot::Mutex<String>,
    inner_size: parking_lot::Mutex<[u32; 2]>,
    config: WindowConfig,
    /// Number of `request_redraw` calls, observable by tests.
    redraw_requests: AtomicUsize,
}

/// Headless constructor
impl WindowSurface {
    /// Creates a surfaceless in-memory window. `create_surface` is a no-op on
    /// it; rendering skips every frame.
    pub(crate) fn new_headless(config: &WindowConfig) -> Self {
        let inner_size = match config.inner_size {
            Some(Size::Physical { width, height }) => [width, height],
            // Headless DPI is fixed at 1.0, so logical == physical.
            Some(Size::Logical { width, height }) => {
                [width.round() as u32, height.round() as u32]
            }
            None => [config.surface_config.width, config.surface_config.height],
        };

        let window = HeadlessWindow {
            id: super::WindowId {
                id: NEXT_HEADLESS_ID.fetch_add(1, Ordering::Relaxed),
            },
            title: parking_lot::Mutex::new(config.title.clone()),
            inner_size: parking_lot::Mutex::new(inner_size),
            config: config.clone(),
            redraw_requests: AtomicUsize::new(0),
        };

        Self::from_native(Box::new(window), config)
    }
}

impl NativeWindow for HeadlessWindow {
    fn id(&self) -> super::WindowId {
        self.id
    }

    fn title(&self) -> String {
        self.title.lock().clone()
    }

    fn set_title(&self, title: &str) {
        *self.title.lock() = title.to_string();
    }

    // --- Size ---

    fn inner_size(&self) -> [u32; 2] {
        *self.inner_size.lock()
    }

    fn request_inner_size(&self, width: u32, height: u32) {
        // No OS to negotiate with: the request is granted immediately.
        *self.inner_size.lock() = [width, height];
    }

    fn outer_size(&self) -> [u32; 2] {
        // No decorations: outer == inner.
        *self.inner_size.lock()
    }

    fn resize_increments(&self) -> Option<[u32; 2]> {
        None
    }

    fn set_resize_increments(&self, _increments: Option<Size>) {}

    fn set_min_inner_size(&self, _min_size: Option<Size>) {}

    fn set_max_inner_size(&self, _max_size: Option<Size>) {}

    // --- Position ---

    fn inner_position(&self) -> Option<[i32; 2]> {
        None
    }

    fn outer_position(&self) -> Option<[i32; 2]> {
        None
    }

    fn set_outer_position(&self, _position: super::Position) {}

    // --- Window state ---

    fn maximized(&self) -> bool {
        false
    }

    fn set_maximized(&self, _maximized: bool) {}

    fn fullscreen(&self) -> Option<super::Fullscreen> {
        None
    }

    fn set_fullscreen(&self, _fullscreen: Option<super::Fullscreen>) {}

    fn is_resizable(&self) -> bool {
        self.config.resizable
    }

    fn set_resizable(&self, _resizable: bool) {}

    fn is_decorated(&self) -> bool {
        self.config.decorations
    }

    fn set_decorations(&self, _decorations: bool) {}

    fn is_visible(&self) -> Option<bool> {
        // Honest: nothing is ever on screen.
        Some(false)
    }

    fn set_visible(&self, _visible: bool) {}

    // --- Appearance ---

    fn theme(&self) -> Option<super::Theme> {
        self.config.preferred_theme
    }

    fn set_theme(&self, _theme: Option<super::Theme>) {}

    fn enabled_buttons(&self) -> super::WindowButtons {
        self.config.enabled_buttons
    }

    fn set_enabled_buttons(&self, _buttons: super::WindowButtons) {}

    // --- DPI / redraw ---

    fn dpi(&self) -> f64 {
        1.0
    }

    fn request_redraw(&self) {
        self.redraw_requests.fetch_add(1, Ordering::Relaxed);
    }

    // No platform IME to talk to.
    fn set_ime_allowed(&self, _allowed: bool) {}
    fn set_ime_cursor_area(&self, _position: [f32; 2], _size: [f32; 2]) {}

    fn create_wgpu_surface(
        &self,
        _instance: &wgpu::Instance,
    ) -> Result<Option<wgpu::Surface<'static>>, WindowSurfaceError> {
        Ok(None)
    }

    /// Returns the creation-time config with the live title/inner size and the
    /// given surface config (there is no OS state to reconstruct from).
    fn get_config(&self, surface_config: wgpu::SurfaceConfiguration) -> WindowConfig {
        let mut config = self.config.clone();
        config.title = self.title.lock().clone();
        config.inner_size = Some(Size::from(*self.inner_size.lock()));
        config.surface_config = surface_config;
        config
    }
}
