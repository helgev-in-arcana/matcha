use utils::rwoption::RwOption;
use wgpu::PipelineCompilationOptions;

use crate::push_constant_buffer::PcBuffer;
use crate::pipeline_cache::PipelineCache;

/// Simple renderer that overwrites a scissored rectangle with a transparent color.
/// API mirrors other widgets_renderer modules: create a small struct with an inner impl and a `render` method.
#[derive(Default)]
pub struct ViewportClear {
    inner: RwOption<ViewportClearImpl>,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PushConstant {
    color: [f32; 4],
}

struct ViewportClearImpl {
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: PipelineCache<wgpu::TextureFormat, wgpu::RenderPipeline>,
    pc_buffer: PcBuffer<PushConstant>,
}

impl ViewportClearImpl {
    fn setup(device: &wgpu::Device) -> Self {
        let pc_layout = PcBuffer::<PushConstant>::bind_group_layout(device);
        let pc_ranges = PcBuffer::<PushConstant>::push_constant_ranges(
            wgpu::ShaderStages::FRAGMENT,
            std::mem::size_of::<PushConstant>() as u32,
        );

        #[cfg(not(target_arch = "wasm32"))]
        let bgl: &[&wgpu::BindGroupLayout] = &[];
        #[cfg(target_arch = "wasm32")]
        let bgl: &[&wgpu::BindGroupLayout] = &[&pc_layout];

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport_clear_pipeline_layout"),
            bind_group_layouts: bgl,
            push_constant_ranges: &pc_ranges,
        });

        let pc_buffer = PcBuffer::new(device, &pc_layout);
        let pipeline = PipelineCache::new();

        ViewportClearImpl {
            pipeline_layout,
            pipeline,
            pc_buffer,
        }
    }
}

impl ViewportClear {
    pub fn reset(&self) {
        self.inner.take();
    }

    pub fn render(
        &self,
        queue: &wgpu::Queue,
        render_pass: &mut wgpu::RenderPass<'_>,
        target_format: wgpu::TextureFormat,
        device: &wgpu::Device,
        color: [f32; 4],
    ) {
        let ViewportClearImpl {
            pipeline_layout,
            pipeline,
            pc_buffer,
        } = &*self
            .inner
            .get_or_insert_with(|| ViewportClearImpl::setup(device));

        let pipeline = pipeline.get_or_insert(target_format, || {
            make_pipeline(device, target_format, pipeline_layout)
        });

        render_pass.set_pipeline(&pipeline);

        let push_constant = PushConstant { color };
        pc_buffer.apply_to_render_pass(
            queue,
            render_pass,
            0,
            wgpu::ShaderStages::FRAGMENT,
            0,
            &push_constant,
        );

        render_pass.draw(0..4, 0..1);
    }
}

fn make_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    pipeline_layout: &wgpu::PipelineLayout,
) -> wgpu::RenderPipeline {
    #[cfg(not(target_arch = "wasm32"))]
    let src = include_str!("viewport_clear.wgsl");
    #[cfg(target_arch = "wasm32")]
    let src = include_str!("viewport_clear_web.wgsl");

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("viewport_clear_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("viewport_clear_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
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
}
