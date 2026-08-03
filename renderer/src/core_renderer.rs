use bytemuck::Zeroable;
use log::{debug, trace, warn};
use std::sync::Arc;

use crate::render_node::RenderNode;
use gpu_utils::texture_atlas;
use texture_atlas::RegionError;
use thiserror::Error;

mod stages;
use stages::{
    BlellochPrefixSumStage, CommandStage, ComputeStage, ScatterStage,
    VisibilityStage,
};

const WGSL_RENDER: &str = include_str!("core_renderer/renderer_render.wgsl");

const PIPELINE_CACHE_SIZE: u64 = 3;

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
/// - `alpha`: draw-time opacity multiplied into the sampled colour.
/// - `mask_offset` / `mask_count`: the half-open range `mask_indices[offset ..
///   offset + count]`, each entry an index into the mask array. The instance's
///   coverage is the **product** of every mask in that range, so `count == 0`
///   means "unmasked". See [`MaskData`].
///
/// NOTE: Keep Rust-side layout (#[repr(C)] + bytemuck) compatible with the WGSL
/// `InstanceData` struct (field order, types, and padding). When changing fields,
/// update both Rust and WGSL declarations simultaneously.
struct InstanceData {
    /// transform vertex: {[0, 0], [0, 1], [1, 1], [1, 0]} to where the texture should be rendered
    viewport_position: nalgebra::Matrix4<f32>,
    atlas_page: u32,
    /// draw-time opacity, multiplied into all four sampled channels.
    alpha: f32,
    /// [x, y] (normalized UVs expected)
    in_atlas_offset: [f32; 2],
    /// [width, height] (normalized size expected)
    in_atlas_size: [f32; 2],
    /// start of this instance's mask chain inside `mask_indices`.
    mask_offset: u32,
    /// number of masks in the chain. 0 means unmasked.
    mask_count: u32,
}

/// A `mat3x3<f32>` in WGSL's storage layout: three columns, each padded to 16
/// bytes. `nalgebra::Matrix3` is tightly packed (36 bytes) and therefore *not*
/// layout-compatible; go through [`mat3_columns`].
type Mat3Columns = [[f32; 4]; 3];

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
/// MaskData describes one element of an instance's mask chain: a quad whose
/// coverage texture attenuates whatever the instance draws.
///
/// A mask is transformed exactly like a texture — same unit quad, same
/// `viewport_position` — but it is applied in **screen space**: the fragment
/// shader maps its own position back into the mask's local unit square and
/// samples there. That is what makes a mask behave like a hole in a box, or a
/// portal, rather than something baked into the instance's own surface, and it
/// stays correct when the mask and the instance are not coplanar.
///
/// Semantics:
/// - `viewport_position`: transform mapping the unit quad into UI space (before
///   normalization). Used for culling, and as the source of `mask_from_screen`.
/// - `mask_from_screen`: the inverse of that transform's **planar homography**,
///   mapping a screen position back to the mask's local unit square. A mask's
///   local coordinates are `(u, v, 0, 1)`, so only rows and columns `{0, 1, 3}`
///   of the 4x4 ever contribute; restricting to that 3x3 and inverting is exact
///   for any affine *or* projective transform, whereas inverting the full 4x4
///   would presuppose that the fragment lies on the mask's plane.
/// - `inverse_exists`: 0 if the transform is degenerate. Such a mask is skipped
///   (treated as fully transparent to whatever it would mask), matching how
///   culling treats it.
/// - `kind`: reserved for analytic mask shapes (SDF rect / rounded rect /
///   ellipse). Only 0 (sample the coverage texture) is implemented.
/// - `atlas_page`: index of the mask atlas page (texture array layer).
/// - `in_atlas_offset` / `in_atlas_size`: offset and size of the coverage image
///   inside the atlas page. Expected to be NORMALIZED UVs (0.0 .. 1.0). If the
///   atlas returns pixel coordinates, the host MUST normalize them before
///   uploading to GPU.
///
/// NOTE: Maintain identical memory layout between this Rust struct and the WGSL
/// `MaskData` declaration (including explicit padding fields). Update both
/// definitions when changing sizes/types.
struct MaskData {
    /// transform vertex: {[0, 0], [0, 1], [1, 1], [1, 0]} to where the mask should be rendered
    viewport_position: nalgebra::Matrix4<f32>,
    /// screen -> mask-local homography; see the struct docs.
    mask_from_screen: Mat3Columns,
    /// 0 = sample the coverage texture. Other values are reserved.
    kind: u32,
    /// 0 if `viewport_position`'s planar homography is not invertible.
    inverse_exists: u32,
    atlas_page: u32,
    _padding1: u32,
    /// [x, y] (normalized UVs expected)
    in_atlas_offset: [f32; 2],
    /// [width, height] (normalized size expected)
    in_atlas_size: [f32; 2],
}

