use log::{debug, trace, warn};
use std::sync::Arc;

use crate::render_node::RenderNode;
use gpu_utils::texture_atlas;
use texture_atlas::RegionError;
use thiserror::Error;

const WGSL_CULL: &str = include_str!("core_renderer/renderer_cull.wgsl");
const WGSL_COMMAND: &str = include_str!("core_renderer/renderer_command.wgsl");
const WGSL_RENDER: &str = include_str!("core_renderer/renderer_render.wgsl");

const PIPELINE_CACHE_SIZE: u64 = 3;
const COMPUTE_WORKGROUP_SIZE: u32 = 64;

// PERF NOTE:
// - BindGroup/Buffer の再利用・リング化を検討（毎フレームの生成/全量 write を抑制）
// - 2 Compute パス（cull→command）の統合可能性検討（最後のスレッドで間接引数を書き込む）
// - ステンシル/テクスチャの BindGroup はアトラス更新時のみ再生成
// - カリングの多角形交差でエッジ交差のみのケース対策（必要性を確認し、線分交差チェックを追加）

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
/// InstanceData describes a single textured instance to be rendered.
///
/// Semantics:
/// - `viewport_position`: 4x4 matrix that maps the unit quad vertices
///   (defined using top-left origin and Y-down as {[0, 0], [0, 1], [1, 1], [1, 0]})
///   into the destination coordinate space prior to normalization. Public renderer APIs
///   accept coordinates in pixels with the origin at the top-left and Y increasing downward.
///   The renderer internally converts these coordinates to the form expected by the GPU
///   pipeline (including any Y inversion) before uploading InstanceData to the GPU.
/// - `atlas_page`: index of the texture array layer (page) inside the texture atlas.
/// - `in_atlas_offset`: (x, y) offset of the sub-image inside the atlas page.
///   Expected units: NORMALIZED UVS (0.0 .. 1.0) relative to the atlas page by default.
///   If the atlas implementation returns pixel coordinates, the host MUST convert
///   them to normalized coordinates before writing InstanceData into GPU memory.
/// - `in_atlas_size`: (width, height) size of the sub-image. Expected as NORMALIZED
///   values (0.0 .. 1.0). If atlas returns pixel sizes, normalize on the host side.
/// - `stencil_index`: index+1 of the associated stencil in the stencil data array.
///   0 indicates "no stencil". The shader uses `stencil_index - 1` to access the stencil.
///
/// NOTE: Keep Rust-side layout (#[repr(C)] + bytemuck) compatible with the WGSL
/// `InstanceData` struct (field order, types, and padding). When changing fields,
/// update both Rust and WGSL declarations simultaneously.
struct InstanceData {
    /// transform vertex: {[0, 0], [0, 1], [1, 1], [1, 0]} to where the texture should be rendered
    viewport_position: nalgebra::Matrix4<f32>,
    atlas_page: u32,
    _padding1: u32,
    /// [x, y] (normalized UVs expected)
    in_atlas_offset: [f32; 2],
    /// [width, height] (normalized size expected)
    in_atlas_size: [f32; 2],
    /// the index of the stencil in the stencil data array.
    /// 0 if no stencil is used. Use `stencil_index - 1` in the shader.
    stencil_index: u32,
    _padding2: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
/// StencilData describes a stencil polygon used to mask instances.
///
/// Semantics:
/// - `viewport_position`: transform mapping the unit quad into stencil space.
///   Public renderer APIs accept coordinates with the origin at the top-left and Y
///   increasing downward; the renderer converts these to the internal form required
///   by the GPU pipeline.
/// - `viewport_position_inverse_exists`: non-zero if `viewport_position` is invertible.
/// - `viewport_position_inverse`: inverse matrix used by the vertex shader to compute
///   stencil-space UV coordinates for masking.
/// - `atlas_page`: index of the stencil atlas page (texture array layer).
/// - `in_atlas_offset` / `in_atlas_size`: offset and size of the stencil image inside
///   the atlas page. Expected to be NORMALIZED UVs (0.0 .. 1.0). If atlas returns
///   pixel coordinates, the host MUST normalize them before uploading to GPU.
///
/// NOTE: Maintain identical memory layout between this Rust struct and the WGSL
/// `StencilData` declaration (including explicit padding fields). Update both
/// definitions when changing sizes/types.
struct StencilData {
    /// transform vertex: {[0, 0], [0, 1], [1, 1], [1, 0]} to where the stencil should be rendered
    viewport_position: nalgebra::Matrix4<f32>,
    /// if the inverse of the viewport position exists.
    /// 0 if the inverse does not exist.
    viewport_position_inverse_exists: u32,
    _padding1: [u32; 3],
    /// inverse of the viewport position matrix.
    /// used to calculate stencil uv coordinates in the shader.
    viewport_position_inverse: nalgebra::Matrix4<f32>,
    atlas_page: u32,
    _padding2: u32,
    /// [x, y] (normalized UVs expected)
    in_atlas_offset: [f32; 2],
    /// [width, height] (normalized size expected)
    in_atlas_size: [f32; 2],
    _padding3: [u32; 2],
}

const _: () = {
    assert!(std::mem::size_of::<InstanceData>() == 96);
    assert!(std::mem::size_of::<StencilData>() == 176);
    assert!(std::mem::size_of::<RenderPushConstants>() == 80);
};

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct CullingPushConstants {
    normalize_matrix: nalgebra::Matrix4<f32>,
    instance_count: u32,
    _pad: [u32; 3],
}

/// Half a texel, in normalized UV units, for each atlas. Used by the render
/// shader to inset the UV clamp bounds so that sampling at the very edge of
/// an atlas region's usable (non-margin) rectangle can never land exactly on
/// the boundary between the last real texel and the next (zero-initialised
/// margin) texel — bilinear filtering would blend 50/50 with that margin
/// texel there, bleeding the background colour into the edge of every
/// texture/stencil sample. Insetting to the last texel's *centre* instead of
/// its edge guarantees a pure, unblended sample.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct RenderPushConstants {
    normalize_matrix: nalgebra::Matrix4<f32>,
    texture_atlas_half_texel: [f32; 2],
    stencil_atlas_half_texel: [f32; 2],
}

