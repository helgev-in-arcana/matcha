//! Backend-agnostic window surface.
//!
//! [`WindowSurface`] is the single concrete type shared by every backend; the
//! backend-specific part (the native OS window, or an in-memory stand-in for
//! headless testing) is abstracted behind the [`NativeWindow`] trait. Surface
//! management (`create_surface`/`resize`/`get_surface_texture`/...) lives here
//! once, so a backend only implements the window contract.

use super::window_config::{Fullscreen, Position, Size, Theme, WindowButtons, WindowConfig};

/// Contract a windowing backend implements for one native window.
///
/// All signatures use platform-agnostic types from `window_config`; conversions
/// to/from backend types are done inside each implementation, so common code
/// never imports a backend directly.
pub(crate) trait NativeWindow: utils::MaybeSendSync {
    fn id(&self) -> super::WindowId;

    // --- Title ---
    fn title(&self) -> String;
    fn set_title(&self, title: &str);

    // --- Size ---
    fn inner_size(&self) -> [u32; 2];
    fn request_inner_size(&self, width: u32, height: u32);
    fn outer_size(&self) -> [u32; 2];
    fn resize_increments(&self) -> Option<[u32; 2]>;
    fn set_resize_increments(&self, increments: Option<Size>);
    fn set_min_inner_size(&self, min_size: Option<Size>);
    fn set_max_inner_size(&self, max_size: Option<Size>);

    // --- Position ---
    fn inner_position(&self) -> Option<[i32; 2]>;
    fn outer_position(&self) -> Option<[i32; 2]>;
    fn set_outer_position(&self, position: Position);

    // --- Window state ---
    fn maximized(&self) -> bool;
    fn set_maximized(&self, maximized: bool);
    fn fullscreen(&self) -> Option<Fullscreen>;
    fn set_fullscreen(&self, fullscreen: Option<Fullscreen>);
    fn is_resizable(&self) -> bool;
    fn set_resizable(&self, resizable: bool);
    fn is_decorated(&self) -> bool;
    fn set_decorations(&self, decorations: bool);
    fn is_visible(&self) -> Option<bool>;
    fn set_visible(&self, visible: bool);

    // --- Appearance ---
    fn theme(&self) -> Option<Theme>;
    fn set_theme(&self, theme: Option<Theme>);
    fn enabled_buttons(&self) -> WindowButtons;
    fn set_enabled_buttons(&self, buttons: WindowButtons);

    // --- DPI / redraw ---
    fn dpi(&self) -> f64;
    fn request_redraw(&self);

    // --- Cursor ---
    fn set_cursor_icon(&self, icon: super::CursorIcon);

    // --- IME ---
    fn set_ime_allowed(&self, allowed: bool);
    fn set_ime_cursor_area(&self, position: [f32; 2], size: [f32; 2]);

    /// Creates a wgpu surface targeting this window, or `Ok(None)` for
    /// backends that cannot present (headless). Raw-window-handle details stay
    /// inside each backend.
    fn create_wgpu_surface(
        &self,
        instance: &wgpu::Instance,
    ) -> Result<Option<wgpu::Surface<'static>>, WindowSurfaceError>;

    /// Reconstructs a [`WindowConfig`] reflecting the window's live state,
    /// with the given surface configuration attached.
    fn get_config(&self, surface_config: wgpu::SurfaceConfiguration) -> WindowConfig;
}

pub struct WindowSurface {
    window: Box<dyn NativeWindow>,
    surface: Option<wgpu::Surface<'static>>,
    /// Retained across surface destruction so `create_surface` can reconfigure correctly.
    // TODO: wgpu v28.0.0 can get current config from surface. Fix this in the future.
    current_config: parking_lot::Mutex<wgpu::SurfaceConfiguration>,
}

// On the web, a window ends up inside a `bevy_ecs` component (matcha-ecs's
// `components::window::Window`), and bevy requires `Send + Sync` of every
// component — there is no non-`Send` component storage to opt into. But winit's
// web window is built on `Rc` and is genuinely `!Send`, which no feature flag
// can change.
//
// SAFETY: `wasm32-unknown-unknown` without the `atomics` target feature is
// single-threaded. There is no other thread for a value to be sent to or shared
// with, so these bounds cannot be violated. This is the same bargain, under the
// same condition, that wgpu makes with `fragile-send-sync-non-atomic-wasm`.
//
// The guard below turns the assumption into a build failure rather than a
// silent unsoundness the moment a threaded wasm target is used.
#[cfg(all(web, target_feature = "atomics"))]
compile_error!(
    "WindowSurface's Send/Sync impls assume a single-threaded wasm target, but \
     `atomics` is enabled. They are no longer sound; see window/surface.rs."
);
#[cfg(web)]
unsafe impl Send for WindowSurface {}
#[cfg(web)]
unsafe impl Sync for WindowSurface {}

