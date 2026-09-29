//// Prefix-sum stage of the order-preserving stream compaction, default
//// implementation: two-level Blelloch (work-efficient) scan.
////
//// Stage contract (identical to renderer_prefix_sum_single_thread.wgsl — the
//// two implementations are interchangeable behind the Rust `ComputeStage`
//// trait):
////   input : visibility_flags[0 .. instance_count]   (each element 0 or 1)
////   output: scan_offsets[i] = exclusive prefix sum of visibility_flags,
////           i.e. the compacted slot that instance i scatters into when its
////           flag is set. Monotonically non-decreasing in i, which is what
////           guarantees that compaction preserves submission (paint) order.
////           visible_instance_count = total number of set flags.
////
//// Structure (three dispatches, in order, recorded by BlellochPrefixSumStage):
////   1. scan_blocks       — each workgroup Blelloch-scans one block of
////                          BLOCK_ELEMENTS flags in workgroup memory, writes
////                          the per-element exclusive results to scan_offsets
////                          and the block total to block_sums[block].
////   2. scan_block_sums   — a single workgroup Blelloch-scans block_sums in
////                          place (exclusive) and stores the grand total to
////                          visible_instance_count.
////   3. add_block_offsets — each element adds its block's scanned base offset.
////
//// Capacity: MAX_BLOCKS * BLOCK_ELEMENTS = 262,144 elements. The Rust stage
//// falls back to the single-thread implementation above this limit.

@group(0) @binding(3) var<storage, read_write> visible_instance_count: atomic<u32>;
@group(0) @binding(5) var<storage, read_write> visibility_flags: array<u32>;
@group(0) @binding(6) var<storage, read_write> scan_offsets: array<u32>;
// Stage-internal scratch buffer, owned by BlellochPrefixSumStage (not part of
// the shared data bind group). One u32 per block, MAX_BLOCKS entries.
@group(1) @binding(0) var<storage, read_write> block_sums: array<u32>;

// Per-frame parameters. Must match `FrameParams` in `core_renderer.rs` and stay
// identical across all five shaders that declare it. Pad with scalar u32s only
// — `vec3<u32>` has alignment 16 and would grow the struct past 96 bytes.
struct Pc {
    normalize_matrix: mat4x4<f32>,
    instance_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    texture_atlas_half_texel: vec2<f32>,
    stencil_atlas_half_texel: vec2<f32>,
};
var<immediate> pc: Pc;

const WORKGROUP_SIZE: u32 = 256u;
// Each thread owns two elements of the shared scratch array.
const BLOCK_ELEMENTS: u32 = 512u;
const MAX_BLOCKS: u32 = 512u;

var<workgroup> scratch: array<u32, BLOCK_ELEMENTS>;

//// Exclusive Blelloch scan over the whole `scratch` array (classic GPU Gems 3
//// formulation: up-sweep to a reduction tree, clear the root, down-sweep).
//// Thread `t` owns elements 2t and 2t+1. Returns the total of all inputs
//// (valid in every thread). Must be called from uniform control flow.
fn scan_scratch_exclusive(t: u32) -> u32 {
    // Up-sweep (reduce).
    var offset = 1u;
    for (var d = BLOCK_ELEMENTS / 2u; d > 0u; d = d >> 1u) {
        workgroupBarrier();
        if (t < d) {
            let ai = offset * (2u * t + 1u) - 1u;
            let bi = offset * (2u * t + 2u) - 1u;
            scratch[bi] += scratch[ai];
        }
        offset = offset << 1u;
    }

    // The root now holds the total; broadcast it before clearing.
    workgroupBarrier();
    let total = scratch[BLOCK_ELEMENTS - 1u];
    workgroupBarrier();
    if (t == 0u) {
        scratch[BLOCK_ELEMENTS - 1u] = 0u;
    }

    // Down-sweep.
    for (var d = 1u; d < BLOCK_ELEMENTS; d = d << 1u) {
        offset = offset >> 1u;
        workgroupBarrier();
        if (t < d) {
            let ai = offset * (2u * t + 1u) - 1u;
            let bi = offset * (2u * t + 2u) - 1u;
            let tmp = scratch[ai];
            scratch[ai] = scratch[bi];
            scratch[bi] += tmp;
        }
    }
    workgroupBarrier();

    return total;
}

@compute @workgroup_size(256)
fn scan_blocks(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
) {
    let t = local_id.x;
    let base = workgroup_id.x * BLOCK_ELEMENTS;

    // Load two elements per thread, zero-padding past instance_count so the
    // scan of a partial tail block stays correct.
    for (var k = 0u; k < 2u; k++) {
        let i = 2u * t + k;
        let g = base + i;
        if (g < pc.instance_count) {
            scratch[i] = visibility_flags[g];
        } else {
            scratch[i] = 0u;
        }
    }

    let block_total = scan_scratch_exclusive(t);

    if (t == 0u) {
        block_sums[workgroup_id.x] = block_total;
    }

    for (var k = 0u; k < 2u; k++) {
        let i = 2u * t + k;
        let g = base + i;
        if (g < pc.instance_count) {
            scan_offsets[g] = scratch[i];
        }
    }
}

@compute @workgroup_size(256)
fn scan_block_sums(@builtin(local_invocation_id) local_id: vec3<u32>) {
    let t = local_id.x;
    let num_blocks = (pc.instance_count + BLOCK_ELEMENTS - 1u) / BLOCK_ELEMENTS;

    // Zero-pad: block_sums entries beyond num_blocks are stale scratch from
    // previous frames and must not contribute.
    for (var k = 0u; k < 2u; k++) {
        let i = 2u * t + k;
        if (i < num_blocks) {
            scratch[i] = block_sums[i];
        } else {
            scratch[i] = 0u;
        }
    }

    let total = scan_scratch_exclusive(t);

    if (t == 0u) {
        atomicStore(&visible_instance_count, total);
    }

    for (var k = 0u; k < 2u; k++) {
        let i = 2u * t + k;
        if (i < num_blocks) {
            block_sums[i] = scratch[i];
        }
    }
}

@compute @workgroup_size(256)
fn add_block_offsets(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let i = global_id.x;
    if (i >= pc.instance_count) {
        return;
    }
    // block_sums[0] is 0 after the exclusive scan, so block 0 is a no-op add.
    scan_offsets[i] += block_sums[i / BLOCK_ELEMENTS];
}
