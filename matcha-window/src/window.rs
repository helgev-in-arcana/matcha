use crate::adapter::EventLoop;

pub mod cursor;
pub mod window_config;
pub use cursor::CursorIcon;
pub use window_config::*;

// --- Common Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId {
    id: usize,
}

pub struct Window {
    config: WindowConfig,
    window_surface: WindowSurface,
}

impl Window {
    /// Creates the native window. The wgpu surface is not attached yet.
    /// Call [`create_surface`](Self::create_surface) before rendering.
    pub fn new(config: &WindowConfig, ctrl: &dyn EventLoop) -> Result<Self, WindowError> {
        let window_surface = ctrl.create_window(config)?;
        Ok(Self {
            config: config.clone(),
            window_surface,
        })
    }

    pub fn config(&self) -> &WindowConfig {
        &self.config
    }

    pub fn has_surface(&self) -> bool {
        self.window_surface.has_surface()
    }

    /// Creates and attaches the wgpu surface. Does nothing if already present.
    pub fn create_surface(
        &mut self,
        instance: &wgpu::Instance,
        device: &wgpu::Device,
    ) -> Result<(), WindowError> {
        self.window_surface
            .create_surface(instance, device)
            .map_err(|e| WindowError::BackendError(e.to_string()))
    }

    /// Detaches and drops the wgpu surface, keeping the native window alive.
    pub fn destroy_surface(&mut self) {
        self.window_surface.destroy_surface();
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WindowError {
    #[error("Failed to create window: {0}")]
    BackendError(String),
    #[error("Failed to create window surface: {0}")]
    CreateWindowSurface(#[from] wgpu::CreateSurfaceError),
}

// --- Platform-agnostic Window API ---
//
// All type conversions between window_config types and backend-specific types
// are performed inside WindowSurface (winit_window.rs). These methods are
// pure delegations with no backend imports required.
impl Window {
    pub fn id(&self) -> WindowId {
        self.window_surface.id()
    }

    // --- Title ---

    pub fn title(&self) -> String {
        self.window_surface.title()
    }

    pub fn set_title(&self, title: &str) {
        self.window_surface.set_title(title);
    }

    // --- Size ---

    /// Returns the inner (client area) size in physical pixels.
    pub fn inner_size(&self) -> [u32; 2] {
        self.window_surface.inner_size()
    }

    pub fn request_inner_size(&self, width: u32, height: u32) {
        self.window_surface.request_inner_size(width, height);
    }

    /// Returns the outer (including decorations) size in physical pixels.
    pub fn outer_size(&self) -> [u32; 2] {
        self.window_surface.outer_size()
    }

    pub fn set_min_inner_size(&self, min_size: Option<window_config::Size>) {
        self.window_surface.set_min_inner_size(min_size);
    }

    pub fn set_max_inner_size(&self, max_size: Option<window_config::Size>) {
        self.window_surface.set_max_inner_size(max_size);
    }

    /// Returns the resize increment hint in physical pixels, if set.
    pub fn resize_increments(&self) -> Option<[u32; 2]> {
        self.window_surface.resize_increments()
    }

    pub fn set_resize_increments(&self, increments: Option<window_config::Size>) {
        self.window_surface.set_resize_increments(increments);
    }

    // --- Position ---

    /// Returns the inner position (top-left of client area) in physical pixels,
    /// or `None` if unsupported on this platform.
    pub fn inner_position(&self) -> Option<[i32; 2]> {
        self.window_surface.inner_position()
    }

    /// Returns the outer position (top-left including decorations) in physical
    /// pixels, or `None` if unsupported.
    pub fn outer_position(&self) -> Option<[i32; 2]> {
        self.window_surface.outer_position()
    }

    pub fn set_outer_position(&self, position: window_config::Position) {
        self.window_surface.set_outer_position(position);
    }

    // --- Window state ---

    pub fn is_maximized(&self) -> bool {
        self.window_surface.maximized()
    }

    pub fn set_maximized(&self, maximized: bool) {
        self.window_surface.set_maximized(maximized);
    }

    pub fn fullscreen(&self) -> Option<window_config::Fullscreen> {
        self.window_surface.fullscreen()
    }

    pub fn set_fullscreen(&self, fullscreen: Option<window_config::Fullscreen>) {
        self.window_surface.set_fullscreen(fullscreen);
    }

    pub fn is_resizable(&self) -> bool {
        self.window_surface.is_resizable()
    }

    pub fn set_resizable(&self, resizable: bool) {
        self.window_surface.set_resizable(resizable);
    }

    pub fn is_decorated(&self) -> bool {
        self.window_surface.is_decorated()
    }

    pub fn set_decorations(&self, decorations: bool) {
        self.window_surface.set_decorations(decorations);
    }

    pub fn is_visible(&self) -> Option<bool> {
        self.window_surface.is_visible()
    }

    pub fn set_visible(&self, visible: bool) {
        self.window_surface.set_visible(visible);
    }

    // --- Appearance ---

    pub fn theme(&self) -> Option<window_config::Theme> {
        self.window_surface.theme()
    }

    pub fn set_theme(&self, theme: Option<window_config::Theme>) {
        self.window_surface.set_theme(theme);
    }

    pub fn enabled_buttons(&self) -> window_config::WindowButtons {
        self.window_surface.enabled_buttons()
    }

    pub fn set_enabled_buttons(&self, buttons: window_config::WindowButtons) {
        self.window_surface.set_enabled_buttons(buttons);
    }

    // --- DPI / surface format ---

    pub fn dpi(&self) -> f64 {
        self.window_surface.dpi()
    }

    /// Returns the configured surface texture format.
    /// The format is retained even when no surface is attached.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.window_surface.format()
    }

    pub fn change_format(&self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        self.window_surface.change_format(device, format);
    }

    // --- Surface access ---

    pub fn surface(&self) -> &WindowSurface {
        &self.window_surface
    }

    pub fn surface_mut(&mut self) -> &mut WindowSurface {
        &mut self.window_surface
    }

    /// Allow or forbid the platform IME for this window.
    ///
    /// IME is **off by default**, so no [`ImeEvent`](crate::event::device_event::ImeEvent)
    /// is produced until this is enabled — without it, non-Latin input methods
    /// simply cannot type into the window at all. Enable it while a widget that
    /// accepts text has focus, and disable it otherwise, so ordinary keyboard
    /// shortcuts are not swallowed by the IME.
    /// Set the pointer's appearance while it is over this window.
    ///
    /// Pushed by whoever tracks what the pointer is over; this crate has no
    /// opinion about when it changes.
    pub fn set_cursor_icon(&self, icon: CursorIcon) {
        self.window_surface.set_cursor_icon(icon);
    }

    pub fn set_ime_allowed(&self, allowed: bool) {
        self.window_surface.set_ime_allowed(allowed);
    }

    /// Tell the platform where the caret is, in physical window coordinates, so
    /// it can place the candidate list next to the text instead of in a corner.
    pub fn set_ime_cursor_area(&self, position: [f32; 2], size: [f32; 2]) {
        self.window_surface.set_ime_cursor_area(position, size);
    }

    pub fn request_redraw(&self) {
        self.window_surface.request_redraw();
    }
}

// Backend-agnostic surface + the NativeWindow trait each backend implements.
mod surface;
pub(crate) use surface::*;

#[cfg(feature = "winit")]
mod winit_window;

#[cfg(feature = "baseview")]
mod baseview_window;

#[cfg(feature = "headless")]
mod headless_window;
