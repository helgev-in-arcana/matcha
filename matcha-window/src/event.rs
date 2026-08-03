pub mod device_event;
pub mod window_event;

pub mod raw_device_event;

// ----------------------------------------------------------------------------
// EventStateConfig — top-level configuration for per-window event state machines
// ----------------------------------------------------------------------------
//
// Stored in `WindowManager` and applied whenever a new `Window` is created.
// Users set this once on `Application`; `UiArch` and `Window` never need to
// touch the individual sub-configs directly.

use device_event::MouseStateConfig;

/// Configuration for all per-window event state machines.
///
/// Build with [`EventStateConfig::default()`] and customise the fields you need,
/// then pass to [`Application::with_event_config`] (or equivalent).
#[derive(Debug, Clone, Copy)]
pub struct EventStateConfig {
    /// Mouse gesture settings (combo timing, long-press, primary button, scroll speed).
    ///
    /// `None` means "use [`MouseStateConfig::default()`]".
    pub mouse: MouseStateConfig,
}

impl Default for EventStateConfig {
    fn default() -> Self {
        Self {
            mouse: MouseStateConfig::default(),
        }
    }
}