/// Constructor
impl WindowSurface {
    /// Wraps a backend window. The wgpu surface is not attached yet;
    /// call [`create_surface`](Self::create_surface) before rendering.
    pub(crate) fn from_native(window: Box<dyn NativeWindow>, config: &WindowConfig) -> Self {
        let [width, height] = window.inner_size();

        let initial_config = wgpu::SurfaceConfiguration {
            width,
            height,
            usage: config.surface_config.usage,
            format: config.surface_config.format,
            // Propagated, not dropped: the web configures the canvas as
            // `Rgba8Unorm` (WebGPU permits no sRGB canvas format) and asks for
            // an `Rgba8UnormSrgb` view format, which is what keeps the automatic
            // linear->sRGB encode the render pipeline relies on. See `format`.
            view_formats: config.surface_config.view_formats.clone(),
            present_mode: config.surface_config.present_mode,
            desired_maximum_frame_latency: config.surface_config.desired_maximum_frame_latency,
            alpha_mode: config.surface_config.alpha_mode,
        };

        Self {
            window,
            surface: None,
            current_config: parking_lot::Mutex::new(initial_config),
        }
    }

    /// Creates and attaches the wgpu surface. Does nothing if already present,
    /// or if the backend cannot present (headless windows stay surfaceless, so
    /// [`get_surface_texture`](Self::get_surface_texture) keeps returning
    /// `Ok(None)` and frames are skipped).
    pub fn create_surface(
        &mut self,
        instance: &wgpu::Instance,
        device: &wgpu::Device,
    ) -> Result<(), WindowSurfaceError> {
        if self.surface.is_some() {
            return Ok(());
        }

        let Some(surface) = self.window.create_wgpu_surface(instance)? else {
            return Ok(());
        };

        let [width, height] = self.window.inner_size();
        {
            let mut config = self.current_config.lock();
            config.width = width;
            config.height = height;
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

    pub fn surface(&self) -> Option<&wgpu::Surface<'_>> {
        self.surface.as_ref()
    }
}

/// Setters and Getters (pure delegations to the backend window)
impl WindowSurface {
    pub fn id(&self) -> super::WindowId {
        self.window.id()
    }

    pub fn title(&self) -> String {
        self.window.title()
    }

    pub fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }

    // --- Size ---

    pub fn inner_size(&self) -> [u32; 2] {
        self.window.inner_size()
    }

    pub fn request_inner_size(&self, width: u32, height: u32) {
        self.window.request_inner_size(width, height);
    }

    pub fn outer_size(&self) -> [u32; 2] {
        self.window.outer_size()
    }

    pub fn resize_increments(&self) -> Option<[u32; 2]> {
        self.window.resize_increments()
    }

    pub fn set_resize_increments(&self, increments: Option<Size>) {
        self.window.set_resize_increments(increments);
    }

    pub fn set_min_inner_size(&self, min_size: Option<Size>) {
        self.window.set_min_inner_size(min_size);
    }

    pub fn set_max_inner_size(&self, max_size: Option<Size>) {
        self.window.set_max_inner_size(max_size);
    }

    // --- Position ---

    pub fn inner_position(&self) -> Option<[i32; 2]> {
        self.window.inner_position()
    }

    pub fn outer_position(&self) -> Option<[i32; 2]> {
        self.window.outer_position()
    }

    pub fn set_outer_position(&self, position: Position) {
        self.window.set_outer_position(position);
    }

    // --- Window state ---

    pub fn maximized(&self) -> bool {
        self.window.maximized()
    }

    pub fn set_maximized(&self, maximized: bool) {
        self.window.set_maximized(maximized);
    }

    pub fn fullscreen(&self) -> Option<Fullscreen> {
        self.window.fullscreen()
    }

