use bitflags::bitflags;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Size {
    Physical { width: u32, height: u32 },
    Logical { width: f64, height: f64 },
}

impl From<[u32; 2]> for Size {
    fn from(size: [u32; 2]) -> Self {
        Self::Physical {
            width: size[0],
            height: size[1],
        }
    }
}

impl From<(u32, u32)> for Size {
    fn from(size: (u32, u32)) -> Self {
        Self::Physical {
            width: size.0,
            height: size.1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Position {
    Physical { x: i32, y: i32 },
    Logical { x: f64, y: f64 },
}

impl From<[i32; 2]> for Position {
    fn from(pos: [i32; 2]) -> Self {
        Self::Physical {
            x: pos[0],
            y: pos[1],
        }
    }
}

impl From<(i32, i32)> for Position {
    fn from(pos: (i32, i32)) -> Self {
        Self::Physical { x: pos.0, y: pos.1 }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct WindowButtons: u32 {
        const CLOSE = 1 << 0;
        const MINIMIZE = 1 << 1;
        const MAXIMIZE = 1 << 2;
        const ALL = Self::CLOSE.bits() | Self::MINIMIZE.bits() | Self::MAXIMIZE.bits();
    }
}

impl Default for WindowButtons {
    fn default() -> Self {
        Self::ALL
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Theme {
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fullscreen {
    Borderless,
    Exclusive,
}

#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    pub inner_size: Option<Size>,
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
    pub surface_config: wgpu::SurfaceConfiguration,
    /// The id of an existing HTML `<canvas>` to render into. When `None`,
    /// winit creates a canvas and appends it to the document body.
    #[cfg(target_arch = "wasm32")]
    pub canvas_id: Option<String>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Matcha Window".to_string(),
            inner_size: None,
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
            #[cfg(target_arch = "wasm32")]
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

    /// Sets the id of an existing HTML `<canvas>` to render into.
    #[cfg(target_arch = "wasm32")]
    pub fn with_canvas_id(mut self, id: impl Into<String>) -> Self {
        self.canvas_id = Some(id.into());
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

        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            use winit::platform::web::WindowAttributesExtWebSys;
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
        }

        attr
    }
}

#[cfg(feature = "winit")]
impl From<Size> for winit::dpi::Size {
    fn from(size: Size) -> Self {
        match size {
            Size::Physical { width, height } => {
                Self::Physical(winit::dpi::PhysicalSize::new(width, height))
            }
            Size::Logical { width, height } => {
                Self::Logical(winit::dpi::LogicalSize::new(width, height))
            }
        }
    }
}

#[cfg(feature = "winit")]
impl From<Position> for winit::dpi::Position {
    fn from(pos: Position) -> Self {
        match pos {
            Position::Physical { x, y } => Self::Physical(winit::dpi::PhysicalPosition::new(x, y)),
            Position::Logical { x, y } => Self::Logical(winit::dpi::LogicalPosition::new(x, y)),
        }
    }
}

#[cfg(feature = "winit")]
impl From<WindowButtons> for winit::window::WindowButtons {
    fn from(buttons: WindowButtons) -> Self {
        let mut winit_buttons = winit::window::WindowButtons::empty();
        if buttons.contains(WindowButtons::CLOSE) {
            winit_buttons |= winit::window::WindowButtons::CLOSE;
        }
        if buttons.contains(WindowButtons::MINIMIZE) {
            winit_buttons |= winit::window::WindowButtons::MINIMIZE;
        }
        if buttons.contains(WindowButtons::MAXIMIZE) {
            winit_buttons |= winit::window::WindowButtons::MAXIMIZE;
        }
        winit_buttons
    }
}

#[cfg(feature = "winit")]
impl From<Theme> for winit::window::Theme {
    fn from(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self::Light,
            Theme::Dark => Self::Dark,
        }
    }
}

#[cfg(feature = "winit")]
impl From<Fullscreen> for winit::window::Fullscreen {
    fn from(fullscreen: Fullscreen) -> Self {
        match fullscreen {
            Fullscreen::Borderless => {
                // Simplified: use primary monitor for borderless by default or current monitor
                winit::window::Fullscreen::Borderless(None)
            }
            Fullscreen::Exclusive => {
                // Simplified: winit requires a VideoMode for exclusive, which we don't have here.
                // For now, fall back to borderless or handle better if needed.
                winit::window::Fullscreen::Borderless(None)
            }
        }
    }
}

#[cfg(feature = "winit")]
impl From<winit::window::Fullscreen> for Fullscreen {
    fn from(f: winit::window::Fullscreen) -> Self {
        match f {
            winit::window::Fullscreen::Borderless(_) => Fullscreen::Borderless,
            winit::window::Fullscreen::Exclusive(_) => Fullscreen::Exclusive,
        }
    }
}

#[cfg(feature = "winit")]
impl From<winit::window::Theme> for Theme {
    fn from(t: winit::window::Theme) -> Self {
        match t {
            winit::window::Theme::Light => Theme::Light,
            winit::window::Theme::Dark => Theme::Dark,
        }
    }
}

#[cfg(feature = "winit")]
impl From<winit::window::WindowButtons> for WindowButtons {
    fn from(b: winit::window::WindowButtons) -> Self {
        let mut result = WindowButtons::empty();
        if b.contains(winit::window::WindowButtons::CLOSE) {
            result |= WindowButtons::CLOSE;
        }
        if b.contains(winit::window::WindowButtons::MINIMIZE) {
            result |= WindowButtons::MINIMIZE;
        }
        if b.contains(winit::window::WindowButtons::MAXIMIZE) {
            result |= WindowButtons::MAXIMIZE;
        }
        result
    }
}
