//! Swappable compute stages of the culling / stream-compaction pipeline.
//!
//! `CoreRenderer` turns the ordered `all_instances` array into the compacted
//! `visible_instances` index list with a sequence of compute stages:
//!
//! 1. [`VisibilityStage`] — per-instance visibility test, emits 0/1 flags.
//! 2. a prefix-sum stage — exclusive scan of the flags into per-instance
//!    compaction offsets + the visible total ([`BlellochPrefixSumStage`] by
//!    default, [`SingleThreadPrefixSumStage`] as the naive reference).
//! 3. [`ScatterStage`] — writes each visible instance index to its scanned
//!    offset, preserving submission (paint) order by construction.
//! 4. [`CommandStage`] — converts the visible count into indirect draw args.
//!
//! Every stage implements [`ComputeStage`] and only communicates with its
//! neighbours through the shared data bind group (see the bind group layout in
//! `core_renderer.rs`), so swapping an algorithm — e.g. replacing the scan
//! implementation — only means constructing a different stage in
//! `CoreRendererInner::new`. A stage may record any number of dispatches (the
//! Blelloch scan records three).

use log::warn;

/// Threads per workgroup for the simple one-thread-per-instance stages
/// (visibility test, scatter). Must match `@workgroup_size` in
/// `renderer_cull.wgsl` and `renderer_scatter.wgsl`.
pub(crate) const COMPUTE_WORKGROUP_SIZE: u32 = 64;

const WGSL_CULL: &str = include_str!("renderer_cull.wgsl");
const WGSL_PREFIX_SUM_BLELLOCH: &str = include_str!("renderer_prefix_sum_blelloch.wgsl");
const WGSL_PREFIX_SUM_SINGLE_THREAD: &str =
    include_str!("renderer_prefix_sum_single_thread.wgsl");
const WGSL_SCATTER: &str = include_str!("renderer_scatter.wgsl");
const WGSL_COMMAND: &str = include_str!("renderer_command.wgsl");

/// Per-frame parameters shared by all compute stages. Each stage picks what it
/// needs and packs its own immediates from these.
pub(crate) struct StageParams {
    pub normalize_matrix: nalgebra::Matrix4<f32>,
    pub instance_count: u32,
}

/// One compute stage of the compaction pipeline. Implementations own their
/// pipeline(s) and any internal resources, and record their dispatches into
/// the frame's command encoder. Stages run in the order they are stored in
/// `CoreRendererInner::compaction_stages`; wgpu inserts the storage-buffer
/// barriers between dispatches.
pub(crate) trait ComputeStage: Send + Sync {
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        data_bind_group: &wgpu::BindGroup,
        params: &StageParams,
    );
}

/// Immediates for [`VisibilityStage`]. Layout must match `Pc` in
/// `renderer_cull.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct VisibilityPushConstants {
    normalize_matrix: nalgebra::Matrix4<f32>,
    instance_count: u32,
    _pad: [u32; 3],
}

/// Immediates for the prefix-sum and scatter stages. Layout must match `Pc`
/// in the corresponding WGSL files.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceCountPushConstants {
    instance_count: u32,
}

fn create_compute_pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    entry_point: &str,
    bind_group_layouts: &[Option<&wgpu::BindGroupLayout>],
    immediate_size: u32,
) -> wgpu::ComputePipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&format!("{label} Layout")),
        bind_group_layouts,
        immediate_size,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        module,
        entry_point: Some(entry_point),
        compilation_options: Default::default(),
        cache: None,
    })
}

// ---------------------------------------------------------------------------
// Visibility stage
// ---------------------------------------------------------------------------

/// Per-instance visibility test (`renderer_cull.wgsl`): writes a 0/1 flag per
/// instance into `visibility_flags`. Currently every instance is flagged
/// visible (the geometric test has a known bug and is bypassed in the shader;
/// see the TODO there).
pub(crate) struct VisibilityStage {
    pipeline: wgpu::ComputePipeline,
}

impl VisibilityStage {
    pub fn new(device: &wgpu::Device, data_bind_group_layout: &wgpu::BindGroupLayout) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Visibility Shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL_CULL.into()),
        });
        let pipeline = create_compute_pipeline(
            device,
            "Visibility Pipeline",
            &module,
            "culling_main",
            &[Some(data_bind_group_layout)],
            std::mem::size_of::<VisibilityPushConstants>() as u32,
        );
        Self { pipeline }
    }
}

