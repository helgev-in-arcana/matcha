//// Scatter stage of the order-preserving stream compaction.
////
//// Consumes the visibility flags and their exclusive prefix sums and writes
//// the compacted index list: every visible instance i lands in
//// visible_instances[scan_offsets[i]]. Because scan_offsets is an exclusive
//// prefix sum computed from the flags, the destination slots are unique and
//// strictly increasing in i, so the compacted array keeps submission (paint)
//// order regardless of GPU thread scheduling — unlike the old
//// `visible_instances[atomicAdd(&count)] = i` pattern.

@group(0) @binding(2) var<storage, read_write> visible_instances: array<u32>;
@group(0) @binding(5) var<storage, read_write> visibility_flags: array<u32>;
@group(0) @binding(6) var<storage, read_write> scan_offsets: array<u32>;

struct Pc {
    instance_count: u32,
};
var<immediate> pc: Pc;

@compute @workgroup_size(64)
fn scatter_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let instance_index = global_id.x;
    if (instance_index >= pc.instance_count) {
        return;
    }
    if (visibility_flags[instance_index] != 0u) {
        visible_instances[scan_offsets[instance_index]] = instance_index;
    }
}
