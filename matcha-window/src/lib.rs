#[cfg(all(feature = "winit", feature = "baseview"))]
compile_error!("feature \"winit\" and feature \"baseview\" cannot be enabled at the same time");

pub mod adapter;
pub mod application;
pub mod event;
pub mod window;

#[cfg(feature = "winit")]
pub(crate) mod winit_interface;

#[cfg(feature = "baseview")]
pub(crate) mod baseview_interface;