    pub fn set_fullscreen(&self, fullscreen: Option<Fullscreen>) {
        self.window.set_fullscreen(fullscreen);
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

    pub fn theme(&self) -> Option<Theme> {
        self.window.theme()
    }

    pub fn set_theme(&self, theme: Option<Theme>) {
        self.window.set_theme(theme);
    }

    pub fn enabled_buttons(&self) -> WindowButtons {
        self.window.enabled_buttons()
    }

    pub fn set_enabled_buttons(&self, buttons: WindowButtons) {
        self.window.set_enabled_buttons(buttons);
    }

    // --- DPI / surface format ---

    pub fn dpi(&self) -> f64 {
        self.window.dpi()
    }

    /// The format render pipelines should target — **not** necessarily the
    /// format the surface is configured with.
    ///
    /// When a view format is requested it wins, because that is what
    /// [`rendering_with_surface_texture`](Self::rendering_with_surface_texture)
    /// creates the render target view as. The two differ only where the platform
    /// forbids the format we actually want to draw in: WebGPU accepts only
    /// `rgba8unorm`, `bgra8unorm` and `rgba16float` for a canvas, so the web
    /// configures a non-sRGB canvas and draws through an sRGB view of it.
    pub fn format(&self) -> wgpu::TextureFormat {
        let config = self.current_config.lock();
        config.view_formats.first().copied().unwrap_or(config.format)
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
    pub fn get_config(&self) -> WindowConfig {
        self.window.get_config(self.current_config.lock().clone())
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
        let [width, height] = self.window.inner_size();

        if width != 0 && height != 0 {
            let mut config = self.current_config.lock();
            config.width = width;
            config.height = height;
            if let Some(surface) = &self.surface {
                surface.configure(device, &config);
            }
        }
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub fn set_cursor_icon(&self, icon: super::CursorIcon) {
        self.window.set_cursor_icon(icon);
    }

    pub fn set_ime_allowed(&self, allowed: bool) {
        self.window.set_ime_allowed(allowed);
    }

    pub fn set_ime_cursor_area(&self, position: [f32; 2], size: [f32; 2]) {
        self.window.set_ime_cursor_area(position, size);
    }
}

/// Rendering
impl WindowSurface {
    pub fn rendering_with_surface_texture<R>(
        &self,
        device: &wgpu::Device,
        f: impl FnOnce(&wgpu::TextureView, &wgpu::Texture) -> R,
    ) -> Result<Option<R>, SurfaceTextureError> {
        match self.get_surface_texture(device)? {
            Some(surface_texture) => {
                // Name the format explicitly: on the web the surface is
                // configured non-sRGB with an sRGB view format, so the
                // texture's own format is not what we want to draw through.
                let view = surface_texture
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor {
                        format: Some(self.format()),
                        ..Default::default()
                    });
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
    ) -> Result<Option<Result<R, E>>, SurfaceTextureError> {
        match self.get_surface_texture(device)? {
            Some(surface_texture) => {
                // Name the format explicitly: on the web the surface is
                // configured non-sRGB with an sRGB view format, so the
                // texture's own format is not what we want to draw through.
                let view = surface_texture
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor {
                        format: Some(self.format()),
                        ..Default::default()
                    });
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
    /// - `Ok(None)`: No surface, timeout, occlusion, or zero-size texture. Frame will be skipped.
    /// - `Err(SurfaceTextureError)`: Other unrecoverable error.
    pub fn get_surface_texture(
        &self,
        device: &wgpu::Device,
    ) -> Result<Option<wgpu::SurfaceTexture>, SurfaceTextureError> {
        let surface = match &self.surface {
            Some(s) => s,
            None => return Ok(None),
        };

        // wgpu 29: `get_current_texture` returns the `CurrentSurfaceTexture` enum
        // (the old `Result<_, SurfaceError>` API is gone). `Suboptimal` still
        // hands us a presentable texture, so treat it as success.
        match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Ok(Some(texture)),
            wgpu::CurrentSurfaceTexture::Timeout => {
                log::warn!("Surface texture acquire timed out. Skipping frame.");
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                log::debug!("Surface is occluded. Skipping frame.");
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                log::warn!("Surface is outdated or lost. Reconfiguring and retrying...");
                let [width, height] = self.window.inner_size();
                if width == 0 || height == 0 {
                    Ok(None)
                } else {
                    self.reconfigure(device);
                    match surface.get_current_texture() {
                        wgpu::CurrentSurfaceTexture::Success(texture)
                        | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Ok(Some(texture)),
                        wgpu::CurrentSurfaceTexture::Timeout
                        | wgpu::CurrentSurfaceTexture::Occluded => {
                            log::warn!(
                                "Surface texture acquire timed out on retry. Skipping frame."
                            );
                            Ok(None)
                        }
                        wgpu::CurrentSurfaceTexture::Outdated
                        | wgpu::CurrentSurfaceTexture::Lost => {
                            Err(SurfaceTextureError::StillOutdatedAfterReconfigure)
                        }
                        wgpu::CurrentSurfaceTexture::Validation => {
                            Err(SurfaceTextureError::Validation)
                        }
                    }
                }
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("Surface texture acquire raised a validation error.");
                Err(SurfaceTextureError::Validation)
            }
        }
    }
}

/// Unrecoverable failures from [`WindowSurface::get_surface_texture`]
/// (recoverable conditions — timeout, occlusion, outdated-then-reconfigured —
/// are reported as `Ok(None)` / retried instead).
#[derive(Debug, thiserror::Error)]
pub enum SurfaceTextureError {
    #[error("surface texture acquire raised a validation error")]
    Validation,
    #[error("surface is still outdated/lost after reconfiguring")]
    StillOutdatedAfterReconfigure,
}

#[derive(Debug, thiserror::Error)]
pub enum WindowSurfaceError {
    /// The OS refused to create the window.
    ///
    /// Carries the backend's message rather than its error type: winit's
    /// `OsError` has no public fields, so formatting it is already the only
    /// thing a caller could do with it, and a backend-specific payload here
    /// would put a windowing library back into this crate's public API.
    #[error("failed to create window: {0}")]
    CreateWindow(String),
    #[error("Failed to create window surface")]
    CreateWindowSurface(wgpu::CreateSurfaceError),
}
