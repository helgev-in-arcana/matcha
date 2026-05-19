use crate::pipeline_cache::PipelineCache;
use crate::push_constant_buffer::PcBuffer;
use gpu_utils::texture_atlas;
use std::sync::Arc;
use utils::rwoption::RwOption;
use wgpu::util::DeviceExt;

#[cfg(not(target_arch = "wasm32"))]
const WGSL_DRAW: &str = include_str!("./bezier_2d_draw.wgsl");
#[cfg(target_arch = "wasm32")]
const WGSL_DRAW: &str = include_str!("./bezier_2d_draw_web.wgsl");

const WGSL_COMPUTE: &str = include_str!("./bezier_2d_compute.wgsl");
const WGSL_COMMAND: &str = include_str!("./bezier_2d_command.wgsl");

const COMPUTE_WORKGROUP_SIZE: u32 = 64;

const VERTEX_DESC: wgpu::VertexBufferLayout = wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &[wgpu::VertexAttribute {
        offset: 0,
        shader_location: 0,
        format: wgpu::VertexFormat::Float32x2,
    }],
};

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct BezierInfo {
    num_anchors: u32,
    div: u32,
    width: f32,
    _padding: u32,
}

/// Combined affine_transform + color for the draw pass (native: split push constants; WASM: single UBO).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawPushConstants {
    affine_transform: nalgebra::Matrix4<f32>,
    color: [f32; 4],
}

#[derive(Default)]
pub struct Bezier2d {
    inner: RwOption<Arc<Bezier2dImpl>>,
}

struct Bezier2dImpl {
    // Bind Group Layouts
    data_bind_group_layout: wgpu::BindGroupLayout,

    // Pipeline Layouts
    compute_pipeline_layout: wgpu::PipelineLayout,
    command_pipeline_layout: wgpu::PipelineLayout,
    draw_pipeline_layout: wgpu::PipelineLayout,

    // Pipelines
    compute_pipeline: wgpu::ComputePipeline,
    command_pipeline: wgpu::ComputePipeline,
    draw_pipeline: PipelineCache<wgpu::TextureFormat, Arc<wgpu::RenderPipeline>>,

    // reusable resources
    draw_command_buffer: wgpu::Buffer,
    draw_command_storage: wgpu::Buffer,

    draw_pc_buffer: PcBuffer<DrawPushConstants>,
}

pub struct TargetData {
    pub atlas_region: texture_atlas::AtlasRegion,
}

pub struct RenderData<'a> {
    pub position: [f32; 2],
    pub anchors: &'a [nalgebra::Vector2<f32>],
    pub div: u32,
    pub width: f32,
    pub color: [f32; 4],
}

