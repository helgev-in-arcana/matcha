struct Params {
    transform: mat4x4<f32>,
    viewport: vec2<f32>,
    opacity: f32,
    masked: u32,
    source_uv: vec4<f32>,
    local_uv: vec4<f32>,
};
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source_image: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;
@group(0) @binding(3) var coverage: texture_2d<f32>;
@group(0) @binding(4) var local_coverage: texture_2d<f32>;
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex fn vertex(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> VertexOut {
    let p = params.transform * vec4<f32>(position, 1.0);
    var out: VertexOut;
    // GUI compositing has paint order, not a depth buffer. Preserve w for
    // projective interpolation and clipping, but do not clip local z as depth.
    out.position = vec4<f32>(2.0*p.x/params.viewport.x-p.w, p.w-2.0*p.y/params.viewport.y, 0.0, p.w);
    out.uv = uv;
    return out;
}
fn atlas_uv(uv:vec2<f32>,region:vec4<f32>,dimensions:vec2<u32>)->vec2<f32> {
    let half_texel=0.5/vec2<f32>(dimensions);
    return clamp(region.xy+uv*region.zw,region.xy+half_texel,region.xy+region.zw-half_texel);
}
fn mask_at(p: vec4<f32>) -> f32 {
    if (params.masked & 1u) == 0u { return 1.0; }
    return textureLoad(coverage, vec2<i32>(p.xy), 0).r;
}
@fragment fn color(in: VertexOut) -> @location(0) vec4<f32> {
    var local = 1.0;
    if (params.masked & 2u) != 0u { local = clamp(textureSample(local_coverage, image_sampler, atlas_uv(in.uv,params.local_uv,textureDimensions(local_coverage))).r, 0.0, 1.0); }
    return textureSample(source_image, image_sampler, atlas_uv(in.uv,params.source_uv,textureDimensions(source_image))) * params.opacity * mask_at(in.position) * local;
}
@fragment fn mask(in: VertexOut) -> @location(0) vec4<f32> {
    let c = clamp(textureSample(source_image, image_sampler, atlas_uv(in.uv,params.source_uv,textureDimensions(source_image))).r, 0.0, 1.0) * mask_at(in.position);
    return vec4<f32>(c, 0.0, 0.0, c);
}
