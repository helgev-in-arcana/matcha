use utils::rwoption::RwOption;
use wgpu::PipelineCompilationOptions;

/// Simple renderer that overwrites a scissored rectangle with a transparent color.
/// API mirrors other widgets_renderer modules: create a small struct with an inner impl and a `render` method.
#[derive(Default)]
pub struct ViewportClear {
    inner: RwOption<ViewportClearImpl>,
}

const PIPELINE_CACHE_SIZE: u64 = 4;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PushConstant {
    color: [f32; 4],
}

const _: () = {
    assert!(
        wgpu::PUSH_CONSTANT_ALIGNMENT == 4,
        "PushConstant alignment changed. check memory layout"
    );
};

const PUSH_CONSTANTS_SIZE: u32 = std::mem::size_of::<PushConstant>() as u32;

struct ViewportClearImpl {
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: crate::pipeline_cache::PipelineCache<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl ViewportClearImpl {
    fn setup(device: &wgpu::Device) -> Self {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport_clear_pipeline_layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[wgpu::PushConstantRange {
                stages: wgpu::ShaderStages::FRAGMENT,
                range: 0..PUSH_CONSTANTS_SIZE,
            }],
        });

        let pipeline = crate::pipeline_cache::PipelineCache::new(PIPELINE_CACHE_SIZE);

        ViewportClearImpl {
            pipeline_layout,
            pipeline,
        }
    }
}

impl ViewportClear {
    pub fn render(
        &self,
        render_pass: &mut wgpu::RenderPass<'_>,
        target_format: wgpu::TextureFormat,
        device: &wgpu::Device,
        color: [f32; 4],
    ) {
        let ViewportClearImpl {
            pipeline_layout,
            pipeline,
        } = &*self
            .inner
            .get_or_insert_with(|| ViewportClearImpl::setup(device));

        let pipeline = pipeline.get_with(target_format, || {
            make_pipeline(device, target_format, pipeline_layout)
        });

        render_pass.set_pipeline(&pipeline);

        let push_constants = PushConstant { color };
        render_pass.set_push_constants(
            wgpu::ShaderStages::FRAGMENT,
            0,
            bytemuck::cast_slice(&[push_constants]),
        );

        render_pass.draw(0..4, 0..1);
    }
}

fn make_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    pipeline_layout: &wgpu::PipelineLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("viewport_clear_shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("viewport_clear.wgsl").into()),
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