impl ComputeStage for VisibilityStage {
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        data_bind_group: &wgpu::BindGroup,
        params: &StageParams,
    ) {
        let pc = VisibilityPushConstants {
            normalize_matrix: params.normalize_matrix,
            instance_count: params.instance_count,
            _pad: [0; 3],
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ObjectRenderer: Visibility Pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, data_bind_group, &[]);
        pass.set_immediates(0, bytemuck::bytes_of(&pc));
        pass.dispatch_workgroups(
            params.instance_count.div_ceil(COMPUTE_WORKGROUP_SIZE),
            1,
            1,
        );
    }
}

// ---------------------------------------------------------------------------
// Prefix-sum stages
// ---------------------------------------------------------------------------

/// Naive reference prefix-sum stage: one GPU thread scans the whole flag array
/// sequentially (`renderer_prefix_sum_single_thread.wgsl`). Trivially correct
/// and unbounded, but serial; kept as the drop-in fallback / bisection tool
/// for [`BlellochPrefixSumStage`].
pub(crate) struct SingleThreadPrefixSumStage {
    pipeline: wgpu::ComputePipeline,
}

impl SingleThreadPrefixSumStage {
    pub fn new(device: &wgpu::Device, data_bind_group_layout: &wgpu::BindGroupLayout) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Prefix Sum Shader (single thread)"),
            source: wgpu::ShaderSource::Wgsl(WGSL_PREFIX_SUM_SINGLE_THREAD.into()),
        });
        let pipeline = create_compute_pipeline(
            device,
            "Prefix Sum Pipeline (single thread)",
            &module,
            "prefix_sum_main",
            &[Some(data_bind_group_layout)],
            std::mem::size_of::<InstanceCountPushConstants>() as u32,
        );
        Self { pipeline }
    }
}

impl ComputeStage for SingleThreadPrefixSumStage {
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        data_bind_group: &wgpu::BindGroup,
        params: &StageParams,
    ) {
        let pc = InstanceCountPushConstants {
            instance_count: params.instance_count,
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ObjectRenderer: Prefix Sum Pass (single thread)"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, data_bind_group, &[]);
        pass.set_immediates(0, bytemuck::bytes_of(&pc));
        pass.dispatch_workgroups(1, 1, 1);
    }
}

/// Elements scanned per workgroup by the Blelloch scan (2 per thread). Must
/// match `BLOCK_ELEMENTS` in `renderer_prefix_sum_blelloch.wgsl`.
const BLELLOCH_BLOCK_ELEMENTS: u32 = 512;
/// Capacity of the block-sums pass (one workgroup scans all block sums). Must
/// match `MAX_BLOCKS` in `renderer_prefix_sum_blelloch.wgsl`.
const BLELLOCH_MAX_BLOCKS: u32 = 512;
/// Maximum instance count the two-level scan supports.
const BLELLOCH_MAX_ELEMENTS: u32 = BLELLOCH_BLOCK_ELEMENTS * BLELLOCH_MAX_BLOCKS;

/// Default prefix-sum stage: two-level work-efficient Blelloch scan
/// (`renderer_prefix_sum_blelloch.wgsl`, three dispatches). Owns its
/// `block_sums` scratch buffer as a private second bind group so the shared
/// data bind group layout stays algorithm-agnostic. Falls back to the
/// single-thread scan beyond [`BLELLOCH_MAX_ELEMENTS`] instances.
pub(crate) struct BlellochPrefixSumStage {
    scan_blocks_pipeline: wgpu::ComputePipeline,
    scan_block_sums_pipeline: wgpu::ComputePipeline,
    add_block_offsets_pipeline: wgpu::ComputePipeline,
    block_sums_bind_group: wgpu::BindGroup,
    fallback: SingleThreadPrefixSumStage,
}