pub struct CoreRenderer {
    inner: parking_lot::RwLock<CoreRendererInner>,
}

impl CoreRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let inner = CoreRendererInner::new(device);
        Self {
            inner: parking_lot::RwLock::new(inner),
        }
    }
}

impl CoreRenderer {
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        // gpu
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        // surface format
        surface_format: wgpu::TextureFormat,
        // destination
        destination_view: &wgpu::TextureView,
        destination_size: [f32; 2],
        // objects
        render_node: &RenderNode,
        load_color: wgpu::Color,
        // texture atlas
        texture_atlas: &wgpu::Texture,
        stencil_atlas: &wgpu::Texture,
    ) -> Result<(), TextureValidationError> {
        let inner_lock = self.inner.read();
        inner_lock.render(
            device,
            queue,
            surface_format,
            destination_view,
            destination_size,
            render_node,
            load_color,
            texture_atlas,
            stencil_atlas,
        )
    }

    /// Flat render entry point: draw several pre-transformed render trees in one
    /// frame. See [`CoreRendererInner::render_flat`].
    #[allow(clippy::too_many_arguments)]
    pub fn render_flat(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        destination_view: &wgpu::TextureView,
        destination_size: [f32; 2],
        items: &[(Arc<RenderNode>, nalgebra::Matrix4<f32>)],
        load_color: wgpu::Color,
        texture_atlas: &wgpu::Texture,
        stencil_atlas: &wgpu::Texture,
    ) -> Result<(), TextureValidationError> {
        let inner_lock = self.inner.read();
        inner_lock.render_flat(
            device,
            queue,
            surface_format,
            destination_view,
            destination_size,
            items,
            load_color,
            texture_atlas,
            stencil_atlas,
        )
    }
}

pub struct CoreRendererInner {
    // Bind Group Layouts
    texture_sampler: wgpu::Sampler,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    data_bind_group_layout: wgpu::BindGroupLayout,

