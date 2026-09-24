//! Ordered mesh and mask composition. This module sees resolved GPU resources,
//! not content IDs, source callbacks, cache policies or submission ownership.
use render_interface::*;
use std::{collections::HashMap, ops::Range};

use crate::resources::{Image, Mesh, extent, make_image};
use wgpu::util::DeviceExt;
pub(crate) const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(crate) const COVERAGE: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Params {
    pub(crate) transform: Matrix4<f32>,
    pub(crate) viewport: [f32; 2],
    pub(crate) opacity: f32,
    pub(crate) masked: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuParams {
    pub(crate) base: Params,
    pub(crate) source_uv: [f32; 4],
    pub(crate) local_uv: [f32; 4],
}

pub(crate) struct ImageRef<'a> {
    pub(crate) view: &'a wgpu::TextureView,
    pub(crate) uv: [f32; 4],
}
impl<'a> From<&'a Image> for ImageRef<'a> {
    fn from(image: &'a Image) -> Self {
        Self {
            view: &image.view,
            uv: image.uv,
        }
    }
}
impl<'a> From<&'a wgpu::TextureView> for ImageRef<'a> {
    fn from(view: &'a wgpu::TextureView) -> Self {
        Self {
            view,
            uv: [0., 0., 1., 1.],
        }
    }
}

