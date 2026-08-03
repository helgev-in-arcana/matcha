var<immediate> normalize_affine: mat4x4<f32>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tex_coords: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0) @binding(1)
var s_diffuse: sampler;

@vertex
fn vs_main(
    model: VertexInput,
) -> VertexOutput {
    var model4 = vec4<f32>(
        model.position.x,
        model.position.y,
        model.position.z,
        1.0,
    );

    let out: VertexOutput = VertexOutput(normalize_affine * model4, model.tex_coords);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_diffuse, s_diffuse, in.tex_coords);
}
