//! Render-pipeline cache keyed by surface/target texture format.
//!
//! Native targets use `moka` for bounded, concurrent caching. wasm is
//! single-threaded and wgpu handles there are `!Send`, so a plain locked
//! map is used instead.

#[cfg(not(web))]
#[path = "pipeline_cache/native.rs"]
mod imp;
#[cfg(web)]
#[path = "pipeline_cache/web.rs"]
mod imp;

pub use imp::PipelineCache;
