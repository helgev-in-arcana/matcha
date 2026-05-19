#[cfg(all(feature = "winit", feature = "baseview"))]
compile_error!("feature \"winit\" and feature \"baseview\" cannot be enabled at the same time");

pub mod adapter;
pub mod application;
pub mod event;
pub mod runtime_handle;
pub mod task_handle;
pub mod window;

pub use runtime_handle::RuntimeHandle;

#[cfg(feature = "winit")]
pub(crate) mod winit_interface;

#[cfg(feature = "baseview")]
pub(crate) mod baseview_interface;
