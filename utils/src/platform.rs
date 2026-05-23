//! Platform-conditional marker traits.
//!
//! On native targets the framework runs on a multi-threaded runtime, so types
//! crossing thread boundaries must be `Send`/`Sync`. WASM is single-threaded,
//! so these markers relax to no-op bounds there.

#[cfg(not(web))]
#[path = "platform/native.rs"]
mod imp;
#[cfg(web)]
#[path = "platform/web.rs"]
mod imp;

pub use imp::{MaybeSend, MaybeSendSync};