/// Mask kind: sample the coverage texture in the mask atlas.
const MASK_KIND_COVERAGE: u32 = 0;

const _: () = {
    assert!(std::mem::size_of::<InstanceData>() == 96);
    assert!(std::mem::size_of::<MaskData>() == 144);
    assert!(std::mem::size_of::<FrameParams>() == 96);
};

/// The planar homography of a unit-quad transform: the restriction of `m` to
/// rows and columns `{0, 1, 3}`. See [`MaskData::mask_from_screen`].
#[rustfmt::skip]
fn planar_homography(m: &nalgebra::Matrix4<f32>) -> nalgebra::Matrix3<f32> {
    nalgebra::Matrix3::new(
        m[(0, 0)], m[(0, 1)], m[(0, 3)],
        m[(1, 0)], m[(1, 1)], m[(1, 3)],
        m[(3, 0)], m[(3, 1)], m[(3, 3)],
    )
}

/// Re-lay a `Matrix3` into WGSL's padded column layout.
fn mat3_columns(m: &nalgebra::Matrix3<f32>) -> Mat3Columns {
    [
        [m[(0, 0)], m[(1, 0)], m[(2, 0)], 0.0],
        [m[(0, 1)], m[(1, 1)], m[(2, 1)], 0.0],
        [m[(0, 2)], m[(1, 2)], m[(2, 2)], 0.0],
    ]
}

/// The per-frame parameter block, shared verbatim by *every* pipeline in the
/// core renderer — the render pass and all four compaction compute stages.
///
/// One block for all of them, rather than a tailored struct per stage, because
/// the web build cannot use immediates at all (WebGPU has no push constants)
/// and has to carry these in a uniform buffer instead. A single shared layout
/// means that path is one buffer, one bind group and one write per frame; a
/// per-stage layout would multiply all three. Each shader declares an identical
/// `struct Pc` and simply ignores the fields it does not need.
///
/// The layout is valid in both the immediate and uniform address spaces: 96
/// bytes, 16-byte aligned, no member straddling a 16-byte boundary. **Pad with
/// scalar `u32`s only** — a `vec3<u32>` has alignment 16 in WGSL, which would
/// silently make the shader-side struct larger than this one.
///
/// `*_half_texel` is half a texel in normalized UV units, per atlas. The render
/// shader uses it to inset the UV clamp bounds, so that sampling at the very
/// edge of an atlas region's usable (non-margin) rectangle can never land
/// exactly on the boundary between the last real texel and the next
/// (zero-initialised margin) texel — bilinear filtering would blend 50/50 with
/// that margin texel there, bleeding the background colour into the edge of
/// every texture/stencil sample. Insetting to the last texel's *centre*
/// instead of its edge guarantees a pure, unblended sample.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct FrameParams {
    pub normalize_matrix: nalgebra::Matrix4<f32>,
    pub instance_count: u32,
    pub _pad0: [u32; 3],
    pub texture_atlas_half_texel: [f32; 2],
    pub stencil_atlas_half_texel: [f32; 2],
}

/// A clip declared by the UI tree: a mask that applies to an entity *and
/// everything nested inside it*.
///
/// Clips form their own small tree, kept as an arena rather than by nesting
/// [`RenderNode`]s, because a frame is handed to the renderer as a flat list of
/// per-entity trees — the ancestor relationship between two entities is simply
/// not expressible in that shape. A `MaskNode` is transformed and sampled
/// exactly like a texture; nothing distinguishes it from an object's own
/// coverage mask once both reach the GPU.
#[derive(Clone, Debug)]
pub struct MaskNode {
    /// Enclosing clip, if any. Always a **smaller index** than this node's own,
    /// so the chain from any node up to its root can be collected by following
    /// parents without a cycle check.
    pub parent: Option<u32>,
    /// Unit quad -> UI space, in the same convention and units as a texture's
    /// position.
    pub transform: nalgebra::Matrix4<f32>,
    /// Coverage image. Single-channel; only the red channel is read.
    pub region: texture_atlas::AtlasRegion,
}

