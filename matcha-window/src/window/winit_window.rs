use std::sync::Arc;

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

pub struct WindowSurface {
    window: Arc<winit::window::Window>,
    surface: Option<wgpu::Surface<'static>>,
    /// Retained across surface destruction so `create_surface` can reconfigure correctly.
    // TODO: wgpu v28.0.0 can get current config from surface. Fix this in the future.
    current_config: parking_lot::Mutex<wgpu::SurfaceConfiguration>,
}

/// Constructor
impl WindowSurface {
    /// Creates the native window only. The wgpu surface is not attached yet.
    /// Call [`create_surface`](Self::create_surface) before rendering.
    pub fn new(
        event_loop: &winit::event_loop::ActiveEventLoop,
        config: &super::window_config::WindowConfig,
    ) -> Result<Self, WindowSurfaceError> {
        let window = event_loop
            .create_window(config.to_winit_attributes())
            .map_err(WindowSurfaceError::CreateWindow)?;

        let window = Arc::new(window);
        let (width, height) = window.inner_size().into();

        let initial_config = wgpu::SurfaceConfiguration {
            width,
            height,
            usage: config.surface_config.usage,
            format: config.surface_config.format,
            view_formats: Vec::new(),
            present_mode: config.surface_config.present_mode,
            desired_maximum_frame_latency: config.surface_config.desired_maximum_frame_latency,
            alpha_mode: config.surface_config.alpha_mode,
        };

        Ok(Self {
            window,
            surface: None,
            current_config: parking_lot::Mutex::new(initial_config),
        })
    }

    /// Creates and attaches the wgpu surface. Does nothing if already present.
    pub fn create_surface(
        &mut self,
        instance: &wgpu::Instance,
        device: &wgpu::Device,
    ) -> Result<(), WindowSurfaceError> {
        if self.surface.is_some() {
            return Ok(());
        }

        let surface = instance
            .create_surface(Arc::clone(&self.window))
            .map_err(WindowSurfaceError::CreateWindowSurface)?;

        let size = self.window.inner_size();
        {
            let mut config = self.current_config.lock();
            config.width = size.width;
            config.height = size.height;
            surface.configure(device, &config);
        }

        self.surface = Some(surface);
        Ok(())
    }

    /// Detaches and drops the wgpu surface, keeping the native window alive.
    pub fn destroy_surface(&mut self) {
        self.surface = None;
    }

    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }

    pub fn window(&self) -> &winit::window::Window {
        &self.window
    }

    pub fn surface(&self) -> Option<&wgpu::Surface<'_>> {
        self.surface.as_ref()
    }
}

/// Setters and Getters
///
/// All signatures use platform-agnostic types from `window_config`.
/// Conversions to/from winit types are done here, so callers never need
/// to import winit directly.
impl WindowSurface {
    pub fn id(&self) -> super::WindowId {
        let u64_id: u64 = self.window.id().into();
        super::WindowId {
            id: u64_id as usize,
        }
    }

    pub fn title(&self) -> String {
        self.window.title()
    }

