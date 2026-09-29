//// Prefix-sum stage of the order-preserving stream compaction, reference
//// implementation: a single GPU thread performs a naive sequential scan.
////
//// Stage contract (identical to renderer_prefix_sum_blelloch.wgsl — the two
//// implementations are interchangeable behind the Rust `ComputeStage` trait):
////   input : visibility_flags[0 .. instance_count]   (each element 0 or 1)
////   output: scan_offsets[i] = exclusive prefix sum of visibility_flags,
////           i.e. the compacted slot that instance i scatters into when its
////           flag is set. Monotonically non-decreasing in i, which is what
////           guarantees that compaction preserves submission (paint) order.
////           visible_instance_count = total number of set flags.

@group(0) @binding(3) var<storage, read_write> visible_instance_count: atomic<u32>;
@group(0) @binding(5) var<storage, read_write> visibility_flags: array<u32>;
@group(0) @binding(6) var<storage, read_write> scan_offsets: array<u32>;

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

@compute @workgroup_size(1)
fn prefix_sum_main() {
    var sum = 0u;
    for (var i = 0u; i < pc.instance_count; i++) {
        scan_offsets[i] = sum;
        sum += visibility_flags[i];
    }
    atomicStore(&visible_instance_count, sum);
}