/// One entry of a frame: a render tree, where to put it, and the state that
/// applies to everything it draws.
#[derive(Clone)]
pub struct FlatItem {
    pub node: Arc<RenderNode>,
    /// Node-local space -> UI space.
    pub transform: nalgebra::Matrix4<f32>,
    /// The innermost clip this item sits inside, as an index into the `masks`
    /// slice passed alongside. The clips it inherits are that node's ancestors.
    pub clip: Option<u32>,
    /// Draw-time opacity applied to everything the tree draws.
    pub alpha: f32,
}

impl FlatItem {
    /// An unclipped, fully opaque item — the common case.
    pub fn new(node: Arc<RenderNode>, transform: nalgebra::Matrix4<f32>) -> Self {
        Self {
            node,
            transform,
            clip: None,
            alpha: 1.0,
        }
    }

    pub fn with_clip(mut self, clip: Option<u32>) -> Self {
        self.clip = clip;
        self
    }

    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha;
        self
    }
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
        items: &[FlatItem],
        clips: &[MaskNode],
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
            clips,
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
    /// Read-only view of the same buffers, for the render pass. See
    /// [`create_render_data_bind_group_layout`].
    render_data_bind_group_layout: wgpu::BindGroupLayout,

    // Pipeline Layouts
    render_pipeline_layout: wgpu::PipelineLayout,
    render_pipeline_shader_module: wgpu::ShaderModule,

    // Compute stages of the compaction pipeline (visibility -> prefix sum ->
    // scatter -> command), run in order. Each stage is an interchangeable
    // `ComputeStage` implementation; see `stages.rs`.
    compaction_stages: Vec<Box<dyn ComputeStage>>,

    // Pipelines
    render_pipeline:
        crate::pipeline_cache::PipelineCache<wgpu::TextureFormat, Arc<wgpu::RenderPipeline>>, // key: surface format

    // reusable buffers
    atomic_counter: wgpu::Buffer,
    draw_command: wgpu::Buffer,
    draw_command_storage: wgpu::Buffer,
}

