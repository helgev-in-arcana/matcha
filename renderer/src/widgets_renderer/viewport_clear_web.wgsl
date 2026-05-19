@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    // Triangle strip full-screen quad by vertex index
    var positions = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );
    let p = positions[vertex_index];
    return vec4<f32>(p.x, p.y, 0.0, 1.0);
}

struct PushConstants {
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> pc: PushConstants;

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return pc.color;
}
