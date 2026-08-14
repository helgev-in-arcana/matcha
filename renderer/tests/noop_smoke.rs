//! Smoke test: `CoreRenderer::new` on a wgpu noop device.
//!
//! The noop adapter reports every feature (including `IMMEDIATES`), so pipeline
//! creation must succeed without any real GPU. This is the renderer-side gate
//! for the headless test stack (`matcha-ecs/tests/headless_app.rs`).

use gpu_utils::gpu::{Gpu, GpuDescriptor};
use renderer::CoreRenderer;

#[test]
fn core_renderer_builds_on_noop_device() {
    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor::noop()))
        .expect("noop backend adapter/device request should succeed without any GPU");
    let (device, _queue) = gpu
        .context()
        .expect("device/queue are available immediately after Gpu::new");
    let _renderer = CoreRenderer::new(&device);
}