impl BlellochPrefixSumStage {
    pub fn new(device: &wgpu::Device, data_bind_group_layout: &wgpu::BindGroupLayout) -> Self {
        let block_sums_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Blelloch Block Sums Bind Group Layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let block_sums_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blelloch Block Sums Buffer"),
            size: (std::mem::size_of::<u32>() as u64) * BLELLOCH_MAX_BLOCKS as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let block_sums_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blelloch Block Sums Bind Group"),
            layout: &block_sums_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: block_sums_buffer.as_entire_binding(),
            }],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Prefix Sum Shader (Blelloch)"),
            source: wgpu::ShaderSource::Wgsl(WGSL_PREFIX_SUM_BLELLOCH.into()),
        });
        let layouts = [
            Some(data_bind_group_layout),
            Some(&block_sums_bind_group_layout),
        ];
        let immediate_size = std::mem::size_of::<InstanceCountPushConstants>() as u32;
        let scan_blocks_pipeline = create_compute_pipeline(
            device,
            "Blelloch Scan Blocks Pipeline",
            &module,
            "scan_blocks",
            &layouts,
            immediate_size,
        );
        let scan_block_sums_pipeline = create_compute_pipeline(
            device,
            "Blelloch Scan Block Sums Pipeline",
            &module,
            "scan_block_sums",
            &layouts,
            immediate_size,
        );
        let add_block_offsets_pipeline = create_compute_pipeline(
            device,
            "Blelloch Add Block Offsets Pipeline",
            &module,
            "add_block_offsets",
            &layouts,
            immediate_size,
        );

        Self {
            scan_blocks_pipeline,
            scan_block_sums_pipeline,
            add_block_offsets_pipeline,
            block_sums_bind_group,
            fallback: SingleThreadPrefixSumStage::new(device, data_bind_group_layout),
        }
    }
}

impl ComputeStage for BlellochPrefixSumStage {
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        data_bind_group: &wgpu::BindGroup,
        params: &StageParams,
    ) {
        if params.instance_count > BLELLOCH_MAX_ELEMENTS {
            warn!(
                "BlellochPrefixSumStage: {} instances exceed the two-level scan capacity ({}); \
                 falling back to the single-thread scan",
                params.instance_count, BLELLOCH_MAX_ELEMENTS
            );
            self.fallback.encode(encoder, data_bind_group, params);
            return;
        }

        let pc = InstanceCountPushConstants {
            instance_count: params.instance_count,
        };
        let num_blocks = params.instance_count.div_ceil(BLELLOCH_BLOCK_ELEMENTS);

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ObjectRenderer: Prefix Sum Pass (Blelloch)"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, data_bind_group, &[]);
        pass.set_bind_group(1, &self.block_sums_bind_group, &[]);

        pass.set_pipeline(&self.scan_blocks_pipeline);
        pass.set_immediates(0, bytemuck::bytes_of(&pc));
        pass.dispatch_workgroups(num_blocks, 1, 1);

        pass.set_pipeline(&self.scan_block_sums_pipeline);
        pass.set_immediates(0, bytemuck::bytes_of(&pc));
        pass.dispatch_workgroups(1, 1, 1);

        pass.set_pipeline(&self.add_block_offsets_pipeline);
        pass.set_immediates(0, bytemuck::bytes_of(&pc));
        // One thread per element here (unlike scan_blocks' two per thread).
        pass.dispatch_workgroups(params.instance_count.div_ceil(256), 1, 1);
    }
}

// ---------------------------------------------------------------------------
// Scatter stage
// ---------------------------------------------------------------------------

/// Writes each visible instance index to its scanned offset in
/// `visible_instances` (`renderer_scatter.wgsl`), completing the
/// order-preserving compaction.
pub(crate) struct ScatterStage {
    pipeline: wgpu::ComputePipeline,
}

impl ScatterStage {
    pub fn new(device: &wgpu::Device, data_bind_group_layout: &wgpu::BindGroupLayout) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Scatter Shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL_SCATTER.into()),
        });
        let pipeline = create_compute_pipeline(
            device,
            "Scatter Pipeline",
            &module,
            "scatter_main",
            &[Some(data_bind_group_layout)],
            std::mem::size_of::<InstanceCountPushConstants>() as u32,
        );
        Self { pipeline }
    }
}

impl ComputeStage for ScatterStage {
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        data_bind_group: &wgpu::BindGroup,
        params: &StageParams,
    ) {
        let pc = InstanceCountPushConstants {
            instance_count: params.instance_count,
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ObjectRenderer: Scatter Pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, data_bind_group, &[]);
        pass.set_immediates(0, bytemuck::bytes_of(&pc));
        pass.dispatch_workgroups(
            params.instance_count.div_ceil(COMPUTE_WORKGROUP_SIZE),
            1,
            1,
        );
    }
}

// ---------------------------------------------------------------------------
// Command stage
// ---------------------------------------------------------------------------

