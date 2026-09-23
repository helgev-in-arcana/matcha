//! Box masks are GPU-generated directly through MaskPrepareContext. The CPU
//! rasterizer in shape.rs is an independent oracle for numerical stress tests.
//! Separable blur preserves the old three-box filter including its quantization.
use crate::shape::CoverageKey;
use render_interface::*;
use wgpu::util::DeviceExt;

pub(crate) struct ShapeGpu {
    device: wgpu::Device,
    shape: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    dummy: wgpu::TextureView,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    size: [f32; 4],
    radius: [f32; 4],
    border: [f32; 4],
    direction: [i32; 4],
}
impl ShapeGpu {
    pub(crate) fn for_device(&self, device: &wgpu::Device) -> bool {
        self.device == *device
    }
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("widget shape inputs"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("widget SDF and shadow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shape_gpu.wgsl").into()),
        });
        let pipeline = |fragment| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fragment),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::R8Unorm,
                        blend: None,
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
        };
        Self {
            device: device.clone(),
            shape: pipeline("shape"),
            blur: pipeline("blur"),
            layout,
            dummy: image(device, [1, 1]).create_view(&Default::default()),
        }
    }
    pub(crate) fn prepare(&self, ctx: MaskPrepareContext<'_>, key: CoverageKey) -> PrepareResult {
        let radius = (key.blur_16th as f32 / 16. * 1.12).round();
        let p = Params {
            size: [
                key.w as f32,
                key.h as f32,
                key.inset_16th as f32 / 16.,
                radius,
            ],
            radius: key.radius_16th.map(|v| v as f32 / 16.),
            border: key.border_16th.map(|v| v as f32 / 16.),
            direction: [0; 4],
        };
        if radius == 0. {
            self.draw(
                ctx.gpu.encoder,
                ctx.target.view,
                &self.dummy,
                &self.shape,
                p,
            );
            return Ok(());
        }
        let a = image(ctx.gpu.device, [key.w, key.h]);
        let b = image(ctx.gpu.device, [key.w, key.h]);
        let av = a.create_view(&Default::default());
        let bv = b.create_view(&Default::default());
        self.draw(ctx.gpu.encoder, &av, &self.dummy, &self.shape, p);
        for i in 0..3 {
            self.draw(
                ctx.gpu.encoder,
                &bv,
                &av,
                &self.blur,
                Params {
                    direction: [1, 0, 0, 0],
                    ..p
                },
            );
            self.draw(
                ctx.gpu.encoder,
                if i == 2 { ctx.target.view } else { &av },
                &bv,
                &self.blur,
                Params {
                    direction: [0, 1, 0, 0],
                    ..p
                },
            );
        }
        Ok(())
    }
    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        input: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        p: Params,
    ) {
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shape parameters"),
                contents: bytemuck::bytes_of(&p),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(input),
                },
            ],
        });
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("widget resource generation"),
            color_attachments: &attachments,
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}
fn image(device: &wgpu::Device, size: [u32; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("widget shadow work"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}
