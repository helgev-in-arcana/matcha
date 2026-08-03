pub(crate) const BACKENDS: wgpu::Backends = wgpu::Backends::BROWSER_WEBGPU;

// WebGPU exposes neither `PUSH_CONSTANTS` nor `VERTEX_WRITABLE_STORAGE`;
// wasm renderers need other fallbacks.
pub(crate) const REQUIRED_FEATURES: wgpu::Features = wgpu::Features::empty();
