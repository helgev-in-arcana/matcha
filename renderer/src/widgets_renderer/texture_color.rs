use crate::pipeline_cache::PipelineCache;
use crate::push_constant_buffer::PcBuffer;
use crate::vertex::uv_vertex::UvVertex;
use utils::rwoption::RwOption;
use wgpu::util::DeviceExt;

/* NOTE: This renderer assumes textures use top-origin UV coordinates (v = 0 at the top).
UvVertex.tex_coords passed to this pipeline must have v = 0 at the top of the image.
If your texture data uses bottom-origin coordinates, invert the v component before
rendering (e.g. use 1.0 - v). */

pub struct TextureColor {
    inner: RwOption<TextureColorImpl>,
}

struct TextureColorImpl {
    texture_bind_group_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: PipelineCache<wgpu::TextureFormat, wgpu::RenderPipeline>,
    texture_sampler: wgpu::Sampler,
    pc_buffer: PcBuffer<nalgebra::Matrix4<f32>>,
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

        let pc_layout = PcBuffer::<nalgebra::Matrix4<f32>>::bind_group_layout(device);
        let pc_ranges = PcBuffer::<nalgebra::Matrix4<f32>>::push_constant_ranges(
            wgpu::ShaderStages::VERTEX,
            std::mem::size_of::<nalgebra::Matrix4<f32>>() as u32,
        );

        // On native: texture at group 0, no UBO group.
        // On WASM: texture at group 0, UBO at group 1.
        #[cfg(not(target_arch = "wasm32"))]
        let bgl: &[&wgpu::BindGroupLayout] = &[&texture_bind_group_layout];
        #[cfg(target_arch = "wasm32")]
        let bgl: &[&wgpu::BindGroupLayout] = &[&texture_bind_group_layout, &pc_layout];

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("TextureColor: Pipeline Layout"),
            bind_group_layouts: bgl,
            push_constant_ranges: &pc_ranges,
        });

        let pc_buffer = PcBuffer::new(device, &pc_layout);
        let pipeline = PipelineCache::new();

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
            pc_buffer,
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
        queue: &wgpu::Queue,
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

        let render_pipeline = inner.pipeline.get_or_insert(target_format, || {
            make_pipeline(device, target_format, &inner.pipeline_layout)
        });

        let view_port_affine_transform =
            affine_transform([target_size[0] as f32, target_size[1] as f32], position);

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("texture_color_vertex_buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("texture_color_index_buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });

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

        render_pass.set_pipeline(&render_pipeline);
        render_pass.set_bind_group(0, &texture_bind_group, &[]);
        inner.pc_buffer.apply_to_render_pass(
            queue,
            render_pass,
            1,
            wgpu::ShaderStages::VERTEX,
            0,
            &view_port_affine_transform,
        );
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
    #[cfg(not(target_arch = "wasm32"))]
    let src = include_str!("texture_color.wgsl");
    #[cfg(target_arch = "wasm32")]
    let src = include_str!("texture_color_web.wgsl");

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("texture_color_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
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
