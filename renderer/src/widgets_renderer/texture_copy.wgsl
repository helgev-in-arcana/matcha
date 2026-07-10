// resource
@group(0) @binding(0)
var copy_source: texture_2d<f32>;
@group(0) @binding(1)
var texture_sampler: sampler;

struct PushConstants {
    color_transformation: mat4x4<f32>,
    color_offset: vec4<f32>,
    target_texture_size: vec2<f32>,
    source_texture_position_min: vec2<f32>,
    source_texture_position_max: vec2<f32>,
};
var<immediate> pc: PushConstants;

// vertex shader
@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32
) -> VertexOutput {
    var pixel_positions: vec2<f32>;
    var tex_coords: vec2<f32>;

    // tex_coords are y-flipped

    if vertex_index == 0 {
        // top-left corner
        pixel_positions = vec2<f32>(pc.source_texture_position_min.x, pc.source_texture_position_min.y);
        tex_coords = vec2<f32>(0.0, 0.0);
    } else if vertex_index == 1 {
        // bottom-left corner
        pixel_positions = vec2<f32>(pc.source_texture_position_min.x, pc.source_texture_position_max.y);
        tex_coords = vec2<f32>(0.0, 1.0);
    } else if vertex_index == 2 {
        // top-right corner
        pixel_positions = vec2<f32>(pc.source_texture_position_max.x, pc.source_texture_position_min.y);
        tex_coords = vec2<f32>(1.0, 0.0);
    } else {
        // bottom-right corner
        pixel_positions = vec2<f32>(pc.source_texture_position_max.x, pc.source_texture_position_max.y);
        tex_coords = vec2<f32>(1.0, 1.0);
    }

    let position_y_down = (pixel_positions * 2.0) / pc.target_texture_size - vec2<f32>(1.0, 1.0);

    // transform the y-axis
    return VertexOutput(
        vec4<f32>(position_y_down.x, -position_y_down.y, 0.0, 1.0),
        tex_coords
    );
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

// fragment shader
@fragment
fn fs_main(
    @location(0) tex_coords: vec2<f32>
) -> @location(0) vec4<f32> {
    let source_color = textureSample(copy_source, texture_sampler, tex_coords);
    return pc.color_transformation * source_color + pc.color_offset;
}