pub(crate) struct Compositor {
    pub(crate) device: wgpu::Device,
    layout: wgpu::BindGroupLayout,
    pub(crate) pipeline_layout: wgpu::PipelineLayout,
    pub(crate) shader: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    pub(crate) color_pipeline: wgpu::RenderPipeline,
    pub(crate) mask_pipeline: wgpu::RenderPipeline,
    pub(crate) clear_pipeline: wgpu::RenderPipeline,
    pub(crate) outputs: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    pub(crate) quad: Mesh,
    pub(crate) white: Image,
    pub(crate) parameter_buffer: Option<wgpu::Buffer>,
    pub(crate) parameter_bytes: Vec<u8>,
}
impl Compositor {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene resources"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(112),
                    },
                    count: None,
                },
                texture_binding(1, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture_binding(3, false),
                texture_binding(4, true),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene compositor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compositor.wgsl").into()),
        });
        let color_pipeline = pipeline(
            device,
            &pipeline_layout,
            &shader,
            COLOR,
            "color",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let maximum = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Max,
        };
        let mask_pipeline = pipeline(
            device,
            &pipeline_layout,
            &shader,
            COVERAGE,
            "mask",
            Some(wgpu::BlendState {
                color: maximum,
                alpha: maximum,
            }),
        );
        let clear_pipeline = pipeline(device, &pipeline_layout, &shader, COVERAGE, "color", None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let vertices = [
            Vertex {
                position: [0., 0., 0.],
                uv: [0., 0.],
            },
            Vertex {
                position: [0., 1., 0.],
                uv: [0., 1.],
            },
            Vertex {
                position: [1., 1., 0.],
                uv: [1., 1.],
            },
            Vertex {
                position: [0., 0., 0.],
                uv: [0., 0.],
            },
            Vertex {
                position: [1., 1., 0.],
                uv: [1., 1.],
            },
            Vertex {
                position: [1., 0., 0.],
                uv: [1., 0.],
            },
        ];
        let quad = Mesh {
            desc: MeshDescriptor::triangles(6, 0),
            vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("compositor quad"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            indices: None,
            vertex_range: 0..120,
            index_range: 0..0,
        };
        let white = make_image(device, TextureDescriptor::new([1, 1], COVERAGE));
        queue.write_texture(
            white.texture.as_image_copy(),
            &[255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            extent([1, 1]),
        );
        Self {
            device: device.clone(),
            layout,
            pipeline_layout,
            shader,
            sampler,
            color_pipeline,
            mask_pipeline,
            clear_pipeline,
            outputs: HashMap::new(),
            quad,
            white,
            parameter_buffer: None,
            parameter_bytes: Vec::new(),
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &self,
        frame: &mut DrawFrame,
        destination: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        mesh: &Mesh,
        image: ImageRef<'_>,
        mask: &wgpu::TextureView,
        params: Params,
        scissor: Option<[u32; 4]>,
        local: Option<ImageRef<'_>>,
    ) {
        let offset = frame.bytes.len();
        let local = local.unwrap_or_else(|| ImageRef::from(&self.white));
        let gpu_params = GpuParams {
            base: params,
            source_uv: image.uv,
            local_uv: local.uv,
        };
        frame
            .bytes
            .extend_from_slice(bytemuck::bytes_of(&gpu_params));
        frame.bytes.resize(offset + frame.stride, 0);
        let key = (image.view.clone(), mask.clone(), local.view.clone());
        let group = frame
            .groups
            .entry(key)
            .or_insert_with(|| {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("scene resident pages"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &frame.uniforms,
                                offset: 0,
                                size: wgpu::BufferSize::new(112),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(image.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(mask),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(local.view),
                        },
                    ],
                })
            })
            .clone();
        if frame
            .destination
            .as_ref()
            .is_some_and(|view| view != destination)
        {
            flush(frame);
        }
        frame.destination = Some(destination.clone());
        frame.pending.push(DrawCall {
            pipeline: pipeline.clone(),
            group,
            vertices: mesh.vertices.clone(),
            indices: mesh.indices.clone(),
            vertex_range: mesh.vertex_range.clone(),
            index_range: mesh.index_range.clone(),
            desc: mesh.desc,
            offset: offset as u32,
            scissor,
        });
    }
}
pub(crate) struct DrawFrame {
    pub(crate) encoder: wgpu::CommandEncoder,
    pub(crate) uniforms: wgpu::Buffer,
    pub(crate) bytes: Vec<u8>,
    pub(crate) stride: usize,
    pub(crate) pending: Vec<DrawCall>,
    pub(crate) destination: Option<wgpu::TextureView>,
    pub(crate) batches: usize,
    pub(crate) groups:
        HashMap<(wgpu::TextureView, wgpu::TextureView, wgpu::TextureView), wgpu::BindGroup>,
}
pub(crate) struct DrawCall {
    pub(crate) vertex_range: Range<u64>,
    pub(crate) index_range: Range<u64>,
    pub(crate) pipeline: wgpu::RenderPipeline,
    pub(crate) group: wgpu::BindGroup,
    pub(crate) vertices: wgpu::Buffer,
    pub(crate) indices: Option<wgpu::Buffer>,
    pub(crate) desc: MeshDescriptor,
    pub(crate) offset: u32,
    pub(crate) scissor: Option<[u32; 4]>,
}
pub(crate) fn flush(frame: &mut DrawFrame) {
    let Some(destination) = frame.destination.take() else {
        return;
    };
    frame.batches += 1;
    let attachments = [Some(wgpu::RenderPassColorAttachment {
        view: &destination,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        },
    })];
    {
        let mut pass = frame
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene ordered batch"),
                color_attachments: &attachments,
                ..Default::default()
            });
        for draw in &frame.pending {
            let [x, y, w, h] = draw.scissor.unwrap_or([
                0,
                0,
                destination.texture().width(),
                destination.texture().height(),
            ]);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_pipeline(&draw.pipeline);
            pass.set_bind_group(0, &draw.group, &[draw.offset]);
            pass.set_vertex_buffer(0, draw.vertices.slice(draw.vertex_range.clone()));
            if let Some(indices) = &draw.indices {
                pass.set_index_buffer(
                    indices.slice(draw.index_range.clone()),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(0..draw.desc.index_count, 0, 0..1);
            } else {
                pass.draw(0..draw.desc.vertex_count, 0..1);
            }
        }
    }
    frame.pending.clear();
}
pub(crate) fn clear(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    color: wgpu::Color,
) {
    let attachments = [Some(wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(color),
            store: wgpu::StoreOp::Store,
        },
    })];
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("scene clear"),
        color_attachments: &attachments,
        ..Default::default()
    });
}
pub(crate) fn texture_binding(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
pub(crate) fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    fragment: &str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("scene pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 20,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}
pub(crate) fn mask_slot(depth: usize) -> usize {
    if depth < 4 { depth } else { 4 + depth % 2 }
}
pub(crate) fn intersection(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    [
        x,
        y,
        (a[0] + a[2]).min(b[0] + b[2]).saturating_sub(x),
        (a[1] + a[3]).min(b[1] + b[3]).saturating_sub(y),
    ]
}
pub(crate) fn pixel_bounds(
    bounds: Option<[[f32; 3]; 2]>,
    transform: &Matrix4<f32>,
    viewport: [f32; 2],
    size: [u32; 2],
) -> [u32; 4] {
    let full = [0, 0, size[0], size[1]];
    let Some([min, max]) = bounds else {
        return full;
    };
    let mut low = [f32::INFINITY; 2];
    let mut high = [f32::NEG_INFINITY; 2];
    for x in [min[0], max[0]] {
        for y in [min[1], max[1]] {
            for z in [min[2], max[2]] {
                let p = transform * nalgebra::Vector4::new(x, y, z, 1.);
                if p.w <= 0. || p.iter().any(|v| !v.is_finite()) {
                    return full;
                }
                for (i, v) in [p.x / p.w, p.y / p.w].into_iter().enumerate() {
                    let v = v * size[i] as f32 / viewport[i];
                    low[i] = low[i].min(v);
                    high[i] = high[i].max(v);
                }
            }
        }
    }
    let left = low[0].floor().clamp(0., size[0] as f32) as u32;
    let top = low[1].floor().clamp(0., size[1] as f32) as u32;
    let right = high[0].ceil().clamp(0., size[0] as f32) as u32;
    let bottom = high[1].ceil().clamp(0., size[1] as f32) as u32;
    [
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    ]
}
