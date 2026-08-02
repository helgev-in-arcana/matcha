//! Winit backend: [`WinitWindow`] implements the [`NativeWindow`] contract
//! for a real OS window, and provides the winit-specific [`WindowSurface`]
//! constructor. All winit type conversions happen here, so callers never need
//! to import winit directly.

use std::sync::Arc;

use super::surface::{NativeWindow, WindowSurface, WindowSurfaceError};

// ---------------------------------------------------------------------------
// WindowId ↔ winit::window::WindowId conversion
// ---------------------------------------------------------------------------

impl From<winit::window::WindowId> for super::WindowId {
    fn from(id: winit::window::WindowId) -> Self {
        let u64_id: u64 = id.into();
        super::WindowId {
            id: u64_id as usize,
        }
    }
}

/// A real OS window managed by winit.
pub(crate) struct WinitWindow(Arc<winit::window::Window>);

/// Winit constructor
impl WindowSurface {
    /// Creates the native window only. The wgpu surface is not attached yet.
    /// Call [`create_surface`](Self::create_surface) before rendering.
    pub(crate) fn new(
        event_loop: &winit::event_loop::ActiveEventLoop,
        config: &super::window_config::WindowConfig,
    ) -> Result<Self, WindowSurfaceError> {
        let window = event_loop
            .create_window(config.to_winit_attributes())
            .map_err(|e| WindowSurfaceError::CreateWindow(e.to_string()))?;

        Ok(Self::from_native(
            Box::new(WinitWindow(Arc::new(window))),
            config,
        ))
    }
}

impl NativeWindow for WinitWindow {
    fn id(&self) -> super::WindowId {
        self.0.id().into()
    }

    fn title(&self) -> String {
        self.0.title()
    }

    fn set_title(&self, title: &str) {
        self.0.set_title(title);
    }

    // --- Size ---

    fn inner_size(&self) -> [u32; 2] {
        let s = self.0.inner_size();
        [s.width, s.height]
    }

    fn request_inner_size(&self, width: u32, height: u32) {
        let _ = self
            .0
            .request_inner_size(winit::dpi::PhysicalSize::new(width, height));
    }

    fn outer_size(&self) -> [u32; 2] {
        let s = self.0.outer_size();
        [s.width, s.height]
    }

    fn resize_increments(&self) -> Option<[u32; 2]> {
        self.0.resize_increments().map(|s| [s.width, s.height])
    }

    fn set_resize_increments(&self, increments: Option<super::Size>) {
        self.0
            .set_resize_increments(increments.map(Into::<winit::dpi::Size>::into));
    }

    fn set_min_inner_size(&self, min_size: Option<super::Size>) {
        self.0
            .set_min_inner_size(min_size.map(Into::<winit::dpi::Size>::into));
    }

    fn set_max_inner_size(&self, max_size: Option<super::Size>) {
        self.0
            .set_max_inner_size(max_size.map(Into::<winit::dpi::Size>::into));
    }

    // --- Position ---

    fn inner_position(&self) -> Option<[i32; 2]> {
        self.0.inner_position().ok().map(|p| [p.x, p.y])
    }

    fn outer_position(&self) -> Option<[i32; 2]> {
        self.0.outer_position().ok().map(|p| [p.x, p.y])
    }

    fn set_outer_position(&self, position: super::Position) {
        match position {
            super::Position::Physical { x, y } => {
                self.0
                    .set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
            }
            super::Position::Logical { x, y } => {
                self.0
                    .set_outer_position(winit::dpi::LogicalPosition::new(x, y));
            }
        }
    }

    // --- Window state ---

    fn maximized(&self) -> bool {
        self.0.is_maximized()
    }

    fn set_maximized(&self, maximized: bool) {
        self.0.set_maximized(maximized);
    }

    fn fullscreen(&self) -> Option<super::Fullscreen> {
        self.0.fullscreen().map(Into::into)
    }

    fn set_fullscreen(&self, fullscreen: Option<super::Fullscreen>) {
        self.0.set_fullscreen(fullscreen.map(Into::into));
    }

    fn is_resizable(&self) -> bool {
        self.0.is_resizable()
    }

    fn set_resizable(&self, resizable: bool) {
        self.0.set_resizable(resizable);
    }

    fn is_decorated(&self) -> bool {
        self.0.is_decorated()
    }

    fn set_decorations(&self, decorations: bool) {
        self.0.set_decorations(decorations);
    }

    fn is_visible(&self) -> Option<bool> {
        self.0.is_visible()
    }

    fn set_visible(&self, visible: bool) {
        self.0.set_visible(visible);
    }

    // --- Appearance ---

    fn theme(&self) -> Option<super::Theme> {
        self.0.theme().map(Into::into)
    }

    fn set_theme(&self, theme: Option<super::Theme>) {
        self.0.set_theme(theme.map(Into::into));
    }

    fn enabled_buttons(&self) -> super::WindowButtons {
        self.0.enabled_buttons().into()
    }

    fn set_enabled_buttons(&self, buttons: super::WindowButtons) {
        self.0.set_enabled_buttons(buttons.into());
    }

    // --- DPI / redraw ---

    fn dpi(&self) -> f64 {
        self.0.scale_factor()
    }

    fn request_redraw(&self) {
        self.0.request_redraw();
    }

    fn set_cursor_icon(&self, icon: super::CursorIcon) {
        use super::CursorIcon as C;
        use winit::window::CursorIcon as W;
        match icon {
            C::Hidden => self.0.set_cursor_visible(false),
            other => {
                self.0.set_cursor_visible(true);
                self.0.set_cursor(match other {
                    C::Default | C::Hidden => W::Default,
                    C::Pointer => W::Pointer,
                    C::Text => W::Text,
                    C::Progress => W::Progress,
                    C::Wait => W::Wait,
                    C::Crosshair => W::Crosshair,
                    C::Move => W::Move,
                    C::Grab => W::Grab,
                    C::Grabbing => W::Grabbing,
                    C::NotAllowed => W::NotAllowed,
                    C::ResizeHorizontal => W::EwResize,
                    C::ResizeVertical => W::NsResize,
                    C::ResizeNeSw => W::NeswResize,
                    C::ResizeNwSe => W::NwseResize,
                });
            }
        }
    }

    fn set_ime_allowed(&self, allowed: bool) {
        self.0.set_ime_allowed(allowed);
    }

    fn set_ime_cursor_area(&self, position: [f32; 2], size: [f32; 2]) {
        self.0.set_ime_cursor_area(
            winit::dpi::PhysicalPosition::new(position[0], position[1]),
            winit::dpi::PhysicalSize::new(size[0], size[1]),
        );
    }

    fn create_wgpu_surface(
        &self,
        instance: &wgpu::Instance,
    ) -> Result<Option<wgpu::Surface<'static>>, WindowSurfaceError> {
        instance
            .create_surface(Arc::clone(&self.0))
            .map(Some)
            .map_err(WindowSurfaceError::CreateWindowSurface)
    }

    fn get_config(&self, surface_config: wgpu::SurfaceConfiguration) -> super::WindowConfig {
        super::WindowConfig::from_winit_window(&self.0, surface_config)
    }
}
