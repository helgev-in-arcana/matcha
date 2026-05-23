use utils::rwoption::RwOption;
use wgpu::util::DeviceExt;

use crate::vertex::uv_vertex::UvVertex;
/* NOTE: This renderer assumes textures use top-origin UV coordinates (v = 0 at the top).
UvVertex.tex_coords passed to this pipeline must have v = 0 at the top of the image.
If your texture data uses bottom-origin coordinates, invert the v component before
rendering (e.g. use 1.0 - v). */

// API similar to line_strip.rs:
// - TextureColor is Default and lazily initializes inner impl on first render
// - Pipeline cached per target format using moka::sync::Cache
// - Push constants used for affine matrix (vertex stage)

const PIPELINE_CACHE_SIZE: u64 = 4;

pub struct TextureColor {
    inner: RwOption<TextureColorImpl>,
}

struct TextureColorImpl {
    texture_bind_group_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: crate::pipeline_cache::PipelineCache<wgpu::TextureFormat, wgpu::RenderPipeline>,
    texture_sampler: wgpu::Sampler,
}

impl TextureColorImpl {
    fn setup(device: &wgpu::Device) -> Self {
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("TextureColor: Texture Bind Group Layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("TextureColor: Pipeline Layout"),
            bind_group_layouts: &[&texture_bind_group_layout],
            push_constant_ranges: &[wgpu::PushConstantRange {
                stages: wgpu::ShaderStages::VERTEX,
                range: 0..(std::mem::size_of::<nalgebra::Matrix4<f32>>() as u32),
            }],
        });

        let pipeline = crate::pipeline_cache::PipelineCache::new(PIPELINE_CACHE_SIZE);

        let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("TextureColor: Texture Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            texture_bind_group_layout,
            pipeline_layout,
            pipeline,
            texture_sampler,
        }
    }
}

pub struct TargetData {
    pub target_size: [u32; 2],
    pub target_format: wgpu::TextureFormat,
}

pub struct RenderData<'a> {
    pub position: [f32; 2],
    pub vertices: &'a [UvVertex],
    pub indices: &'a [u16],
    pub texture_view: &'a wgpu::TextureView,
}

impl Default for TextureColor {
    fn default() -> Self {
        Self {
            inner: RwOption::new(),
        }
    }
}

impl TextureColor {
    pub fn render(
        &self,
        render_pass: &mut wgpu::RenderPass<'_>,
        TargetData {
            target_size,
            target_format,
        }: TargetData,
        RenderData {
            position,
            vertices,
            indices,
            texture_view,
        }: RenderData,
        device: &wgpu::Device,
    ) {
        let inner = self
            .inner
            .get_or_insert_with(|| TextureColorImpl::setup(device));

        // get or create pipeline for this target format
        let render_pipeline = inner.pipeline.get_with(target_format, || {
            make_pipeline(device, target_format, &inner.pipeline_layout)
        });

        // compute viewport affine transform
        let view_port_affine_transform =
            affine_transform([target_size[0] as f32, target_size[1] as f32], position);

        // push constant (affine matrix) - must be set after pipeline is set
        // create vertex buffer
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("texture_color_vertex_buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // create index buffer
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("texture_color_index_buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // texture bind group
        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("TextureColor: Texture Bind Group"),
            layout: &inner.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&inner.texture_sampler),
                },
            ],
        });

        // set pipeline and resources
        render_pass.set_pipeline(&render_pipeline);
        render_pass.set_push_constants(
            wgpu::ShaderStages::VERTEX,
            0,
            bytemuck::cast_slice(view_port_affine_transform.as_slice()),
        );
        render_pass.set_bind_group(0, &texture_bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        render_pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
    }
}

fn make_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    pipeline_layout: &wgpu::PipelineLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("texture_color_shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("texture_color.wgsl").into()),
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("texture_color_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[UvVertex::desc()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(target_format.into())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
        cache: None,
    })
}

#[rustfmt::skip]
fn affine_transform(
    viewport_size: [f32; 2],
    position: [f32; 2],
) -> nalgebra::Matrix4<f32> {
    let position = nalgebra::Matrix4::new_translation(&nalgebra::Vector3::new(
        position[0],
        position[1],
        0.0,
    ));

    let transform = nalgebra::Matrix4::new_translation(&nalgebra::Vector3::new(
        -1.0,
        1.0,
        0.0,
    ));

    let scale = nalgebra::Matrix4::new_nonuniform_scaling(
        &nalgebra::Vector3::new(
            2.0 / viewport_size[0],
            -2.0 / viewport_size[1],
            1.0,
        ),
    );

    transform * scale * position
}
