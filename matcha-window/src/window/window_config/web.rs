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
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                view_formats: Vec::new(),
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
