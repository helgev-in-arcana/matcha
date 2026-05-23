//! Configuration for [`WindowSurface`](crate::window::WindowSurface).
//!
//! The `Size`, `Position`, `WindowButtons`, `Theme` and `Fullscreen` value
//! types live in `common.rs` and are available on every target. The
//! `WindowConfig` struct itself diverges: native exposes the full set of
//! OS window-manager fields; web only exposes the fields the browser
//! actually supports, plus an optional `canvas_id`.

mod common;
pub use common::{Fullscreen, Position, Size, Theme, WindowButtons};

#[cfg(not(web))]
#[path = "window_config/native.rs"]
mod imp;
#[cfg(web)]
#[path = "window_config/web.rs"]
mod imp;

pub use imp::WindowConfig;