/// Bind group layout for the data buffers, as the **compaction compute stages**
/// see them: bindings 2-6 are writable, and every binding is `COMPUTE`-only
/// (see `stages.rs`).
///
/// The render pass uses [`create_render_data_bind_group_layout`] instead, over
/// the same buffers. Keeping the two apart is what lets the renderer drop the
/// `VERTEX_WRITABLE_STORAGE` requirement: a writable storage buffer visible to
/// the vertex stage demands that feature, which WebGPU does not expose. Note
/// the failure mode if `VERTEX` ever creeps back into a writable entry here —
/// the layout is rejected, every pipeline built on it is silently invalid, and
/// the stages appear to run but produce all-zero output.
fn create_data_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ObjectRenderer Data Bind Group Layout"),
        entries: &[
            // All Instances Buffer
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // All Masks Buffer. Masks are resolved per fragment, from the
            // fragment's own screen position; the vertex stage never reads one.
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
            // Visible Instances Buffer (compacted, submission order)
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // Atomic Counter (visible instance count)
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
            // command buffer (indirect draw args)
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
            // Visibility flags (0/1 per instance; written by the visibility
            // stage, consumed by the prefix-sum and scatter stages)
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // Scan offsets (exclusive prefix sum of the visibility flags;
            // written by the prefix-sum stage, consumed by the scatter stage)
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // Mask chains: the flat backing store every instance's
            // `mask_offset`/`mask_count` range points into.
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Bind group layout for the data buffers, as the **render pass** sees them:
/// the same buffers as [`create_data_bind_group_layout`], but every binding
/// read-only, and only the four the render shader actually declares.
///
/// This exists so the renderer does not require `VERTEX_WRITABLE_STORAGE`.
/// `visible_instances` (binding 2) is written by the compaction stages but only
/// *read* by the vertex shader; binding it writable there would demand a
/// feature WebGPU does not expose, for no benefit. A bind group layout need not
/// be contiguous, so the compute-only intermediates (3-6) are simply absent.
fn create_render_data_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let read_only_storage = |binding: u32, visibility: wgpu::ShaderStages| {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ObjectRenderer Render Data Bind Group Layout"),
        entries: &[
            // All instances
            read_only_storage(0, wgpu::ShaderStages::VERTEX),
            // All masks — resolved per fragment, from the fragment's own screen
            // position; the vertex stage never reads one.
            read_only_storage(1, wgpu::ShaderStages::FRAGMENT),
            // Visible instances (compacted, submission order)
            read_only_storage(2, wgpu::ShaderStages::VERTEX),
            // Mask chains
            read_only_storage(7, wgpu::ShaderStages::FRAGMENT),
        ],
    })
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

        let data_bind_group_layout = create_data_bind_group_layout(device);
        let render_data_bind_group_layout = create_render_data_bind_group_layout(device);

        let compaction_stages: Vec<Box<dyn ComputeStage>> = vec![
            Box::new(VisibilityStage::new(device, &data_bind_group_layout)),
            // Swap the scan algorithm by constructing a different stage here
            // (e.g. `SingleThreadPrefixSumStage` as the naive reference).
            Box::new(BlellochPrefixSumStage::new(device, &data_bind_group_layout)),
            Box::new(ScatterStage::new(device, &data_bind_group_layout)),
            Box::new(CommandStage::new(device, &data_bind_group_layout)),
        ];

        let (render_pipeline_layout, render_pipeline_shader_module) =
            Self::create_render_pipeline_layout(
                device,
                &texture_bind_group_layout,
                &render_data_bind_group_layout,
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
            render_data_bind_group_layout,
            render_pipeline_layout,
            render_pipeline_shader_module,
            compaction_stages,
            render_pipeline,
            atomic_counter,
            draw_command,
            draw_command_storage,
        }
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
            immediate_size: std::mem::size_of::<FrameParams>() as u32,
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
        let frame = create_frame(render_node, texture_atlas.format(), stencil_atlas.format())?;

        self.render_instances(
            device,
            queue,
            surface_format,
            destination_view,
            destination_size,
            &frame,
            load_color,
            texture_atlas,
            stencil_atlas,
        )
    }

    /// Flat variant of [`render`](Self::render): renders several already
    /// window-space-transformed render trees (paint order) in a single frame.
    /// Used by the M4 render-thread path, which extracts one [`FlatItem`] per
    /// widget entity instead of building a pseudo root.
    ///
    /// `clips` is the frame's clip arena; an item names the innermost clip it
    /// sits inside and inherits that clip's ancestors. See [`MaskNode`].
    #[allow(clippy::too_many_arguments)]
    pub fn render_flat(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        destination_view: &wgpu::TextureView,
        destination_size: [f32; 2],
        items: &[FlatItem],
        clips: &[MaskNode],
        load_color: wgpu::Color,
        texture_atlas: &wgpu::Texture,
        stencil_atlas: &wgpu::Texture,
    ) -> Result<(), TextureValidationError> {
        let frame = create_flat_frame(
            items,
            clips,
            texture_atlas.format(),
            stencil_atlas.format(),
        )?;

        self.render_instances(
            device,
            queue,
            surface_format,
            destination_view,
            destination_size,
            &frame,
            load_color,
            texture_atlas,
            stencil_atlas,
        )
    }

    /// Encode and submit a prepared frame. Shared by [`render`](Self::render)
    /// (single tree) and [`render_flat`](Self::render_flat) (multiple
    /// pre-transformed roots).
    #[allow(clippy::too_many_arguments)]
    fn render_instances(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        destination_view: &wgpu::TextureView,
        destination_size: [f32; 2],
        frame: &FlatFrame,
        load_color: wgpu::Color,
        texture_atlas: &wgpu::Texture,
        stencil_atlas: &wgpu::Texture,
    ) -> Result<(), TextureValidationError> {
        let FlatFrame {
            instances,
            masks,
            mask_indices,
        } = frame;

        trace!(
            "CoreRenderer::render_instances: prepared {} instances, {} masks, {} chain entries",
            instances.len(),
            masks.len(),
            mask_indices.len()
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

        // `.max(1)`: a zero-sized storage binding is invalid, and a frame with
        // no mask at all is the common case.
        let all_mask_data_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Mask Buffer"),
            size: (std::mem::size_of::<MaskData>() * masks.len().max(1)) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mask_indices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Mask Chain Buffer"),
            size: (std::mem::size_of::<u32>() * mask_indices.len().max(1)) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let visible_instance_indices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Visible Instances Buffer"),
            size: (std::mem::size_of::<u32>() * instances.len()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let visibility_flags_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Visibility Flags Buffer"),
            size: (std::mem::size_of::<u32>() * instances.len()) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let scan_offsets_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjectRenderer Scan Offsets Buffer"),
            size: (std::mem::size_of::<u32>() * instances.len()) as u64,
            usage: wgpu::BufferUsages::STORAGE,
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
                    resource: all_mask_data_buffer.as_entire_binding(),
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
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: visibility_flags_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: scan_offsets_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: mask_indices_buffer.as_entire_binding(),
                },
            ],
        });

        // The same buffers again, read-only, for the render pass. Bind groups
        // are rebuilt every frame anyway, so this is one extra cheap object.
        let render_data_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjectRenderer Render Data Bind Group"),
            layout: &self.render_data_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: all_instance_data_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: all_mask_data_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: visible_instance_indices_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: mask_indices_buffer.as_entire_binding(),
                },
            ],
        });

        // already checked that instances is not empty
        queue.write_buffer(
            &all_instance_data_buffer,
            0,
            bytemuck::cast_slice(instances),
        );

        // The `.max(1)`-sized buffers above are padded, never read: an instance
        // with `mask_count == 0` dereferences neither. Zero-fill them anyway so
        // a stale mapping can never be mistaken for a real mask.
        if masks.is_empty() {
            queue.write_buffer(&all_mask_data_buffer, 0, bytemuck::bytes_of(&MaskData::zeroed()));
        } else {
            queue.write_buffer(&all_mask_data_buffer, 0, bytemuck::cast_slice(masks));
        }

        if mask_indices.is_empty() {
            queue.write_buffer(&mask_indices_buffer, 0, bytemuck::bytes_of(&0u32));
        } else {
            queue.write_buffer(&mask_indices_buffer, 0, bytemuck::cast_slice(mask_indices));
        }

        queue.write_buffer(&self.atomic_counter, 0, bytemuck::cast_slice(&[0u32]));

        let mut command_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ObjectRenderer: Command Encoder"),
        });
        trace!("CoreRenderer::render: command encoder created");

        let texture_atlas_size = texture_atlas.size();
        let stencil_atlas_size = stencil_atlas.size();
        // One block for the whole frame: the compaction stages and the render
        // pass all read from this same struct.
        let frame_params = FrameParams {
            normalize_matrix: make_normalize_matrix(destination_size),
            instance_count: instances.len() as u32,
            _pad0: [0; 3],
            texture_atlas_half_texel: [
                0.5 / texture_atlas_size.width as f32,
                0.5 / texture_atlas_size.height as f32,
            ],
            stencil_atlas_half_texel: [
                0.5 / stencil_atlas_size.width as f32,
                0.5 / stencil_atlas_size.height as f32,
            ],
        };

        // Compaction pipeline: visibility -> prefix sum -> scatter -> command.
        // The stages together produce an order-preserving compaction of the
        // instance indices in `visible_instances` plus the indirect draw args.
        for stage in &self.compaction_stages {
            stage.encode(&mut command_encoder, &data_bind_group, &frame_params);
        }
        trace!("CoreRenderer::render: compaction stages dispatched");

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
            render_pass.set_bind_group(1, &render_data_bind_group, &[]);
            render_pass.set_immediates(0, bytemuck::bytes_of(&frame_params));
            render_pass.draw_indirect(&self.draw_command, 0);
        }
        trace!("CoreRenderer::render: render pass completed");

        queue.submit(std::iter::once(command_encoder.finish()));
        trace!("CoreRenderer::render: commands submitted");

        Ok(())
    }
}

