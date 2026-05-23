use super::common::{Fullscreen, Position, Size, Theme, WindowButtons};

#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    pub inner_size: Option<Size>,
    pub surface_config: wgpu::SurfaceConfiguration,

    pub min_inner_size: Option<Size>,
    pub max_inner_size: Option<Size>,
    pub position: Option<Position>,
    pub resizable: bool,
    pub enabled_buttons: WindowButtons,
    pub maximized: bool,
    pub fullscreen: Option<Fullscreen>,
    pub visible: bool,
    pub transparent: bool,
    pub decorations: bool,
    pub preferred_theme: Option<Theme>,
    pub resize_increments: Option<Size>,
    pub active: bool,
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

            min_inner_size: None,
            max_inner_size: None,
            position: None,
            resizable: true,
            enabled_buttons: WindowButtons::default(),
            maximized: false,
            fullscreen: None,
            visible: true,
            transparent: false,
            decorations: true,
            preferred_theme: None,
            resize_increments: None,
            active: true,
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

    pub fn with_min_inner_size(mut self, size: impl Into<Size>) -> Self {
        self.min_inner_size = Some(size.into());
        self
    }

    pub fn with_max_inner_size(mut self, size: impl Into<Size>) -> Self {
        self.max_inner_size = Some(size.into());
        self
    }

    pub fn with_position(mut self, position: impl Into<Position>) -> Self {
        self.position = Some(position.into());
        self
    }

    pub fn with_resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn with_enabled_buttons(mut self, buttons: WindowButtons) -> Self {
        self.enabled_buttons = buttons;
        self
    }

    pub fn with_maximized(mut self, maximized: bool) -> Self {
        self.maximized = maximized;
        self
    }

    pub fn with_fullscreen(mut self, fullscreen: Option<Fullscreen>) -> Self {
        self.fullscreen = fullscreen;
        self
    }

    pub fn with_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn with_transparent(mut self, transparent: bool) -> Self {
        self.transparent = transparent;
        self
    }

    pub fn with_decorations(mut self, decorations: bool) -> Self {
        self.decorations = decorations;
        self
    }

    pub fn with_preferred_theme(mut self, theme: Option<Theme>) -> Self {
        self.preferred_theme = theme;
        self
    }

    pub fn with_resize_increments(mut self, increments: impl Into<Size>) -> Self {
        self.resize_increments = Some(increments.into());
        self
    }

    pub fn with_active(mut self, active: bool) -> Self {
        self.active = active;
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

    #[cfg(feature = "winit")]
    pub(crate) fn to_winit_attributes(&self) -> winit::window::WindowAttributes {
        let mut attr = winit::window::WindowAttributes::default();
        attr.title = self.title.clone();
        attr.inner_size = self.inner_size.map(Into::into);
        attr.min_inner_size = self.min_inner_size.map(Into::into);
        attr.max_inner_size = self.max_inner_size.map(Into::into);
        attr.position = self.position.map(Into::into);
        attr.resizable = self.resizable;
        attr.enabled_buttons = self.enabled_buttons.into();
        attr.maximized = self.maximized;
        attr.fullscreen = self.fullscreen.map(Into::into);
        attr.visible = self.visible;
        attr.transparent = self.transparent;
        attr.decorations = self.decorations;
        attr.preferred_theme = self.preferred_theme.map(Into::into);
        attr.resize_increments = self.resize_increments.map(Into::into);
        attr.active = self.active;
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
            min_inner_size: None,
            max_inner_size: None,
            position: window
                .outer_position()
                .ok()
                .map(|p| Position::Physical { x: p.x, y: p.y }),
            resizable: window.is_resizable(),
            enabled_buttons: WindowButtons::ALL,
            maximized: window.is_maximized(),
            fullscreen: None,
            visible: window.is_visible().unwrap_or(true),
            transparent: false,
            decorations: window.is_decorated(),
            preferred_theme: None,
            resize_increments: None,
            active: window.has_focus(),
        }
    }
}