    // Pipeline Layouts
    culling_pipeline_layout: wgpu::PipelineLayout,
    command_pipeline_layout: wgpu::PipelineLayout,
    render_pipeline_layout: wgpu::PipelineLayout,
    render_pipeline_shader_module: wgpu::ShaderModule,

    // Pipelines
    culling_pipeline: wgpu::ComputePipeline,
    command_pipeline: wgpu::ComputePipeline,
    render_pipeline:
        crate::pipeline_cache::PipelineCache<wgpu::TextureFormat, Arc<wgpu::RenderPipeline>>, // key: surface format

    // reusable buffers
    atomic_counter: wgpu::Buffer,
    draw_command: wgpu::Buffer,
    draw_command_storage: wgpu::Buffer,
}

impl CoreRendererInner {
    pub fn new(device: &wgpu::Device) -> Self {
        debug!("CoreRenderer::new: initializing renderer");
        // Sampler
        let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ObjectRenderer Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            border_color: Some(wgpu::SamplerBorderColor::TransparentBlack),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjectRenderer Texture Bind Group Layout"),
                entries: &[
                    // Texture Sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // Texture Atlas
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2Array,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // Stencil Atlas
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2Array,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });

        // Culling Pipeline
        let data_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Culling Bind Group Layout"),
                entries: &[
                    // All Instances Buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // All Stencils Buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE
                            | wgpu::ShaderStages::FRAGMENT
                            | wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // Visible Instances Buffer
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
                    // Atomic Counter
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
                    // command buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
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

        let (culling_pipeline_layout, culling_pipeline) =
            Self::create_culling_pipeline(device, &data_bind_group_layout);

        let (command_pipeline_layout, command_pipeline) =
            Self::create_command_pipeline(device, &data_bind_group_layout);

        let (render_pipeline_layout, render_pipeline_shader_module) =
            Self::create_render_pipeline_layout(
                device,
                &texture_bind_group_layout,
                &data_bind_group_layout,
            );
        trace!("CoreRenderer::new: pipeline layouts created");

        let render_pipeline = crate::pipeline_cache::PipelineCache::new(PIPELINE_CACHE_SIZE);

        // Create buffers
        let atomic_counter = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Atomic Counter Buffer"),
            size: std::mem::size_of::<u32>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let draw_command = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Draw Command Buffer"),
            size: (std::mem::size_of::<wgpu::util::DrawIndirectArgs>()) as u64,
            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let draw_command_storage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Draw Command Storage Buffer"),
            size: (std::mem::size_of::<wgpu::util::DrawIndirectArgs>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        trace!("CoreRenderer::new: renderer state initialized");

        Self {
            texture_sampler,
            texture_bind_group_layout,
            data_bind_group_layout,
            culling_pipeline_layout,
            command_pipeline_layout,
            render_pipeline_layout,
            render_pipeline_shader_module,
            culling_pipeline,
            command_pipeline,
            render_pipeline,
            atomic_counter,
            draw_command,
            draw_command_storage,
        }
    }

    fn create_culling_pipeline(
        device: &wgpu::Device,
        bind_group_layout: &wgpu::BindGroupLayout,
    ) -> (wgpu::PipelineLayout, wgpu::ComputePipeline) {
        trace!("CoreRenderer::create_culling_pipeline: creating pipeline");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Culling Shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL_CULL.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Culling Pipeline Layout"),
            bind_group_layouts: &[Some(bind_group_layout)],
            immediate_size: std::mem::size_of::<CullingPushConstants>() as u32,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Culling Pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("culling_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        trace!("CoreRenderer::create_culling_pipeline: pipeline ready");

        (pipeline_layout, pipeline)
    }

    fn create_command_pipeline(
        device: &wgpu::Device,
        bind_group_layout: &wgpu::BindGroupLayout,
    ) -> (wgpu::PipelineLayout, wgpu::ComputePipeline) {
        trace!("CoreRenderer::create_command_pipeline: creating pipeline");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Command Shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL_COMMAND.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Command Pipeline Layout"),
            bind_group_layouts: &[Some(bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Command Pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("command_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        trace!("CoreRenderer::create_command_pipeline: pipeline ready");

        (pipeline_layout, pipeline)
    }

    fn create_render_pipeline_layout(
        device: &wgpu::Device,
        texture_bind_group_layout: &wgpu::BindGroupLayout,
        data_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> (wgpu::PipelineLayout, wgpu::ShaderModule) {
        trace!("CoreRenderer::create_render_pipeline_layout: creating pipeline layout");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Render Shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL_RENDER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Render Pipeline Layout"),
            bind_group_layouts: &[
                Some(texture_bind_group_layout),
                Some(data_bind_group_layout),
            ],
            immediate_size: std::mem::size_of::<RenderPushConstants>() as u32,
        });

        (pipeline_layout, module)
    }

    fn create_render_pipeline(
        device: &wgpu::Device,
        render_pipeline_layout: &wgpu::PipelineLayout,
        shader_module: &wgpu::ShaderModule,
        target_format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        trace!(
            "CoreRenderer::create_render_pipeline: building pipeline for format {target_format:?}"
        );
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader_module,
                entry_point: Some("vertex_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: shader_module,
                entry_point: Some("fragment_main"),
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
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        // gpu
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        // surface format
        surface_format: wgpu::TextureFormat,
        // destination
        destination_view: &wgpu::TextureView,
        destination_size: [f32; 2],
        // objects
        render_node: &RenderNode,
        load_color: wgpu::Color,
        // texture atlas
        texture_atlas: &wgpu::Texture,
        stencil_atlas: &wgpu::Texture,
    ) -> Result<(), TextureValidationError> {
        trace!(
            "CoreRenderer::render: begin render_node_count={} surface_format={:?} destination_size={:?}",
            render_node.count(),
            surface_format,
            destination_size
        );
        // #[cfg(debug_assertions)]
        // {
        //     println!(
        //         "[CoreRenderer] render: {} objects, destination_size={:?}, surface_format={:?}",
        //         render_node.count(),
        //         destination_size,
        //         surface_format,
        //     );

        //     println!("[CoreRenderer] render_node: {render_node:#?}",);
        // }

        // integrate objects into a instance array
        let (instances, stencils) = create_instance_and_stencil_data(
            render_node,
            texture_atlas.format(),
            stencil_atlas.format(),
        )?;

        self.render_instances(
            device,
            queue,
            surface_format,
            destination_view,
            destination_size,
            &instances,
            &stencils,
            load_color,
            texture_atlas,
            stencil_atlas,
        )
    }

    /// Flat variant of [`render`](Self::render): renders several already
    /// window-space-transformed render trees (paint order) in a single frame.
    /// Used by the M4 render-thread path, which extracts one `(RenderNode,
    /// transform)` per widget entity instead of building a pseudo root.
    #[allow(clippy::too_many_arguments)]
    pub fn render_flat(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        destination_view: &wgpu::TextureView,
        destination_size: [f32; 2],
        items: &[(Arc<RenderNode>, nalgebra::Matrix4<f32>)],
        load_color: wgpu::Color,
        texture_atlas: &wgpu::Texture,
        stencil_atlas: &wgpu::Texture,
    ) -> Result<(), TextureValidationError> {
        let (instances, stencils) = create_instance_and_stencil_data_flat(
            items,
            texture_atlas.format(),
            stencil_atlas.format(),
        )?;

        self.render_instances(
            device,
            queue,
            surface_format,
            destination_view,
            destination_size,
            &instances,
            &stencils,
            load_color,
            texture_atlas,
            stencil_atlas,
        )
    }

    /// Encode and submit a prepared instance/stencil set. Shared by
    /// [`render`](Self::render) (single tree) and [`render_flat`](Self::render_flat)
    /// (multiple pre-transformed roots).
    #[allow(clippy::too_many_arguments)]
    fn render_instances(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        destination_view: &wgpu::TextureView,
        destination_size: [f32; 2],
        instances: &[InstanceData],
        stencils: &[StencilData],
        load_color: wgpu::Color,
        texture_atlas: &wgpu::Texture,
        stencil_atlas: &wgpu::Texture,
    ) -> Result<(), TextureValidationError> {
        trace!(
            "CoreRenderer::render_instances: prepared {} instances and {} stencils",
            instances.len(),
            stencils.len()
        );

        // #[cfg(debug_assertions)]
        // {
        //     println!("[CoreRenderer] instances: {instances:#?}",);
        // }

        if instances.is_empty() {
            trace!("CoreRenderer::render: no instances to render");
            return Ok(());
        }

        // get or create render pipeline that matches given surface format
        let render_pipeline = self.render_pipeline.get_with(surface_format, || {
            trace!("CoreRenderer::render: creating render pipeline for format {surface_format:?}");
            Arc::new(Self::create_render_pipeline(
                device,
                &self.render_pipeline_layout,
                &self.render_pipeline_shader_module,
                surface_format,
            ))
        });

        // Create buffers
        let all_instance_data_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Instance Buffer"),
            size: (std::mem::size_of::<InstanceData>() * instances.len()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let all_stencil_data_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Stencil Buffer"),
            size: (std::mem::size_of::<StencilData>() * stencils.len().max(1)) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let visible_instance_indices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Visible Instances Buffer"),
            size: (std::mem::size_of::<u32>() * instances.len()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Create bind groups
        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjectRenderer Texture Bind Group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&self.texture_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture_atlas.create_view(
                        &wgpu::TextureViewDescriptor {
                            dimension: Some(wgpu::TextureViewDimension::D2Array),
                            aspect: wgpu::TextureAspect::All,
                            ..Default::default()
                        },
                    )),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&stencil_atlas.create_view(
                        &wgpu::TextureViewDescriptor {
                            dimension: Some(wgpu::TextureViewDimension::D2Array),
                            aspect: wgpu::TextureAspect::All,
                            ..Default::default()
                        },
                    )),
                },
            ],
        });

        let data_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjectRenderer Data Bind Group"),
            layout: &self.data_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: all_instance_data_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: all_stencil_data_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: visible_instance_indices_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.atomic_counter.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.draw_command_storage.as_entire_binding(),
                },
            ],
        });

        // already checked that instances is not empty
        queue.write_buffer(
            &all_instance_data_buffer,
            0,
            bytemuck::cast_slice(instances),
        );

        if !stencils.is_empty() {
            queue.write_buffer(&all_stencil_data_buffer, 0, bytemuck::cast_slice(stencils));
        } else {
            let default_stencil = StencilData {
                viewport_position: nalgebra::Matrix4::identity(),
                viewport_position_inverse_exists: 1,
                _padding1: [0; 3],
                viewport_position_inverse: nalgebra::Matrix4::identity(),
                atlas_page: 0,
                _padding2: 0,
                in_atlas_offset: [0.0, 0.0],
                in_atlas_size: [0.0, 0.0],
                _padding3: [0; 2],
            };
            queue.write_buffer(
                &all_stencil_data_buffer,
                0,
                bytemuck::bytes_of(&default_stencil),
            );
        }

        queue.write_buffer(&self.atomic_counter, 0, bytemuck::cast_slice(&[0u32]));

        let mut command_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ObjectRenderer: Command Encoder"),
        });
        trace!("CoreRenderer::render: command encoder created");

        let normalize_matrix = make_normalize_matrix(destination_size);
        let cull_pc = CullingPushConstants {
            normalize_matrix,
            instance_count: instances.len() as u32,
            _pad: [0; 3],
        };
        let texture_atlas_size = texture_atlas.size();
        let stencil_atlas_size = stencil_atlas.size();
        let render_pc = RenderPushConstants {
            normalize_matrix,
            texture_atlas_half_texel: [
                0.5 / texture_atlas_size.width as f32,
                0.5 / texture_atlas_size.height as f32,
            ],
            stencil_atlas_half_texel: [
                0.5 / stencil_atlas_size.width as f32,
                0.5 / stencil_atlas_size.height as f32,
            ],
        };

        // culling compute pass
        {
            let mut culling_pass =
                command_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("ObjectRenderer: Culling Pass"),
                    timestamp_writes: None,
                });
            culling_pass.set_pipeline(&self.culling_pipeline);
            culling_pass.set_bind_group(0, &data_bind_group, &[]);
            culling_pass.set_immediates(0, bytemuck::bytes_of(&cull_pc));
            culling_pass.dispatch_workgroups(
                (instances.len() as u32).div_ceil(COMPUTE_WORKGROUP_SIZE),
                1,
                1,
            );
        }
        trace!("CoreRenderer::render: culling pass dispatched");

        // command encoding pass
        {
            let mut command_pass =
                command_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("ObjectRenderer: Command Pass"),
                    timestamp_writes: None,
                });
            command_pass.set_pipeline(&self.command_pipeline);
            command_pass.set_bind_group(0, &data_bind_group, &[]);
            command_pass.dispatch_workgroups(1, 1, 1);
        }
        trace!("CoreRenderer::render: command pass dispatched");

        command_encoder.copy_buffer_to_buffer(
            &self.draw_command_storage,
            0,
            &self.draw_command,
            0,
            std::mem::size_of::<wgpu::util::DrawIndirectArgs>() as u64,
        );

        // render pass
        {
            let mut render_pass = command_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ObjectRenderer: Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: destination_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(load_color),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });

            render_pass.set_pipeline(render_pipeline.as_ref());
            render_pass.set_bind_group(0, &texture_bind_group, &[]);
            render_pass.set_bind_group(1, &data_bind_group, &[]);
            render_pass.set_immediates(0, bytemuck::bytes_of(&render_pc));
            render_pass.draw_indirect(&self.draw_command, 0);
        }
        trace!("CoreRenderer::render: render pass completed");

        queue.submit(std::iter::once(command_encoder.finish()));
        trace!("CoreRenderer::render: commands submitted");

        Ok(())
    }
}