    pub fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }

    // --- Size ---

    pub fn inner_size(&self) -> [u32; 2] {
        let s = self.window.inner_size();
        [s.width, s.height]
    }

    pub fn request_inner_size(&self, width: u32, height: u32) {
        let _ = self
            .window
            .request_inner_size(winit::dpi::PhysicalSize::new(width, height));
    }

    pub fn outer_size(&self) -> [u32; 2] {
        let s = self.window.outer_size();
        [s.width, s.height]
    }

    pub fn resize_increments(&self) -> Option<[u32; 2]> {
        self.window.resize_increments().map(|s| [s.width, s.height])
    }

    pub fn set_resize_increments(&self, increments: Option<super::Size>) {
        self.window
            .set_resize_increments(increments.map(Into::<winit::dpi::Size>::into));
    }

    pub fn set_min_inner_size(&self, min_size: Option<super::Size>) {
        self.window
            .set_min_inner_size(min_size.map(Into::<winit::dpi::Size>::into));
    }

    pub fn set_max_inner_size(&self, max_size: Option<super::Size>) {
        self.window
            .set_max_inner_size(max_size.map(Into::<winit::dpi::Size>::into));
    }

    // --- Position ---

    pub fn inner_position(&self) -> Option<[i32; 2]> {
        self.window.inner_position().ok().map(|p| [p.x, p.y])
    }

    pub fn outer_position(&self) -> Option<[i32; 2]> {
        self.window.outer_position().ok().map(|p| [p.x, p.y])
    }

    pub fn set_outer_position(&self, position: super::Position) {
        match position {
            super::Position::Physical { x, y } => {
                self.window
                    .set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
            }
            super::Position::Logical { x, y } => {
                self.window
                    .set_outer_position(winit::dpi::LogicalPosition::new(x, y));
            }
        }
    }

    // --- Window state ---

    pub fn maximized(&self) -> bool {
        self.window.is_maximized()
    }

    pub fn set_maximized(&self, maximized: bool) {
        self.window.set_maximized(maximized);
    }

    pub fn fullscreen(&self) -> Option<super::Fullscreen> {
        self.window.fullscreen().map(Into::into)
    }

    pub fn set_fullscreen(&self, fullscreen: Option<super::Fullscreen>) {
        self.window.set_fullscreen(fullscreen.map(Into::into));
    }

    pub fn is_resizable(&self) -> bool {
        self.window.is_resizable()
    }

    pub fn set_resizable(&self, resizable: bool) {
        self.window.set_resizable(resizable);
    }

    pub fn is_decorated(&self) -> bool {
        self.window.is_decorated()
    }

    pub fn set_decorations(&self, decorations: bool) {
        self.window.set_decorations(decorations);
    }

    pub fn is_visible(&self) -> Option<bool> {
        self.window.is_visible()
    }

    pub fn set_visible(&self, visible: bool) {
        self.window.set_visible(visible);
    }

    // --- Appearance ---

    pub fn theme(&self) -> Option<super::Theme> {
        self.window.theme().map(Into::into)
    }

    pub fn set_theme(&self, theme: Option<super::Theme>) {
        self.window.set_theme(theme.map(Into::into));
    }

    pub fn enabled_buttons(&self) -> super::WindowButtons {
        self.window.enabled_buttons().into()
    }

    pub fn set_enabled_buttons(&self, buttons: super::WindowButtons) {
        self.window.set_enabled_buttons(buttons.into());
    }

    // --- DPI / surface format ---

    pub fn dpi(&self) -> f64 {
        self.window.scale_factor()
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.current_config.lock().format
    }

    /// Updates the surface format. If no surface is attached, only updates the
    /// stored config so the next `create_surface` uses the new format.
    pub fn change_format(&self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        let mut config = self.current_config.lock();
        config.format = format;
        if let Some(surface) = &self.surface {
            surface.configure(device, &config);
        }
    }
}

impl WindowSurface {
    pub fn get_config(&self) -> super::WindowConfig {
        let config = self.current_config.lock();
        use crate::window::window_config::{Position, Size, WindowButtons};

        let inner_size = self.window.inner_size();
        let position = self
            .window
            .outer_position()
            .ok()
            .map(|p| Position::Physical { x: p.x, y: p.y });

        super::WindowConfig {
            title: self.window.title(),
            inner_size: Some(Size::Physical {
                width: inner_size.width,
                height: inner_size.height,
            }),
            min_inner_size: None,
            max_inner_size: None,
            position,
            resizable: self.window.is_resizable(),
            enabled_buttons: WindowButtons::ALL,
            maximized: self.window.is_maximized(),
            fullscreen: None,
            visible: self.window.is_visible().unwrap_or(true),
            transparent: false,
            decorations: self.window.is_decorated(),
            preferred_theme: None,
            resize_increments: None,
            active: self.window.has_focus(),
            surface_config: config.clone(),
            canvas_id: None,
        }
    }

