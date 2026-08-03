//! Bridge between winit's event loop and the framework's [`Adapter`].
//!
//! [`Adapter`]: crate::adapter::Adapter

mod common;
mod keyboard;

#[cfg(not(web))]
#[path = "winit_interface/native.rs"]
mod imp;
#[cfg(web)]
#[path = "winit_interface/web.rs"]
mod imp;

pub(crate) use imp::run;
