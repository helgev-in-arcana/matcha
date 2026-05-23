//! Cross-platform async runtime abstraction for matcha-tree.
//!
//! Native targets drive background tasks on a multi-threaded Tokio runtime.
//! WASM targets drive them on the browser microtask queue via
//! `wasm-bindgen-futures`. The rest of the framework only ever sees
//! [`Runtime`], [`RuntimeHandle`] and [`JoinHandle`], never the backend.

#[cfg(not(web))]
#[path = "runtime/native.rs"]
mod imp;
#[cfg(web)]
#[path = "runtime/web.rs"]
mod imp;

pub use imp::{JoinHandle, Runtime, RuntimeHandle};