    pub fn surface_config(&self) -> wgpu::SurfaceConfiguration {
        self.current_config.lock().clone()
    }
}

/// Operations
impl WindowSurface {
    /// Updates dimensions in the stored config and reconfigures the surface if present.
    pub fn resize(&self, size: [u32; 2], device: &wgpu::Device) {
        if size[0] != 0 && size[1] != 0 {
            let mut config = self.current_config.lock();
            config.width = size[0];
            config.height = size[1];
            if let Some(surface) = &self.surface {
                surface.configure(device, &config);
            }
        }
    }

    pub fn reconfigure(&self, device: &wgpu::Device) {
        let size = self.window.inner_size();

        if size.width != 0 && size.height != 0 {
            let mut config = self.current_config.lock();
            config.width = size.width;
            config.height = size.height;
            if let Some(surface) = &self.surface {
                surface.configure(device, &config);
            }
        }
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }
}

/// Rendering
impl WindowSurface {
    pub fn rendering_with_surface_texture<R>(
        &self,
        device: &wgpu::Device,
        f: impl FnOnce(&wgpu::TextureView, &wgpu::Texture) -> R,
    ) -> Result<Option<R>, wgpu::SurfaceError> {
        match self.get_surface_texture(device)? {
            Some(surface_texture) => {
                let view = surface_texture
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let result = f(&view, &surface_texture.texture);
                surface_texture.present();
                Ok(Some(result))
            }
            None => Ok(None),
        }
    }

    pub fn try_rendering_with_surface_texture<R, E>(
        &self,
        device: &wgpu::Device,
        f: impl FnOnce(wgpu::TextureView) -> Result<R, E>,
    ) -> Result<Option<Result<R, E>>, wgpu::SurfaceError> {
        match self.get_surface_texture(device)? {
            Some(surface_texture) => {
                let view = surface_texture
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let result = f(view);
                surface_texture.present();
                Ok(Some(result))
            }
            None => Ok(None),
        }
    }
}

impl WindowSurface {
    /// Return Value:
    /// - `Ok(Some(texture))`: Success to acquire surface texture.
    /// - `Ok(None)`: No surface, timeout, or zero-size texture. Frame will be skipped.
    /// - `Err(wgpu::SurfaceError)`: Other unrecoverable error.
    pub fn get_surface_texture(
        &self,
        device: &wgpu::Device,
    ) -> Result<Option<wgpu::SurfaceTexture>, wgpu::SurfaceError> {
        let surface = match &self.surface {
            Some(s) => s,
            None => return Ok(None),
        };

        match surface.get_current_texture() {
            Ok(texture) => Ok(Some(texture)),
            Err(wgpu::SurfaceError::Timeout) => {
                log::warn!("Surface texture acquire timed out. Skipping frame.");
                Ok(None)
            }
            Err(wgpu::SurfaceError::Outdated) | Err(wgpu::SurfaceError::Lost) => {
                log::warn!("Surface is outdated or lost. Reconfiguring and retrying...");
                let size = self.window.inner_size();
                if size.width == 0 || size.height == 0 {
                    Ok(None)
                } else {
                    self.reconfigure(device);
                    match surface.get_current_texture() {
                        Ok(texture) => Ok(Some(texture)),
                        Err(wgpu::SurfaceError::Timeout) => {
                            log::warn!(
                                "Surface texture acquire timed out on retry. Skipping frame."
                            );
                            Ok(None)
                        }
                        Err(e) => Err(e),
                    }
                }
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                log::error!("System is out of memory. Application should probably exit.");
                Err(wgpu::SurfaceError::OutOfMemory)
            }
            Err(wgpu::SurfaceError::Other) => {
                log::error!("Unknown surface error");
                Err(wgpu::SurfaceError::Other)
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WindowSurfaceError {
    #[error("Failed to create window")]
    CreateWindow(winit::error::OsError),
    #[error("Failed to create window surface")]
    CreateWindowSurface(wgpu::CreateSurfaceError),
}
