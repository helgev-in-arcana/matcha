/*
push constants:
    [[f32; 4]; 4] // composed affine matrix
*/

use crate::vertex::colored_vertex::ColorVertex;
use utils::rwoption::RwOption;
use wgpu::{PipelineCompilationOptions, util::DeviceExt};

pub struct VertexColor {
    inner: RwOption<VertexColorImpl>,
}

const PIPELINE_CACHE_SIZE: u64 = 4;

struct VertexColorImpl {
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: crate::pipeline_cache::PipelineCache<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl VertexColorImpl {
    fn setup(device: &wgpu::Device) -> Self {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vertex_color_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: (std::mem::size_of::<nalgebra::Matrix4<f32>>() as u32),
        });

        let pipeline = crate::pipeline_cache::PipelineCache::new(PIPELINE_CACHE_SIZE);

        Self {
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
    pub transform: nalgebra::Matrix4<f32>,
    pub vertices: &'a [ColorVertex],
    pub indices: &'a [u16],
}

impl Default for VertexColor {
    fn default() -> Self {
        Self {
            inner: RwOption::new(),
        }
    }
}

impl VertexColor {
    pub fn render(
        &self,
        render_pass: &mut wgpu::RenderPass<'_>,
        TargetData {
            target_size,
            target_format,
        }: TargetData,
        RenderData {
            transform,
            vertices,
            indices,
        }: RenderData,
        device: &wgpu::Device,
    ) {
        let VertexColorImpl {
            pipeline_layout,
            pipeline,
        } = &*self
            .inner
            .get_or_insert_with(|| VertexColorImpl::setup(device));

        let render_pipeline = pipeline.get_with(target_format, || {
            make_pipeline(device, target_format, pipeline_layout)
        });

        let view_port_affine_transform =
            viewport_transform([target_size[0] as f32, target_size[1] as f32]) * transform; // compose adaptive affine (style-provided) after viewport transform

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertex_color_vertex_buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertex_color_index_buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        render_pass.set_pipeline(&render_pipeline);
        render_pass.set_immediates(
            0,
            bytemuck::cast_slice(view_port_affine_transform.as_slice()),
        );
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        render_pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
    }
}

fn viewport_transform(viewport_size: [f32; 2]) -> nalgebra::Matrix4<f32> {
    let scale = nalgebra::Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(
        2.0 / viewport_size[0],
        -2.0 / viewport_size[1],
        1.0,
    ));

    let transform = nalgebra::Matrix4::new_translation(&nalgebra::Vector3::new(-1.0, 1.0, 0.0));

    transform * scale
}

fn make_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    pipeline_layout: &wgpu::PipelineLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vertex_color_shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("vertex_color.wgsl").into()),
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("vertex_color_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[ColorVertex::desc()],
            compilation_options: PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(target_format.into())],
            compilation_options: PipelineCompilationOptions::default(),
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
        multiview_mask: None,
        cache: None,
    })
}
