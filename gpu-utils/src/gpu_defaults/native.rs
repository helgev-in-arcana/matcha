pub(crate) const BACKENDS: wgpu::Backends = wgpu::Backends::PRIMARY;

pub(crate) const REQUIRED_FEATURES: wgpu::Features = wgpu::Features::PUSH_CONSTANTS
    .union(wgpu::Features::VERTEX_WRITABLE_STORAGE);