/// The GPU-side arrays one frame flattens down to.
///
/// Everything here is a plain `Vec`, uploaded verbatim: the walk below is the
/// only place that knows about the pointer-linked [`RenderNode`] shape, so a
/// future array-of-structs `RenderNode` changes the input to [`Flattener::walk`]
/// and nothing else.
struct FlatFrame {
    instances: Vec<InstanceData>,
    masks: Vec<MaskData>,
    /// Backing storage for every instance's mask chain. An instance references
    /// `mask_indices[mask_offset .. mask_offset + mask_count]`.
    mask_indices: Vec<u32>,
}

/// Walks render trees into a [`FlatFrame`], validating that every region really
/// comes from the atlas the caller is about to bind.
struct Flattener {
    texture_format: wgpu::TextureFormat,
    mask_format: wgpu::TextureFormat,
    out: FlatFrame,
    texture_atlas_id: Option<texture_atlas::TextureAtlasId>,
    mask_atlas_id: Option<texture_atlas::TextureAtlasId>,
    /// The chain most recently appended to `mask_indices`. Siblings under one
    /// clip are painted consecutively and share a chain, so this one-entry
    /// cache collapses the common run without hashing anything.
    last_chain: Option<(u32, u32)>,
    /// Longest chain emitted this frame, for the debug log.
    max_chain_len: usize,
    /// Opacity of the item currently being walked; constant for its whole tree.
    alpha: f32,
}