/// Flatten several `(render tree, window-space transform)` pairs into a single
/// instance/stencil array. Each root is walked with its own transform; the
/// shared atlas-id checks in the recursive walk still apply across all roots.
fn create_instance_and_stencil_data_flat(
    items: &[(Arc<RenderNode>, nalgebra::Matrix4<f32>)],
    texture_format: wgpu::TextureFormat,
    stencil_format: wgpu::TextureFormat,
) -> Result<(Vec<InstanceData>, Vec<StencilData>), TextureValidationError> {
    let mut instances = Vec::new();
    let mut stencils = Vec::new();

    let mut texture_atlas_id = None;
    let mut stencil_atlas_id = None;

    for (node, transform) in items {
        create_instance_and_stencil_data_recursive(
            texture_format,
            stencil_format,
            node,
            *transform,
            &mut instances,
            &mut stencils,
            &mut texture_atlas_id,
            &mut stencil_atlas_id,
            0,
        )?;
    }

    Ok((instances, stencils))
}

fn create_instance_and_stencil_data(
    objects: &RenderNode,
    texture_format: wgpu::TextureFormat,
    stencil_format: wgpu::TextureFormat,
) -> Result<(Vec<InstanceData>, Vec<StencilData>), TextureValidationError> {
    trace!("CoreRenderer::create_instance_and_stencil_data: start");
    let mut instances = Vec::new();
    let mut stencils = Vec::new();

    let mut texture_atlas_id = None;
    let mut stencil_atlas_id = None;

    create_instance_and_stencil_data_recursive(
        texture_format,
        stencil_format,
        objects,
        nalgebra::Matrix4::identity(),
        &mut instances,
        &mut stencils,
        &mut texture_atlas_id,
        &mut stencil_atlas_id,
        0,
    )?;

    trace!(
        "CoreRenderer::create_instance_and_stencil_data: completed with {} instances and {} stencils",
        instances.len(),
        stencils.len()
    );
    Ok((instances, stencils))
}

