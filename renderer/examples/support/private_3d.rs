//! A texture generator may own arbitrary private rendering machinery. This cube
//! uses its own vertex ABI, camera and depth attachment; only the resulting color
//! enters the UI compositor. Inter-object scene depth/G-buffer is not implied.
use render_interface::*;
use wgpu::util::DeviceExt;
pub fn cube(size: [u32; 2], angle: f32) -> TextureSource {
    let mut desc = TextureDescriptor::new(size, wgpu::TextureFormat::Rgba8Unorm);
    desc.usages = wgpu::TextureUsages::RENDER_ATTACHMENT;
    TextureSource::new(desc, move |ctx| {
        let corners = [
            [-1., -1., -1.],
            [1., -1., -1.],
            [1., 1., -1.],
            [-1., 1., -1.],
            [-1., -1., 1.],
            [1., -1., 1.],
            [1., 1., 1.],
            [-1., 1., 1.],
        ];
        let faces = [
            ([4, 5, 6, 7], [0.05, 0.65, 0.95]),
            ([1, 0, 3, 2], [0.8, 0.18, 0.12]),
            ([0, 4, 7, 3], [0.15, 0.8, 0.4]),
            ([5, 1, 2, 6], [0.8, 0.4, 0.04]),
            ([7, 6, 2, 3], [0.6, 0.3, 0.9]),
            ([0, 1, 5, 4], [0.05, 0.3, 0.5]),
        ];
        let mut vertices = Vec::<[f32; 6]>::new();
        for (face, color) in faces {
            for i in [0, 1, 2, 0, 2, 3] {
                let p = corners[face[i]];
                vertices.push([p[0], p[1], p[2], color[0], color[1], color[2]]);
            }
        }
        let vertex = ctx
            .gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("private cube geometry"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let camera = nalgebra::Perspective3::new(size[0] as f32 / size[1] as f32, 0.85, 0.1, 20.)
            .to_homogeneous()
            * Matrix4::look_at_rh(
                &nalgebra::Point3::new(3., 2.3, 4.5),
                &nalgebra::Point3::origin(),
                &nalgebra::Vector3::y(),
            )
            * Matrix4::from_euler_angles(0.2, angle, 0.);
        let uniform = ctx
            .gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("private camera"),
                contents: bytemuck::bytes_of(&camera),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let shader=ctx.gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {label:Some("private depth renderer"),source:wgpu::ShaderSource::Wgsl(r#"
@group(0) @binding(0) var<uniform> camera:mat4x4<f32>;
struct Out {@builtin(position) position:vec4<f32>,@location(0) color:vec3<f32>};
@vertex fn vertex(@location(0) position:vec3<f32>,@location(1) color:vec3<f32>)->Out {
    let clip=camera*vec4<f32>(position,1.);var out:Out;out.position=vec4<f32>(clip.xy,0.5*(clip.z+clip.w),clip.w);out.color=color;return out;
}
@fragment fn fragment(in:Out)->@location(0) vec4<f32> {return vec4<f32>(in.color,1.);}
"#.into())});
        let pipeline = ctx
            .gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("private 3D pipeline"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 24,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3],
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fragment"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: desc.format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth24Plus,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
        let group = ctx
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                }],
            });
        let depth = ctx.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("private depth"),
            size: ctx.target.texture.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24Plus,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&Default::default());
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: ctx.target.view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = ctx
            .gpu
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("private 3D render"),
                color_attachments: &attachments,
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_vertex_buffer(0, vertex.slice(..));
        pass.draw(0..36, 0..1);
        Ok(())
    })
}