impl Flattener {
    fn new(texture_format: wgpu::TextureFormat, mask_format: wgpu::TextureFormat) -> Self {
        Self {
            texture_format,
            mask_format,
            out: FlatFrame {
                instances: Vec::new(),
                masks: Vec::new(),
                mask_indices: Vec::new(),
            },
            texture_atlas_id: None,
            mask_atlas_id: None,
            last_chain: None,
            max_chain_len: 0,
            alpha: 1.0,
        }
    }

    /// Register the frame's clips, returning the resolved root-to-leaf mask
    /// chain for each one.
    ///
    /// Relies on a parent always preceding its child, which [`MaskNode::parent`]
    /// guarantees, so one forward pass suffices and no cycle can form.
    fn register_clips(
        &mut self,
        clips: &[MaskNode],
    ) -> Result<Vec<Vec<u32>>, TextureValidationError> {
        let mut chains: Vec<Vec<u32>> = Vec::with_capacity(clips.len());
        for (i, clip) in clips.iter().enumerate() {
            let index = self.push_mask(&clip.region, clip.transform)?;
            let mut chain = match clip.parent {
                Some(parent) if (parent as usize) < i => chains[parent as usize].clone(),
                Some(parent) => {
                    warn!(
                        "CoreRenderer: clip {i} references parent {parent}, which is not \
                         an earlier clip; treating it as a root"
                    );
                    Vec::new()
                }
                None => Vec::new(),
            };
            chain.push(index);
            chains.push(chain);
        }
        Ok(chains)
    }

    /// Record a mask quad and return its index.
    fn push_mask(
        &mut self,
        region: &texture_atlas::AtlasRegion,
        position: nalgebra::Matrix4<f32>,
    ) -> Result<u32, TextureValidationError> {
        if region.format() != self.mask_format {
            warn!("CoreRenderer: mask format mismatch");
            return Err(TextureValidationError::FormatMismatch);
        }
        let atlas_id = self.mask_atlas_id.get_or_insert_with(|| region.atlas_id());
        if atlas_id != &region.atlas_id() {
            warn!("CoreRenderer: mask atlas id mismatch");
            return Err(TextureValidationError::AtlasIdMismatch);
        }
        let (page, position_in_atlas) = region.position_in_atlas()?;

        // A degenerate transform has no usable inverse; the shaders treat such
        // a mask as absent rather than as fully occluding, so a widget with a
        // collapsed mask still draws instead of silently vanishing.
        let inverse = planar_homography(&position).try_inverse();

        self.out.masks.push(MaskData {
            viewport_position: position,
            mask_from_screen: mat3_columns(&inverse.unwrap_or_else(nalgebra::Matrix3::identity)),
            kind: MASK_KIND_COVERAGE,
            inverse_exists: u32::from(inverse.is_some()),
            atlas_page: page,
            _padding1: 0,
            in_atlas_offset: [position_in_atlas.min.x, position_in_atlas.min.y],
            in_atlas_size: [position_in_atlas.width(), position_in_atlas.height()],
        });
        Ok(self.out.masks.len() as u32 - 1)
    }