impl Bezier2dImpl {
    pub fn setup(device: &wgpu::Device) -> Arc<Self> {
        let data_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("bezier_2d_data_bind_group_layout"),
                entries: &[
                    // 0: BezierInfo
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // 1: Anchor Points
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // 2: Vertex Buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // 3: Draw Command Storage
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let (compute_pipeline_layout, compute_pipeline) =
            make_compute_pipeline(device, &data_bind_group_layout);
        let (command_pipeline_layout, command_pipeline) =
            make_command_pipeline(device, &data_bind_group_layout);

        let draw_pc_layout = PcBuffer::<DrawPushConstants>::bind_group_layout(device);
        let draw_pipeline_layout = make_draw_pipeline_layout(device, &draw_pc_layout);

        let draw_pc_buffer = PcBuffer::new(device, &draw_pc_layout);

        let draw_pipeline = PipelineCache::new();

        let draw_command_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bezier_2d_draw_command_buffer"),
            size: std::mem::size_of::<wgpu::util::DrawIndirectArgs>() as u64,
            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let draw_command_storage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bezier_2d_draw_command_storage_buffer"),
            size: std::mem::size_of::<wgpu::util::DrawIndirectArgs>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        Arc::new(Self {
            data_bind_group_layout,
            compute_pipeline_layout,
            command_pipeline_layout,
            draw_pipeline_layout,
            compute_pipeline,
            command_pipeline,
            draw_pipeline,
            draw_command_buffer,
            draw_command_storage,
            draw_pc_buffer,
        })
    }
}

impl Bezier2d {
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        command_encoder: &mut wgpu::CommandEncoder,
        TargetData { atlas_region }: TargetData,
        RenderData {
            position,
            anchors,
            div,
            width,
            color,
        }: RenderData,
        device: &wgpu::Device,
    ) {
        if anchors.len() < 2 || div == 0 {
            return;
        }

        let num_anchors = anchors.len() as u32;
        let compute_vertices = div + 1;
        let num_vertices = compute_vertices + 2;

        let target_format = atlas_region.format();
        let target_size = atlas_region.texture_size();

        let bezier_impl = self
            .inner
            .get_or_insert_with(|| Bezier2dImpl::setup(device));

        let draw_pipeline = bezier_impl.draw_pipeline.get_or_insert(target_format, || {
            Arc::new(make_draw_pipeline(
                device,
                target_format,
                &bezier_impl.draw_pipeline_layout,
            ))
        });

        let info = BezierInfo {
            num_anchors,
            div,
            width,
            _padding: 0,
        };
        let info_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bezier_2d_info_buffer"),
            contents: bytemuck::bytes_of(&info),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let anchor_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bezier_2d_anchor_buffer"),
            contents: bytemuck::cast_slice(anchors),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bezier_2d_vertex_buffer"),
            size: (std::mem::size_of::<[f32; 2]>() * num_vertices as usize) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });

        let data_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bezier_2d_data_bind_group"),
            layout: &bezier_impl.data_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: info_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: anchor_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: vertex_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: bezier_impl.draw_command_storage.as_entire_binding(),
                },
            ],
        });

        // Compute Pass
        {
            let mut compute_pass =
                command_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("bezier_2d_compute_pass"),
                    timestamp_writes: None,
                });
            compute_pass.set_pipeline(&bezier_impl.compute_pipeline);
            compute_pass.set_bind_group(0, &data_bind_group, &[]);
            compute_pass.dispatch_workgroups(
                compute_vertices.div_ceil(COMPUTE_WORKGROUP_SIZE),
                1,
                1,
            );
        }

        // Command Pass
        {
            let mut command_pass =
                command_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("bezier_2d_command_pass"),
                    timestamp_writes: None,
                });
            command_pass.set_pipeline(&bezier_impl.command_pipeline);
            command_pass.set_bind_group(0, &data_bind_group, &[]);
            command_pass.dispatch_workgroups(1, 1, 1);
        }

        command_encoder.copy_buffer_to_buffer(
            &bezier_impl.draw_command_storage,
            0,
            &bezier_impl.draw_command_buffer,
            0,
            std::mem::size_of::<wgpu::util::DrawIndirectArgs>() as u64,
        );

        let Ok(mut render_pass) = atlas_region.begin_render_pass(command_encoder) else {
            return;
        };

        let affine = affine_transform([target_size[0] as f32, target_size[1] as f32], position);
        let draw_pc = DrawPushConstants {
            affine_transform: affine,
            color,
        };

        render_pass.set_pipeline(&draw_pipeline);

        // Native: two separate push constant ranges (VERTEX + FRAGMENT).
        // WASM: single UBO at group 0 via PcBuffer.
        #[cfg(not(target_arch = "wasm32"))]
        {
            render_pass.set_push_constants(
                wgpu::ShaderStages::VERTEX,
                0,
                bytemuck::cast_slice(affine.as_slice()),
            );
            render_pass.set_push_constants(
                wgpu::ShaderStages::FRAGMENT,
                std::mem::size_of::<nalgebra::Matrix4<f32>>() as u32,
                bytemuck::cast_slice(&color),
            );
        }
        #[cfg(target_arch = "wasm32")]
        {
            bezier_impl.draw_pc_buffer.apply_to_render_pass(
                queue,
                &mut render_pass,
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                0,
                &draw_pc,
            );
        }

        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.draw_indirect(&bezier_impl.draw_command_buffer, 0);
    }
}

fn make_compute_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> (wgpu::PipelineLayout, wgpu::ComputePipeline) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("bezier_2d_compute_shader"),
        source: wgpu::ShaderSource::Wgsl(WGSL_COMPUTE.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("bezier_2d_compute_pipeline_layout"),
        bind_group_layouts: &[bind_group_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("bezier_2d_compute_pipeline"),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    (pipeline_layout, pipeline)
}

fn make_command_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> (wgpu::PipelineLayout, wgpu::ComputePipeline) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("bezier_2d_command_shader"),
        source: wgpu::ShaderSource::Wgsl(WGSL_COMMAND.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("bezier_2d_command_pipeline_layout"),
        bind_group_layouts: &[bind_group_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("bezier_2d_command_pipeline"),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    (pipeline_layout, pipeline)
}

fn make_draw_pipeline_layout(
    device: &wgpu::Device,
    pc_layout: &wgpu::BindGroupLayout,
) -> wgpu::PipelineLayout {
    let matrix_size = std::mem::size_of::<nalgebra::Matrix4<f32>>() as u32;
    let color_size = std::mem::size_of::<[f32; 4]>() as u32;

    #[cfg(not(target_arch = "wasm32"))]
    let pc_ranges: &[wgpu::PushConstantRange] = &[
        wgpu::PushConstantRange {
            stages: wgpu::ShaderStages::VERTEX,
            range: 0..matrix_size,
        },
        wgpu::PushConstantRange {
            stages: wgpu::ShaderStages::FRAGMENT,
            range: matrix_size..(matrix_size + color_size),
        },
    ];
    #[cfg(not(target_arch = "wasm32"))]
    let bgl: &[&wgpu::BindGroupLayout] = &[];

    #[cfg(target_arch = "wasm32")]
    let pc_ranges: &[wgpu::PushConstantRange] = &[];
    #[cfg(target_arch = "wasm32")]
    let bgl: &[&wgpu::BindGroupLayout] = &[pc_layout];

    // suppress unused warning on native
    let _ = pc_layout;

    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("bezier_2d_draw_pipeline_layout"),
        bind_group_layouts: bgl,
        push_constant_ranges: pc_ranges,
    })
}

fn make_draw_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    pipeline_layout: &wgpu::PipelineLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("bezier_2d_draw_shader"),
        source: wgpu::ShaderSource::Wgsl(WGSL_DRAW.into()),
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("bezier_2d_draw_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[VERTEX_DESC],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
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
