#[cfg(all(feature = "winit", feature = "baseview"))]
compile_error!("feature \"winit\" and feature \"baseview\" cannot be enabled at the same time");

pub mod adapter;
pub mod clipboard;
pub mod application;
pub mod event;
pub mod window;

#[cfg(feature = "winit")]
pub(crate) mod winit_interface;

#[cfg(feature = "baseview")]
pub(crate) mod baseview_interface;

/// Headless backend for driving the full app pipeline in tests: no OS window,
/// no wgpu surface, no event loop (the test itself calls the
/// [`adapter::Adapter`] methods). Public, unlike the other interfaces, because
/// tests in downstream crates construct these types directly.
#[cfg(feature = "headless")]
pub mod headless_interface;
