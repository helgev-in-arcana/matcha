use nalgebra::Matrix4;
use utils::rwoption::RwOption;
use wgpu::PipelineCompilationOptions;

/*
bind group 0:
    @binding(0) texture_2d<f32>
    @binding(1) sampler

push constants (as PushConstant struct):
    target_texture_size: vec2<f32>
    source_texture_position_min: vec2<f32>
    source_texture_position_max: vec2<f32>
    color_transformation: mat4x4<f32>
    color_offset: vec4<f32>
*/

// vertex position will be calculated in the vertex shader (`vs_main`)
// color will be calculated in the fragment shader (`fs_main`)

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PushConstant {
    color_transformation: Matrix4<f32>,
    color_offset: [f32; 4],
    target_texture_size: [f32; 2],
    source_texture_position_min: [f32; 2],
    source_texture_position_max: [f32; 2],
}

const _: () = {
    assert!(
        wgpu::IMMEDIATE_DATA_ALIGNMENT == 4,
        "PushConstant alignment changed. check memory layout"
    );
};

const PUSH_CONSTANTS_SIZE: u32 = std::mem::size_of::<PushConstant>() as u32;

/// Copy texture data from one texture to another in a wgpu pipeline,
/// with offset and size parameters.
#[derive(Default)]
pub struct TextureCopy {
    inner: RwOption<TextureCopyImpl>,
}

const PIPELINE_CACHE_SIZE: u64 = 4;

struct TextureCopyImpl {
    texture_bind_group_layout: wgpu::BindGroupLayout,
    texture_sampler: wgpu::Sampler,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: crate::pipeline_cache::PipelineCache<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl TextureCopyImpl {
    fn setup(device: &wgpu::Device) -> Self {
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("texture_copy_bind_group_layout"),
                entries: &[
                    // texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("texture_copy_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("texture_copy_pipeline_layout"),
            bind_group_layouts: &[Some(&texture_bind_group_layout)],
            immediate_size: PUSH_CONSTANTS_SIZE,
        });

        let pipeline = crate::pipeline_cache::PipelineCache::new(PIPELINE_CACHE_SIZE);

        TextureCopyImpl {
            texture_bind_group_layout,
            texture_sampler,
            pipeline_layout,
            pipeline,
        }
    }
}

pub struct TargetData {
    pub target_size: [u32; 2],
    pub target_format: wgpu::TextureFormat,
}

pub struct RenderData<'a> {
    pub source_texture_view: &'a wgpu::TextureView,
    pub source_texture_position_min: [f32; 2],
    pub source_texture_position_max: [f32; 2],
    pub color_transformation: Option<Matrix4<f32>>,
    pub color_offset: Option<[f32; 4]>,
}

impl TextureCopy {
    pub fn render(
        &self,
        render_pass: &mut wgpu::RenderPass<'_>,
        TargetData {
            target_size,
            target_format,
        }: TargetData,
        RenderData {
            source_texture_view: source_texture,
            source_texture_position_min,
            source_texture_position_max,
            color_transformation,
            color_offset,
        }: RenderData<'_>,
        device: &wgpu::Device,
    ) {
        let TextureCopyImpl {
            texture_bind_group_layout,
            texture_sampler,
            pipeline_layout,
            pipeline,
        } = &*self
            .inner
            .get_or_insert_with(|| TextureCopyImpl::setup(device));

        let render_pipeline = pipeline.get_with(target_format, || {
            make_pipeline(device, target_format, pipeline_layout)
        });

        render_pass.set_pipeline(&render_pipeline);
        render_pass.set_bind_group(
            0,
            &device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("TextureCopyBindGroup"),
                layout: texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source_texture),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(texture_sampler),
                    },
                ],
            }),
            &[],
        );
        let push_constants = PushConstant {
            target_texture_size: [target_size[0] as f32, target_size[1] as f32],
            source_texture_position_min,
            source_texture_position_max,
            color_transformation: color_transformation.unwrap_or_else(Matrix4::identity),
            color_offset: color_offset.unwrap_or([0.0; 4]),
        };
        render_pass.set_immediates(0, bytemuck::cast_slice(&[push_constants]));
        render_pass.draw(0..4, 0..1);
    }
}

fn make_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    pipeline_layout: &wgpu::PipelineLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("texture_copy_shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("texture_copy.wgsl").into()),
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("texture_copy_pipeline"),
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
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    })
}