/// Converts `visible_instance_count` into the indirect draw arguments
/// (`renderer_command.wgsl`).
pub(crate) struct CommandStage {
    pipeline: wgpu::ComputePipeline,
}

impl CommandStage {
    pub fn new(device: &wgpu::Device, data_bind_group_layout: &wgpu::BindGroupLayout) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Command Shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL_COMMAND.into()),
        });
        let pipeline = create_compute_pipeline(
            device,
            "Command Pipeline",
            &module,
            "command_main",
            &[Some(data_bind_group_layout)],
            0,
        );
        Self { pipeline }
    }
}

impl ComputeStage for CommandStage {
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        data_bind_group: &wgpu::BindGroup,
        _params: &StageParams,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ObjectRenderer: Command Pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, data_bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Correctness tests for the prefix-sum stage contract, checked against a
    //! CPU exclusive scan. These need a real GPU adapter (the noop backend
    //! does not execute shaders) and skip themselves when none is available —
    //! run `cargo test -p renderer` on a machine with a GPU to exercise them.

    use super::*;

    struct TestGpu {
        device: wgpu::Device,
        queue: wgpu::Queue,
        data_bind_group_layout: wgpu::BindGroupLayout,
    }

    fn test_gpu() -> Option<TestGpu> {
        let gpu = futures::executor::block_on(gpu_utils::gpu::Gpu::new(
            gpu_utils::gpu::GpuDescriptor::default(),
        ))
        .ok()?;
        let (device, queue) = gpu.context()?;
        let data_bind_group_layout = crate::core_renderer::create_data_bind_group_layout(&device);
        Some(TestGpu {
            device,
            queue,
            data_bind_group_layout,
        })
    }

    fn cpu_exclusive_scan(flags: &[u32]) -> (Vec<u32>, u32) {
        let mut offsets = Vec::with_capacity(flags.len());
        let mut sum = 0u32;
        for &f in flags {
            offsets.push(sum);
            sum += f;
        }
        (offsets, sum)
    }