#[allow(clippy::too_many_arguments)]
fn create_instance_and_stencil_data_recursive(
    texture_format: wgpu::TextureFormat,
    stencil_format: wgpu::TextureFormat,
    object: &RenderNode,
    transform: nalgebra::Matrix4<f32>,
    instances: &mut Vec<InstanceData>,
    stencils: &mut Vec<StencilData>,
    texture_atlas_id: &mut Option<texture_atlas::TextureAtlasId>,
    stencil_atlas_id: &mut Option<texture_atlas::TextureAtlasId>,
    // the index + 1 of the current stencil in the stencils vector.
    // 0 if no stencil is used.
    mut current_stencil: u32,
) -> Result<(), TextureValidationError> {
    if let Some((stencil, stencil_position)) = &object.stencil() {
        if stencil.format() != stencil_format {
            warn!("CoreRenderer: stencil format mismatch");
            return Err(TextureValidationError::FormatMismatch);
        }

        let atlas_id = stencil_atlas_id.get_or_insert_with(|| stencil.atlas_id());

        if atlas_id != &stencil.atlas_id() {
            warn!("CoreRenderer: stencil atlas id mismatch");
            return Err(TextureValidationError::AtlasIdMismatch);
        }

        let (page, position_in_atlas) = stencil.position_in_atlas()?;

        let stencil_position = transform * stencil_position;
        let (inverse_exists, stencil_position_inverse) = stencil_position
            .try_inverse()
            .map(|m| (true, m))
            .unwrap_or_else(|| (false, nalgebra::Matrix4::identity()));

        stencils.push(StencilData {
            viewport_position: stencil_position,
            viewport_position_inverse_exists: if inverse_exists { 1 } else { 0 },
            viewport_position_inverse: stencil_position_inverse,
            atlas_page: page,
            in_atlas_offset: [position_in_atlas.min.x, position_in_atlas.min.y],
            in_atlas_size: [position_in_atlas.width(), position_in_atlas.height()],
            _padding1: [0; 3],
            _padding2: 0,
            _padding3: [0; 2],
        });

        current_stencil = stencils.len() as u32;
    }

    if let Some((texture, texture_position)) = &object.texture() {
        if texture.format() != texture_format {
            warn!("CoreRenderer: texture format mismatch");
            return Err(TextureValidationError::FormatMismatch);
        }

        let atlas_id = texture_atlas_id.get_or_insert_with(|| texture.atlas_id());

        if atlas_id != &texture.atlas_id() {
            warn!("CoreRenderer: texture atlas id mismatch");
            return Err(TextureValidationError::AtlasIdMismatch);
        }

        let (page, position_in_atlas) = texture.position_in_atlas()?;

        instances.push(InstanceData {
            viewport_position: transform * texture_position,
            atlas_page: page,
            in_atlas_offset: [position_in_atlas.min.x, position_in_atlas.min.y],
            in_atlas_size: [position_in_atlas.width(), position_in_atlas.height()],
            stencil_index: current_stencil,
            _padding1: 0,
            _padding2: 0,
        });
    }

    for (child, child_transform) in object.child_elements() {
        create_instance_and_stencil_data_recursive(
            texture_format,
            stencil_format,
            child,
            transform * child_transform,
            instances,
            stencils,
            texture_atlas_id,
            stencil_atlas_id,
            current_stencil,
        )?;
    }

    Ok(())
}

#[derive(Error, Debug)]
pub enum TextureValidationError {
    #[error("texture format mismatch")]
    FormatMismatch,
    #[error("texture atlas id mismatch")]
    AtlasIdMismatch,
    #[error("texture atlas error: {0}")]
    AtlasError(#[from] RegionError),
}

#[rustfmt::skip]
fn make_normalize_matrix(destination_size: [f32; 2]) -> nalgebra::Matrix4<f32> {
    // Map pixel coordinates [0..width] x [0..height] into clip space [-1..1] x [-1..1],
    // flipping the Y axis so that Y increases downward in pixel space maps to clip Y decreasing.
    nalgebra::Matrix4::new(
        2.0 / destination_size[0], 0.0, 0.0, -1.0,
        0.0, -2.0 / destination_size[1], 0.0, 1.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    )
}
