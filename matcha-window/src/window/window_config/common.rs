use bitflags::bitflags;

// ---------------------------------------------------------------------------
// Size
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Position
// ---------------------------------------------------------------------------

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

#[cfg(feature = "winit")]
impl From<Position> for winit::dpi::Position {
    fn from(pos: Position) -> Self {
        match pos {
            Position::Physical { x, y } => Self::Physical(winit::dpi::PhysicalPosition::new(x, y)),
            Position::Logical { x, y } => Self::Logical(winit::dpi::LogicalPosition::new(x, y)),
        }
    }
}

// ---------------------------------------------------------------------------
// WindowButtons
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Theme {
    Light,
    Dark,
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
impl From<winit::window::Theme> for Theme {
    fn from(t: winit::window::Theme) -> Self {
        match t {
            winit::window::Theme::Light => Theme::Light,
            winit::window::Theme::Dark => Theme::Dark,
        }
    }
}

// ---------------------------------------------------------------------------
// Fullscreen
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fullscreen {
    Borderless,
    Exclusive,
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
