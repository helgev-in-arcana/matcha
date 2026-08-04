use super::common::Size;

#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    pub inner_size: Option<Size>,
    pub surface_config: wgpu::SurfaceConfiguration,

    /// The id of an existing HTML `<canvas>` to render into. When `None`,
    /// winit creates a canvas and appends it to the document body.
    pub canvas_id: Option<String>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Matcha Window".to_string(),
            inner_size: None,
            surface_config: wgpu::SurfaceConfiguration {
                width: 100,
                height: 100,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                // WebGPU accepts only `rgba8unorm`, `bgra8unorm` and
                // `rgba16float` as canvas formats, and wgpu's WebGPU backend
                // does no fallback — it passes the format straight to
                // `configure()` and unwraps, so an sRGB format here throws.
                //
                // Drawing still happens through an `Rgba8UnormSrgb` *view*
                // (requested below, honoured by `WindowSurface::format`), which
                // preserves the automatic linear->sRGB encode that the render
                // pipeline — and `linear_to_srgb_u8` on the upload side —
                // depend on. Configuring non-sRGB *and* dropping the view format
                // would silently change every colour in the app.
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_formats: vec![wgpu::TextureFormat::Rgba8UnormSrgb],
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 1,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
            },
            canvas_id: None,
        }
    }
}

impl WindowConfig {
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn with_inner_size(mut self, size: impl Into<Size>) -> Self {
        self.inner_size = Some(size.into());
        self
    }

    pub fn with_surface_usage(mut self, usage: wgpu::TextureUsages) -> Self {
        self.surface_config.usage = usage;
        self
    }

    pub fn with_surface_format(mut self, format: wgpu::TextureFormat) -> Self {
        self.surface_config.format = format;
        self
    }

    pub fn with_surface_present_mode(mut self, present_mode: wgpu::PresentMode) -> Self {
        self.surface_config.present_mode = present_mode;
        self
    }

    pub fn with_surface_alpha_mode(mut self, alpha_mode: wgpu::CompositeAlphaMode) -> Self {
        self.surface_config.alpha_mode = alpha_mode;
        self
    }

    /// Sets the id of an existing HTML `<canvas>` to render into.
    pub fn with_canvas_id(mut self, id: impl Into<String>) -> Self {
        self.canvas_id = Some(id.into());
        self
    }

    #[cfg(feature = "winit")]
    pub(crate) fn to_winit_attributes(&self) -> winit::window::WindowAttributes {
        use wasm_bindgen::JsCast;
        use winit::platform::web::WindowAttributesExtWebSys;

        let mut attr = winit::window::WindowAttributes::default();
        attr.title = self.title.clone();
        attr.inner_size = self.inner_size.map(Into::into);
        attr = match &self.canvas_id {
            Some(id) => {
                let canvas = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.get_element_by_id(id))
                    .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok());
                if let Some(canvas) = &canvas {
                    sync_backing_store_to_css_size(canvas);
                }
                attr.with_canvas(canvas)
            }
            None => attr.with_append(true),
        };
        attr
    }

    /// Reconstructs a `WindowConfig` from a live winit window and its current
    /// surface configuration.
    #[cfg(feature = "winit")]
    pub(crate) fn from_winit_window(
        window: &winit::window::Window,
        surface_config: wgpu::SurfaceConfiguration,
    ) -> Self {
        let inner_size = window.inner_size();
        Self {
            title: window.title(),
            inner_size: Some(Size::Physical {
                width: inner_size.width,
                height: inner_size.height,
            }),
            surface_config,
            canvas_id: None,
        }
    }
}

/// Size a canvas's backing store to the size CSS is actually giving it.
///
/// A `<canvas>` has two independent sizes: the CSS box it occupies, and the
/// `width`/`height` attributes that define its drawing buffer. The stylesheet
/// controls the first; the second defaults to **300x150** and is *not* derived
/// from it. winit reads the drawing buffer as the window's inner size, so
/// without this the app lays out its first frames against 300x150 while filling
/// a full-page box — content at the wrong size, and picking resolving clicks
/// against rects that do not match what is on screen.
///
/// winit's own `ResizeObserver` corrects this, but only once the observed size
/// *changes*, so on a static page the mismatch survives until the user resizes
/// the window. Doing it here, before the window is created, means the very
/// first frame is already right.
///
/// The buffer is sized in **physical** pixels (CSS pixels times
/// `devicePixelRatio`), which is what the rest of the stack expects — see
/// `matcha-ecs`'s `UiScale` for the conversion back to UI pixels.
#[cfg(feature = "winit")]
fn sync_backing_store_to_css_size(canvas: &web_sys::HtmlCanvasElement) {
    let dpr = web_sys::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0);
    let css_w = canvas.client_width();
    let css_h = canvas.client_height();

    // A canvas that is not laid out yet (display:none, or not in the document)
    // reports 0. Leaving the default alone is better than a zero-sized surface,
    // which wgpu rejects outright.
    if css_w <= 0 || css_h <= 0 {
        log::warn!(
            "canvas has no CSS size yet ({css_w}x{css_h}); leaving its drawing \
             buffer at the default. Is it styled and in the document?"
        );
        return;
    }

    canvas.set_width((css_w as f64 * dpr).round() as u32);
    canvas.set_height((css_h as f64 * dpr).round() as u32);
}
