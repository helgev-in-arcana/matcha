//! Platform-default values for [`crate::gpu::GpuDescriptor`].

#[cfg(not(web))]
#[path = "gpu_defaults/native.rs"]
mod imp;
#[cfg(web)]
#[path = "gpu_defaults/web.rs"]
mod imp;

pub(crate) use imp::{BACKENDS, REQUIRED_FEATURES};
