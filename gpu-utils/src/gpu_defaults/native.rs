pub(crate) const BACKENDS: wgpu::Backends = wgpu::Backends::PRIMARY;

// `VERTEX_WRITABLE_STORAGE` used to be required because the render pass bound
// `visible_instances` writably even though the vertex shader only reads it; the
// render pass now has its own read-only bind group layout, so it is gone.
// `IMMEDIATES` is still native-only — the web build carries the same parameter
// block in a uniform buffer instead.
pub(crate) const REQUIRED_FEATURES: wgpu::Features = wgpu::Features::IMMEDIATES;