    /// Append a chain to `mask_indices` and return its `(offset, count)`.
    fn push_chain(&mut self, chain: &[u32]) -> (u32, u32) {
        self.max_chain_len = self.max_chain_len.max(chain.len());
        if chain.is_empty() {
            return (0, 0);
        }
        if let Some((offset, count)) = self.last_chain
            && count as usize == chain.len()
            && self.out.mask_indices[offset as usize..][..chain.len()] == *chain
        {
            return (offset, count);
        }
        let offset = self.out.mask_indices.len() as u32;
        self.out.mask_indices.extend_from_slice(chain);
        let entry = (offset, chain.len() as u32);
        self.last_chain = Some(entry);
        entry
    }

    /// Walk one tree. `inherited` is the chain of clips enclosing it.
    fn walk(
        &mut self,
        object: &RenderNode,
        transform: nalgebra::Matrix4<f32>,
        inherited: &[u32],
    ) -> Result<(), TextureValidationError> {
        // A node's own mask covers that node alone (see `RenderNode::with_stencil`);
        // only the enclosing clips carry down to children. Appending it here
        // rather than in the child recursion is what expresses that.
        let mut own_chain;
        let chain: &[u32] = match object.stencil() {
            Some((mask, mask_position)) => {
                let index = self.push_mask(mask, transform * mask_position)?;
                own_chain = Vec::with_capacity(inherited.len() + 1);
                own_chain.extend_from_slice(inherited);
                own_chain.push(index);
                &own_chain
            }
            None => inherited,
        };

        if let Some((texture, texture_position)) = &object.texture() {
            if texture.format() != self.texture_format {
                warn!("CoreRenderer: texture format mismatch");
                return Err(TextureValidationError::FormatMismatch);
            }
            let atlas_id = self
                .texture_atlas_id
                .get_or_insert_with(|| texture.atlas_id());
            if atlas_id != &texture.atlas_id() {
                warn!("CoreRenderer: texture atlas id mismatch");
                return Err(TextureValidationError::AtlasIdMismatch);
            }
            let (page, position_in_atlas) = texture.position_in_atlas()?;
            let (mask_offset, mask_count) = self.push_chain(chain);

            self.out.instances.push(InstanceData {
                viewport_position: transform * texture_position,
                atlas_page: page,
                alpha: self.alpha,
                in_atlas_offset: [position_in_atlas.min.x, position_in_atlas.min.y],
                in_atlas_size: [position_in_atlas.width(), position_in_atlas.height()],
                mask_offset,
                mask_count,
            });
        }

        for (child, child_transform) in object.child_elements() {
            self.walk(child, transform * child_transform, chain)?;
        }

        Ok(())
    }
}

/// Flatten a frame's items and clips into the GPU-side arrays. Each item is
/// walked with its own transform; the shared atlas-id checks apply across all
/// of them.
fn create_flat_frame(
    items: &[FlatItem],
    clips: &[MaskNode],
    texture_format: wgpu::TextureFormat,
    mask_format: wgpu::TextureFormat,
) -> Result<FlatFrame, TextureValidationError> {
    let mut flattener = Flattener::new(texture_format, mask_format);
    let clip_chains = flattener.register_clips(clips)?;

    const NO_CLIP: &[u32] = &[];
    for item in items {
        let inherited = match item.clip {
            Some(clip) => clip_chains.get(clip as usize).map_or_else(
                || {
                    warn!("CoreRenderer: item references clip {clip}, which does not exist");
                    NO_CLIP
                },
                Vec::as_slice,
            ),
            None => NO_CLIP,
        };
        flattener.alpha = item.alpha;
        flattener.walk(&item.node, item.transform, inherited)?;
    }

    debug!(
        "CoreRenderer: flattened {} instances, {} masks, {} chain entries, deepest chain {}",
        flattener.out.instances.len(),
        flattener.out.masks.len(),
        flattener.out.mask_indices.len(),
        flattener.max_chain_len,
    );
    Ok(flattener.out)
}

fn create_frame(
    objects: &RenderNode,
    texture_format: wgpu::TextureFormat,
    mask_format: wgpu::TextureFormat,
) -> Result<FlatFrame, TextureValidationError> {
    let mut flattener = Flattener::new(texture_format, mask_format);
    flattener.walk(objects, nalgebra::Matrix4::identity(), &[])?;
    Ok(flattener.out)
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
