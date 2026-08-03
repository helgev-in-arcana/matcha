// ----------------------------------------------------------------------------
// WindowEventState
// ----------------------------------------------------------------------------

/// Tracks the current state of a window (size, position, focus, etc.).
#[derive(Debug)]
pub struct WindowEventState {
    inner_size: [f32; 2],
    outer_size: [f32; 2],
    inner_position: [f32; 2],
    outer_position: [f32; 2],
}

impl Default for WindowEventState {
    fn default() -> Self {
        Self {
            inner_size: [1.0, 1.0],
            outer_size: [1.0, 1.0],
            inner_position: [1.0, 1.0],
            outer_position: [1.0, 1.0],
        }
    }
}

impl WindowEventState {
    pub fn new(
        inner_size: [f32; 2],
        outer_size: [f32; 2],
        inner_position: [f32; 2],
        outer_position: [f32; 2],
    ) -> Self {
        Self {
            inner_size,
            outer_size,
            inner_position,
            outer_position,
        }
    }

    /// Process an input WindowEvent from the windowing backend mapping it to a stateful event.
    pub fn process(&mut self, event: WindowEvent) -> WindowEvent {
        match event {
            WindowEvent::Resized {
                inner_size,
                outer_size,
            } => {
                self.inner_size = inner_size;
                self.outer_size = outer_size;
                WindowEvent::Resized {
                    inner_size: self.inner_size,
                    outer_size: self.outer_size,
                }
            }
            WindowEvent::Moved {
                inner_position,
                outer_position,
            } => {
                self.inner_position = inner_position;
                self.outer_position = outer_position;
                WindowEvent::Moved {
                    inner_position: self.inner_position,
                    outer_position: self.outer_position,
                }
            }
            _ => event,
        }
    }
}

// ----------------------------------------------------------------------------
// WindowEvent
// ----------------------------------------------------------------------------

/// A high-level window lifecycle / state event.
///
/// These events are distinct from device input events (mouse, keyboard, etc.)
/// and describe changes to the window itself.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowEvent {
    CloseRequested,
    /// Stateless input from windowing backend.
    Resized {
        inner_size: [f32; 2],
        outer_size: [f32; 2],
    },
    /// Stateless input from windowing backend.
    Moved {
        inner_position: [f32; 2],
        outer_position: [f32; 2],
    },
    Focus(bool),
    Theme(crate::window::window_config::Theme),
    ScaleFactorChanged {
        scale_factor: f64,
    },
    Occluded(bool),
}

// Convenience accessors
impl WindowEvent {
    pub fn on_close_requested<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce() -> R,
    {
        match self {
            WindowEvent::CloseRequested => Some(f()),
            _ => None,
        }
    }

    pub fn on_focus<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(bool) -> R,
    {
        match self {
            WindowEvent::Focus(focused) => Some(f(*focused)),
            _ => None,
        }
    }

    pub fn on_resized<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce([f32; 2], [f32; 2]) -> R,
    {
        match self {
            WindowEvent::Resized {
                inner_size,
                outer_size,
            } => Some(f(
                *inner_size,
                *outer_size,
            )),
            _ => None,
        }
    }

    pub fn on_moved<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce([f32; 2], [f32; 2]) -> R,
    {
        match self {
            WindowEvent::Moved {
                inner_position,
                outer_position,
            } => Some(f(
                *inner_position,
                *outer_position,
            )),
            _ => None,
        }
    }

    pub fn on_theme<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(crate::window::window_config::Theme) -> R,
    {
        match self {
            WindowEvent::Theme(theme) => Some(f(*theme)),
            _ => None,
        }
    }

    pub fn on_scale_factor_changed<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(f64) -> R,
    {
        match self {
            WindowEvent::ScaleFactorChanged { scale_factor } => Some(f(*scale_factor)),
            _ => None,
        }
    }

    pub fn on_occluded<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(bool) -> R,
    {
        match self {
            WindowEvent::Occluded(occluded) => Some(f(*occluded)),
            _ => None,
        }
    }
}
