//! Private widget painting into logical outputs. CPU/UI-dependent values are
//! resolved before these owned commands are captured by a resource generator.
//! No atlas coordinates, final-frame phases or queue submissions live here.
use render_interface::{
    GpuPrepareContext, PrepareResult, TextureDescriptor, TexturePrepareContext, TextureTarget,
    upload_texture,
};
use std::{cell::RefCell, sync::Arc};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Vertex {
    pub position: [f32; 4],
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

pub(crate) struct ImageData {
    pub size: [u32; 2],
    /// sRGB encoding of linear-premultiplied RGB, with linear alpha.
    pub pixels: Vec<u8>,
}

pub(crate) fn image_data(image: image::DynamicImage) -> ImageData {
    let mut image = image.to_rgba8();
    let size = [image.width(), image.height()];
    for pixel in image.as_mut().chunks_exact_mut(4) {
        let alpha = pixel[3] as f32 / 255.;
        for channel in &mut pixel[..3] {
            let s = *channel as f32 / 255.;
            let linear = if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            } * alpha;
            let encoded = if linear <= 0.0031308 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1. / 2.4) - 0.055
            };
            *channel = (encoded * 255.).round().clamp(0., 255.) as u8;
        }
    }
    ImageData {
        size,
        pixels: image.into_raw(),
    }
}

/// Geometry is already resolved in logical output pixels, before viewport mapping.
pub(crate) fn vertex(
    position: [f32; 2],
    color: [f32; 4],
    uv: [f32; 2],
    transform: nalgebra::Matrix4<f32>,
    offset: [f32; 2],
) -> Vertex {
    let p = transform * nalgebra::Vector4::new(position[0], position[1], 0., 1.);
    Vertex {
        position: [p.x - offset[0] * p.w, p.y - offset[1] * p.w, 0., p.w],
        color,
        uv,
    }
}

pub(crate) fn rectangle(
    min: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
    offset: [f32; 2],
) -> Vec<Vertex> {
    [[0., 0.], [0., 1.], [1., 1.], [0., 0.], [1., 1.], [1., 0.]]
        .into_iter()
        .map(|uv| {
            vertex(
                [min[0] + uv[0] * size[0], min[1] + uv[1] * size[1]],
                color,
                uv,
                nalgebra::Matrix4::identity(),
                offset,
            )
        })
        .collect()
}

pub(crate) fn clear(context: &mut TexturePrepareContext<'_>, rgba: [f32; 4]) {
    let [r, g, b, a] = rgba;
    let attachments = [Some(wgpu::RenderPassColorAttachment {
        view: context.target.view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color {
                r: (r * a) as f64,
                g: (g * a) as f64,
                b: (b * a) as f64,
                a: a as f64,
            }),
            store: wgpu::StoreOp::Store,
        },
    })];
    let _pass = context
        .gpu
        .encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("widget logical clear"),
            color_attachments: &attachments,
            ..Default::default()
        });
}

struct Pipelines {
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    color: wgpu::RenderPipeline,
    image: wgpu::RenderPipeline,
    images: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}
thread_local! { static PIPELINES: RefCell<Option<Pipelines>> = const { RefCell::new(None) }; }

impl Pipelines {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let images = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("widget image"),
            entries: &[
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("widget painters"),
            source: wgpu::ShaderSource::Wgsl(include_str!("paint.wgsl").into()),
        });
        let pipeline = |fragment: &str, textured: bool| {
            let image_layouts = [Some(&images)];
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: if textured { &image_layouts } else { &[] },
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{label:Some(fragment),layout:Some(&layout),
                vertex:wgpu::VertexState{module:&shader,entry_point:Some("vertex"),buffers:&[wgpu::VertexBufferLayout{array_stride:40,step_mode:wgpu::VertexStepMode::Vertex,attributes:&wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x2]}],compilation_options:Default::default()},
                fragment:Some(wgpu::FragmentState{module:&shader,entry_point:Some(fragment),targets:&[Some(wgpu::ColorTargetState{format,blend:None,write_mask:wgpu::ColorWrites::ALL})],compilation_options:Default::default()}),primitive:Default::default(),depth_stencil:None,multisample:Default::default(),multiview_mask:None,cache:None,
            })
        };
        Self {
            device: device.clone(),
            format,
            color: pipeline("color", false),
            image: pipeline("image", true),
            images,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }
}

pub(crate) fn draw(
    context: &mut TexturePrepareContext<'_>,
    vertices: &[Vertex],
    image: Option<&Arc<ImageData>>,
) -> PrepareResult {
    if vertices.is_empty() {
        return Ok(());
    }
    let size = context.target.desc.size;
    let mut vertices = vertices.to_vec();
    for v in &mut vertices {
        let [x, y, _, w] = v.position;
        v.position = [
            2. * x / size[0] as f32 - w,
            w - 2. * y / size[1] as f32,
            0.,
            w,
        ];
    }
    let buffer = context
        .gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("widget painter vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
    let image_view = if let Some(data) = image {
        let desc = TextureDescriptor::new(data.size, wgpu::TextureFormat::Rgba8UnormSrgb);
        let texture = context.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("private image upload"),
            size: wgpu::Extent3d {
                width: data.size[0],
                height: data.size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: desc.format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut gpu = GpuPrepareContext {
            device: context.gpu.device,
            encoder: context.gpu.encoder,
            snapshot: context.gpu.snapshot,
        };
        upload_texture(
            &mut gpu,
            &TextureTarget {
                desc: &desc,
                texture: &texture,
                view: &view,
            },
            &data.pixels,
        )?;
        Some(view)
    } else {
        None
    };
    PIPELINES.with_borrow_mut(|cache| {
        if cache.as_ref().is_none_or(|p| {
            p.device != *context.gpu.device || p.format != context.target.desc.format
        }) {
            *cache = Some(Pipelines::new(
                context.gpu.device,
                context.target.desc.format,
            ));
        }
        let pipelines = cache.as_ref().expect("pipeline cache initialized");
        let group = image_view.as_ref().map(|view| {
            context
                .gpu
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &pipelines.images,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&pipelines.sampler),
                        },
                    ],
                })
        });
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: context.target.view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = context
            .gpu
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("widget logical paint"),
                color_attachments: &attachments,
                ..Default::default()
            });
        pass.set_pipeline(if group.is_some() {
            &pipelines.image
        } else {
            &pipelines.color
        });
        if let Some(group) = &group {
            pass.set_bind_group(0, group, &[]);
        }
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.draw(0..vertices.len() as u32, 0..1);
    });
    Ok(())
}
