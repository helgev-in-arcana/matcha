use std::collections::HashMap;

use parking_lot::Mutex;
use wgpu::PipelineCompilationOptions;

#[derive(Default)]
pub(super) struct ViewportClear {
    inner: Mutex<Option<ViewportClearInner>>,
}

struct ViewportClearInner {
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    #[cfg(target_arch = "wasm32")]
    uniform_buffer: wgpu::Buffer,
    #[cfg(target_arch = "wasm32")]
    bind_group_layout: wgpu::BindGroupLayout,
    #[cfg(target_arch = "wasm32")]
    bind_group: wgpu::BindGroup,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PushConstant {
    color: [f32; 4],
}

const PUSH_CONSTANT_SIZE: u32 = std::mem::size_of::<PushConstant>() as u32;

impl ViewportClearInner {
    fn new(device: &wgpu::Device) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let (pipeline_layout, bgl_opt) = {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("atlas_viewport_clear_pipeline_layout"),
                bind_group_layouts: &[],
                push_constant_ranges: &[wgpu::PushConstantRange {
                    stages: wgpu::ShaderStages::FRAGMENT,
                    range: 0..PUSH_CONSTANT_SIZE,
                }],
            });
            (layout, ())
        };

        #[cfg(target_arch = "wasm32")]
        let (pipeline_layout, bgl_opt) = {
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("atlas_viewport_clear_pipeline_layout"),
                bind_group_layouts: &[&bgl],
                push_constant_ranges: &[],
            });
            (layout, bgl)
        };

        #[cfg(not(target_arch = "wasm32"))]
        let src = include_str!("viewport_clear.wgsl");
        #[cfg(target_arch = "wasm32")]
        let src = include_str!("viewport_clear_web.wgsl");

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("atlas_viewport_clear_shader"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });

        #[cfg(target_arch = "wasm32")]
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("atlas_viewport_clear_uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        #[cfg(target_arch = "wasm32")]
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas_viewport_clear_bind_group"),
            layout: &bgl_opt,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // suppress unused warning on native
        let _ = &bgl_opt;

        ViewportClearInner {
            pipeline_layout,
            shader,
            pipelines: HashMap::new(),
            #[cfg(target_arch = "wasm32")]
            bind_group_layout: bgl_opt,
            #[cfg(target_arch = "wasm32")]
            uniform_buffer,
            #[cfg(target_arch = "wasm32")]
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
        let pipeline = inner.pipeline(device, target_format);

        render_pass.set_pipeline(pipeline);

        let constants = PushConstant { color };
        #[cfg(not(target_arch = "wasm32"))]
        render_pass.set_push_constants(
            wgpu::ShaderStages::FRAGMENT,
            0,
            bytemuck::bytes_of(&constants),
        );
        #[cfg(target_arch = "wasm32")]
        {
            queue.write_buffer(&inner.uniform_buffer, 0, bytemuck::bytes_of(&constants));
            render_pass.set_bind_group(0, &inner.bind_group, &[]);
        }

        // suppress unused warning on native
        let _ = queue;

        render_pass.draw(0..4, 0..1);
    }

    pub(super) fn reset(&self) {
        *self.inner.lock() = None;
    }
}
