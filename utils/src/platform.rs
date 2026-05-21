//! Platform-conditional marker traits.
//!
//! On native targets the framework runs on a multi-threaded runtime, so types
//! crossing thread boundaries must be `Send`/`Sync`. WASM is single-threaded,
//! so these markers relax to no-op bounds there.

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    pub trait MaybeSend: Send {}
    impl<T: Send> MaybeSend for T {}

    pub trait MaybeSendSync: Send + Sync + MaybeSend {}
    impl<T: Send + Sync> MaybeSendSync for T {}
}

#[cfg(target_arch = "wasm32")]
mod imp {
    pub trait MaybeSend {}
    impl<T> MaybeSend for T {}

    pub trait MaybeSendSync: MaybeSend {}
    impl<T> MaybeSendSync for T {}
}

pub use imp::{MaybeSend, MaybeSendSync};
