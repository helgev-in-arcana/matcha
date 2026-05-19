use std::collections::HashMap;

use parking_lot::Mutex;
use wgpu::PipelineCompilationOptions;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PushConstant {
    color: [f32; 4],
}

const PUSH_CONSTANT_SIZE: u32 = std::mem::size_of::<PushConstant>() as u32;

// ---------------------------------------------------------------------------
// Public wrapper — no cfg
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(super) struct ViewportClear {
    inner: Mutex<Option<ViewportClearInner>>,
}

impl ViewportClear {
    pub(super) fn render(
        &self,
        queue: &wgpu::Queue,
        render_pass: &mut wgpu::RenderPass<'_>,
        target_format: wgpu::TextureFormat,
        device: &wgpu::Device,
        color: [f32; 4],
    ) {
        let mut guard = self.inner.lock();
        let inner = guard.get_or_insert_with(|| ViewportClearInner::new(device));
        inner.render(queue, render_pass, target_format, device, color);
    }

    pub(super) fn reset(&self) {
        *self.inner.lock() = None;
    }
}

// ---------------------------------------------------------------------------
// Native implementation (push constants)
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{HashMap, PipelineCompilationOptions, PushConstant, PUSH_CONSTANT_SIZE};

    pub(super) struct ViewportClearInner {
        pipeline_layout: wgpu::PipelineLayout,
        shader: wgpu::ShaderModule,
        pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    }

    impl ViewportClearInner {
        pub(super) fn new(device: &wgpu::Device) -> Self {
            let pipeline_layout =
                device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("atlas_viewport_clear_pipeline_layout"),
                    bind_group_layouts: &[],
                    push_constant_ranges: &[wgpu::PushConstantRange {
                        stages: wgpu::ShaderStages::FRAGMENT,
                        range: 0..PUSH_CONSTANT_SIZE,
                    }],
                });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("atlas_viewport_clear_shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("viewport_clear.wgsl").into(),
                ),
            });
            Self {
                pipeline_layout,
                shader,
                pipelines: HashMap::new(),
            }
        }

        fn pipeline(
            &mut self,
            device: &wgpu::Device,
            format: wgpu::TextureFormat,
        ) -> &wgpu::RenderPipeline {
            let shader = &self.shader;
            let pipeline_layout = &self.pipeline_layout;
            self.pipelines.entry(format).or_insert_with(|| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("atlas_viewport_clear_pipeline"),
                    layout: Some(pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: shader,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: PipelineCompilationOptions::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: PipelineCompilationOptions::default(),
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview: None,
                    cache: None,
                })
            })
        }

        pub(super) fn render(
            &mut self,
            _queue: &wgpu::Queue,
            render_pass: &mut wgpu::RenderPass<'_>,
            target_format: wgpu::TextureFormat,
            device: &wgpu::Device,
            color: [f32; 4],
        ) {
            let pipeline = self.pipeline(device, target_format);
            render_pass.set_pipeline(pipeline);
            render_pass.set_push_constants(
                wgpu::ShaderStages::FRAGMENT,
                0,
                bytemuck::bytes_of(&PushConstant { color }),
            );
            render_pass.draw(0..4, 0..1);
        }
    }
}

// ---------------------------------------------------------------------------
// WASM implementation (uniform buffer)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::{HashMap, PipelineCompilationOptions, PushConstant};

    pub(super) struct ViewportClearInner {
        pipeline_layout: wgpu::PipelineLayout,
        shader: wgpu::ShaderModule,
        pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
        uniform_buffer: wgpu::Buffer,
        bind_group_layout: wgpu::BindGroupLayout,
        bind_group: wgpu::BindGroup,
    }

    impl ViewportClearInner {
        pub(super) fn new(device: &wgpu::Device) -> Self {
            let bind_group_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("atlas_viewport_clear_bgl"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });
            let pipeline_layout =
                device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("atlas_viewport_clear_pipeline_layout"),
                    bind_group_layouts: &[&bind_group_layout],
                    push_constant_ranges: &[],
                });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("atlas_viewport_clear_shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("viewport_clear_web.wgsl").into(),
                ),
            });
            let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("atlas_viewport_clear_uniform"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("atlas_viewport_clear_bind_group"),
                layout: &bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                }],
            });
            Self {
                pipeline_layout,
                shader,
                pipelines: HashMap::new(),
                uniform_buffer,
                bind_group_layout,
                bind_group,
            }
        }

        fn pipeline(
            &mut self,
            device: &wgpu::Device,
            format: wgpu::TextureFormat,
        ) -> &wgpu::RenderPipeline {
            let shader = &self.shader;
            let pipeline_layout = &self.pipeline_layout;
            self.pipelines.entry(format).or_insert_with(|| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("atlas_viewport_clear_pipeline"),
                    layout: Some(pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: shader,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: PipelineCompilationOptions::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: PipelineCompilationOptions::default(),
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview: None,
                    cache: None,
                })
            })
        }

        pub(super) fn render(
            &mut self,
            queue: &wgpu::Queue,
            render_pass: &mut wgpu::RenderPass<'_>,
            target_format: wgpu::TextureFormat,
            device: &wgpu::Device,
            color: [f32; 4],
        ) {
            let pipeline = self.pipeline(device, target_format);
            render_pass.set_pipeline(pipeline);
            queue.write_buffer(
                &self.uniform_buffer,
                0,
                bytemuck::bytes_of(&PushConstant { color }),
            );
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.draw(0..4, 0..1);
        }
    }
}

// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
use native::ViewportClearInner;
#[cfg(target_arch = "wasm32")]
use wasm::ViewportClearInner;
