//! Unfiltered device input, ahead of any per-window routing.
//!
//! Placeholder. Nothing constructs these yet — the winit interface receives a
//! `DeviceId` and drops it (`winit_interface/common.rs`), and every
//! [`Application`](crate::application::Application) impl in the workspace takes
//! the default no-op. The surface exists to record the intended capability:
//! raw deltas that have not been accelerated, clamped to the window, or
//! attributed to a widget — what a 3D viewport or a drag past the screen edge
//! wants.

/// Raw, unfiltered input from one device.
#[derive(Debug, Clone)]
pub struct RawDeviceEvent {}

/// Identifies the device an event came from.
///
/// Opaque and backend-neutral: both winit and baseview hand out their own
/// opaque handles, so the only thing portable about a device id is that two of
/// them can be compared. Do not attach meaning to the number — a backend is
/// free to mint it however it likes.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RawDeviceId(u64);

impl RawDeviceId {
    /// Mint an id from a backend's own device handle.
    // Unused until a backend actually maps raw device events; see the module
    // docs. Kept so that wiring one up is a call, not a redesign.
    #[allow(dead_code)]
    pub(crate) fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}
