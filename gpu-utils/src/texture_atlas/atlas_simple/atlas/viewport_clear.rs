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
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PushConstant {
    color: [f32; 4],
}

const PUSH_CONSTANT_SIZE: u32 = std::mem::size_of::<PushConstant>() as u32;

impl ViewportClearInner {
    fn new(device: &wgpu::Device) -> Self {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("atlas_viewport_clear_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: PUSH_CONSTANT_SIZE,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("atlas_viewport_clear_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("viewport_clear.wgsl").into()),
        });

        ViewportClearInner {
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
        self.pipelines.entry(format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("atlas_viewport_clear_pipeline"),
                layout: Some(&self.pipeline_layout),
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
                multiview_mask: None,
                cache: None,
            })
        })
    }
}

impl ViewportClear {
    pub(super) fn render(
        &self,
        device: &wgpu::Device,
        render_pass: &mut wgpu::RenderPass<'_>,
        target_format: wgpu::TextureFormat,
        color: [f32; 4],
    ) {
        let mut guard = self.inner.lock();
        let inner = guard.get_or_insert_with(|| ViewportClearInner::new(device));
        let pipeline = inner.pipeline(device, target_format);

        render_pass.set_pipeline(pipeline);
        let constants = PushConstant { color };
        render_pass.set_immediates(0, bytemuck::bytes_of(&constants));
        render_pass.draw(0..4, 0..1);
    }

    pub(super) fn reset(&self) {
        *self.inner.lock() = None;
    }
}