    /// Run a chain of stages over `flags` and read back
    /// (scan_offsets, count, visible_instances[..count]).
    fn run_stages(
        gpu: &TestGpu,
        stages: &[&dyn ComputeStage],
        flags: &[u32],
    ) -> (Vec<u32>, u32, Vec<u32>) {
        let device = &gpu.device;
        let n = flags.len();
        let u32_size = std::mem::size_of::<u32>() as u64;
        let flags_bytes = u32_size * n as u64;

        let make_storage = |label: &str, size: u64, extra: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | extra,
                mapped_at_creation: false,
            })
        };

        // Bindings the scan stages don't touch still need buffers to build the
        // shared bind group; small dummies suffice (dispatch-time validation
        // only covers bindings the pipeline actually uses).
        let dummy_instances = make_storage("test instances", 96, wgpu::BufferUsages::empty());
        let dummy_stencils = make_storage("test stencils", 176, wgpu::BufferUsages::empty());
        let visible_instances = make_storage(
            "test visible_instances",
            flags_bytes,
            wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        let counter = make_storage(
            "test counter",
            u32_size,
            wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        let draw_command = make_storage("test draw_command", 16, wgpu::BufferUsages::empty());
        let flags_buffer = make_storage(
            "test visibility_flags",
            flags_bytes,
            wgpu::BufferUsages::COPY_DST,
        );
        let offsets_buffer = make_storage(
            "test scan_offsets",
            flags_bytes,
            wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );

        let data_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("test data bind group"),
            layout: &gpu.data_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: dummy_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: dummy_stencils.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: visible_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: counter.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: draw_command.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: flags_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: offsets_buffer.as_entire_binding(),
                },
            ],
        });

        gpu.queue
            .write_buffer(&flags_buffer, 0, bytemuck::cast_slice(flags));
        // Poison the outputs so stale zeroes can't fake a pass.
        gpu.queue
            .write_buffer(&offsets_buffer, 0, &vec![0xffu8; flags_bytes as usize]);
        gpu.queue
            .write_buffer(&counter, 0, bytemuck::bytes_of(&0xdead_beefu32));
        gpu.queue
            .write_buffer(&visible_instances, 0, &vec![0xffu8; flags_bytes as usize]);

        let make_readback = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        let readback_offsets = make_readback("test readback offsets", flags_bytes);
        let readback_count = make_readback("test readback count", u32_size);
        let readback_visible = make_readback("test readback visible", flags_bytes);

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let params = StageParams {
            normalize_matrix: nalgebra::Matrix4::identity(),
            instance_count: n as u32,
        };
        for stage in stages {
            stage.encode(&mut encoder, &data_bind_group, &params);
        }
        encoder.copy_buffer_to_buffer(&offsets_buffer, 0, &readback_offsets, 0, flags_bytes);
        encoder.copy_buffer_to_buffer(&counter, 0, &readback_count, 0, u32_size);
        encoder.copy_buffer_to_buffer(&visible_instances, 0, &readback_visible, 0, flags_bytes);
        gpu.queue.submit(std::iter::once(encoder.finish()));

        for buffer in [&readback_offsets, &readback_count, &readback_visible] {
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.expect("buffer map"));
        }
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll");

        let offsets: Vec<u32> =
            bytemuck::cast_slice(&readback_offsets.slice(..).get_mapped_range()).to_vec();
        let count: u32 =
            bytemuck::cast_slice::<u8, u32>(&readback_count.slice(..).get_mapped_range())[0];
        let visible: Vec<u32> = bytemuck::cast_slice::<u8, u32>(
            &readback_visible.slice(..).get_mapped_range(),
        )[..count as usize]
            .to_vec();
        (offsets, count, visible)
    }

    /// Deterministic pseudo-random 0/1 flags (xorshift).
    fn random_flags(n: usize, mut seed: u32) -> Vec<u32> {
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed & 1
            })
            .collect()
    }

    fn check_stage_against_cpu(gpu: &TestGpu, stage: &dyn ComputeStage, label: &str) {
        // Lengths chosen around the Blelloch block boundaries (512 elements
        // per block) plus small and large irregular sizes.
        let lengths = [1usize, 3, 64, 511, 512, 513, 1000, 1024, 4096, 70_000];
        for &n in &lengths {
            let patterns: [(&str, Vec<u32>); 3] = [
                ("all-ones", vec![1u32; n]),
                ("all-zeros", vec![0u32; n]),
                ("random", random_flags(n, 0x9e37_79b9 ^ n as u32)),
            ];
            for (pattern, flags) in &patterns {
                let (gpu_offsets, gpu_count, _) = run_stages(gpu, &[stage], flags);
                let (cpu_offsets, cpu_count) = cpu_exclusive_scan(flags);
                assert_eq!(
                    gpu_count, cpu_count,
                    "{label}: total mismatch (n={n}, pattern={pattern})"
                );
                assert_eq!(
                    gpu_offsets, cpu_offsets,
                    "{label}: offsets mismatch (n={n}, pattern={pattern})"
                );
            }
        }
    }

    #[test]
    fn single_thread_prefix_sum_matches_cpu_scan() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skipping: no real GPU adapter available");
            return;
        };
        let stage = SingleThreadPrefixSumStage::new(&gpu.device, &gpu.data_bind_group_layout);
        check_stage_against_cpu(&gpu, &stage, "single-thread scan");
    }

    #[test]
    fn blelloch_prefix_sum_matches_cpu_scan() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skipping: no real GPU adapter available");
            return;
        };
        let stage = BlellochPrefixSumStage::new(&gpu.device, &gpu.data_bind_group_layout);
        check_stage_against_cpu(&gpu, &stage, "Blelloch scan");
    }

    /// End-to-end property of the compaction (scan + scatter): the compacted
    /// `visible_instances` array is exactly the flagged indices in ascending
    /// (submission/paint) order — the invariant whose violation caused the
    /// original intermittent draw-order bug.
    #[test]
    fn scan_plus_scatter_compacts_in_submission_order() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skipping: no real GPU adapter available");
            return;
        };
        let scan = BlellochPrefixSumStage::new(&gpu.device, &gpu.data_bind_group_layout);
        let scatter = ScatterStage::new(&gpu.device, &gpu.data_bind_group_layout);
        for n in [1usize, 64, 511, 513, 1000, 70_000] {
            let flags = random_flags(n, n as u32 | 1);
            let (_, count, visible) = run_stages(&gpu, &[&scan, &scatter], &flags);
            let expected: Vec<u32> = flags
                .iter()
                .enumerate()
                .filter(|&(_, &f)| f != 0)
                .map(|(i, _)| i as u32)
                .collect();
            assert_eq!(count as usize, expected.len(), "count mismatch (n={n})");
            assert_eq!(visible, expected, "compaction order mismatch (n={n})");
        }
    }
}
