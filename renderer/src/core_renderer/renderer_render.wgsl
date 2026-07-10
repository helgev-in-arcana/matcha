// InstanceData describes a single textured instance uploaded from the host.
// Semantics:
// - `viewport_position`: 4x4 matrix that maps the unit quad vertices
//   (defined as {[0, 0], [0, 1], [1, 1], [1, 0]} in this renderer)
//   into the destination coordinate space prior to normalization. The shader
//   multiplies this with the push-constant `normalize_matrix` to produce
//   clip-space positions.
// - `atlas_page`: index of the texture array layer (page) inside the texture atlas.
// - `in_atlas_offset`: (x, y) offset of the sub-image inside the atlas page.
//   Expected units: NORMALIZED UVs (0.0 .. 1.0) relative to the atlas page.
//   If the atlas implementation provides pixel coordinates, the host MUST
//   convert them to normalized coordinates before writing InstanceData into GPU memory.
// - `in_atlas_size`: (width, height) size of the sub-image. Expected as NORMALIZED
//   values (0.0 .. 1.0). If atlas returns pixel sizes, normalize on the host side.
// - `stencil_index`: index+1 of the associated stencil in the stencil data array.
//   0 indicates "no stencil". The shader uses `stencil_index - 1` to access the stencil.
//
// NOTE: Keep WGSL-side layout (field order and explicit padding) compatible with the
// Rust `InstanceData` declaration. When changing fields, update both Rust and WGSL.
struct InstanceData {
    viewport_position: mat4x4<f32>,
    atlas_page: u32,
    _padding1: u32,
    in_atlas_offset: vec2<f32>,
    in_atlas_size: vec2<f32>,
    stencil_index: u32,
    _padding2: u32,
};

// StencilData describes a stencil polygon used to mask instances.
// Semantics:
// - `viewport_position`: transform mapping the unit quad into stencil space.
// - `viewport_position_inverse_exists`: non-zero if `viewport_position` is invertible.
// - `viewport_position_inverse`: inverse matrix used by the vertex shader to compute
//   stencil-space UV coordinates for masking.
// - `atlas_page`: index of the stencil atlas page (texture array layer).
// - `in_atlas_offset` / `in_atlas_size`: offset and size of the stencil image inside
//   the atlas page. Expected to be NORMALIZED UVs (0.0 .. 1.0). If the atlas returns
//   pixel coordinates, the host MUST normalize them before uploading to GPU.
//
// NOTE: Maintain identical memory layout between this WGSL struct and the Rust
// `StencilData` declaration (including explicit padding fields). Update both
// definitions when changing sizes/types.
struct StencilData {
    viewport_position: mat4x4<f32>,
    viewport_position_inverse_exists: u32,
    _padding1: array<u32, 3>,
    viewport_position_inverse: mat4x4<f32>,
    atlas_page: u32,
    _padding2: u32,
    in_atlas_offset: vec2<f32>,
    in_atlas_size: vec2<f32>,
    _padding3: array<u32, 2>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // texture
    @location(0) texture_uv: vec2<f32>,
    @location(1) texture_atlas_page: u32,
    @location(2) texture_atlas_bounds_x: vec2<f32>,
    @location(3) texture_atlas_bounds_y: vec2<f32>,
    // stencil
    @location(4) use_stencil: u32,
    @location(5) stencil_uv: vec2<f32>,
    @location(6) stencil_atlas_page: u32,
    @location(7) stencil_atlas_bounds_x: vec2<f32>,
    @location(8) stencil_atlas_bounds_y: vec2<f32>,
};

@group(0) @binding(0) var texture_sampler: sampler;
@group(0) @binding(1) var texture_atlas: texture_2d_array<f32>;
@group(0) @binding(2) var stencil_atlas: texture_2d_array<f32>; // R channel only be used.

@group(1) @binding(0) var<storage, read> all_instances: array<InstanceData>;
@group(1) @binding(1) var<storage, read> all_stencils: array<StencilData>;
@group(1) @binding(2) var<storage, read_write> visible_instances: array<u32>;

// Half a texel, in normalized UV units, for each atlas — see the Rust-side
// `RenderPushConstants` doc comment for why the fragment shader needs this
// (avoiding bilinear bleed with the zero-initialised margin just outside
// each atlas region's usable rectangle).
struct Pc {
    normalize_matrix: mat4x4<f32>,
    texture_atlas_half_texel: vec2<f32>,
    stencil_atlas_half_texel: vec2<f32>,
};
var<immediate> pc: Pc;

// vertices (y-axis is down, matches public UI unit-quad ordering):
// 0 - 2
// | / |
// 1 - 3
const VERTICES = array<vec4<f32>, 4>(
    vec4<f32>(0.0, 0.0, 0.0, 1.0),
    vec4<f32>(0.0, 1.0, 0.0, 1.0),
    vec4<f32>(1.0, 0.0, 0.0, 1.0),
    vec4<f32>(1.0, 1.0, 0.0, 1.0),
);
// vertices (y-axis is down):
// 0 - 3
// |   |
// 1 - 2
const UVS = array<vec2<f32>, 4>(
    vec2<f32>(0.0, 0.0),
    vec2<f32>(0.0, 1.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(1.0, 1.0),
);

@vertex
fn vertex_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32
) -> VertexOutput {
    // preparation
    let all_instance_index = visible_instances[instance_index];
    let instance = all_instances[all_instance_index];
    let stencil_index_add_1 = instance.stencil_index;
    let use_stencil = stencil_index_add_1 > 0u;
    let stencil_index = max(stencil_index_add_1 - 1u, 0u);
    let stencil = all_stencils[stencil_index];

    // vertex position
    let pre = instance.viewport_position * VERTICES[vertex_index];
    let vertex_position = pc.normalize_matrix * pre;
    let texture_uv = instance.in_atlas_offset + instance.in_atlas_size * UVS[vertex_index];

    // stencil uv
    // space that stencil position becomes {(0, 0), (0, 1), (1, 1), (1, 0)}
    let stencil_space = stencil.viewport_position_inverse * pre;
    let stencil_local_uv = (stencil_space.xy / stencil_space.w);
    // Map the local [0,1] unit-square coordinate into the stencil's atlas
    // sub-rectangle, exactly like `texture_uv` does above. Without this, the
    // fragment shader's clamp to `stencil_atlas_bounds` pins almost every
    // fragment to one corner of the atlas region instead of sweeping across
    // it, producing a smeared blob instead of the actual stencil shape.
    let stencil_uv = stencil.in_atlas_offset + stencil.in_atlas_size * stencil_local_uv;

    // output
    var output: VertexOutput;
    output.position = vertex_position;
    output.texture_uv = texture_uv;
    output.texture_atlas_page = instance.atlas_page;
    output.texture_atlas_bounds_x = vec2<f32>(instance.in_atlas_offset.x, instance.in_atlas_offset.x + instance.in_atlas_size.x);
    output.texture_atlas_bounds_y = vec2<f32>(instance.in_atlas_offset.y, instance.in_atlas_offset.y + instance.in_atlas_size.y);
    output.use_stencil = select(
        /*false*/0u,
        /*true*/ 1u,
        use_stencil && (stencil.viewport_position_inverse_exists != 0u)
    );
    output.stencil_uv = stencil_uv;
    output.stencil_atlas_page = stencil.atlas_page;
    output.stencil_atlas_bounds_x = vec2<f32>(stencil.in_atlas_offset.x, stencil.in_atlas_offset.x + stencil.in_atlas_size.x);
    output.stencil_atlas_bounds_y = vec2<f32>(stencil.in_atlas_offset.y, stencil.in_atlas_offset.y + stencil.in_atlas_size.y);
    return output;
}

@fragment
fn fragment_main(
    @location(0) texture_uv: vec2<f32>,
    @location(1) texture_atlas_page: u32,
    @location(2) texture_atlas_bounds_x: vec2<f32>,
    @location(3) texture_atlas_bounds_y: vec2<f32>,
    @location(4) use_stencil_num: u32,
    @location(5) stencil_uv: vec2<f32>,
    @location(6) stencil_atlas_page: u32,
    @location(7) stencil_atlas_bounds_x: vec2<f32>,
    @location(8) stencil_atlas_bounds_y: vec2<f32>
) -> @location(0) vec4<f32> {
    let use_stencil = use_stencil_num != 0u;

    // Clamp texture_uv/stencil_uv to the atlas bounds, inset by half a texel
    // on each side. Clamping to the exact bounds edge would let bilinear
    // filtering sample exactly halfway between the last real texel and the
    // next (zero-initialised margin) texel, bleeding the margin's
    // transparent black into every atlas region's border. Insetting to the
    // last texel's centre instead guarantees a pure sample there.
    let clamped_texture_uv = vec2<f32>(
        clamp(texture_uv.x, texture_atlas_bounds_x[0] + pc.texture_atlas_half_texel.x, texture_atlas_bounds_x[1] - pc.texture_atlas_half_texel.x),
        clamp(texture_uv.y, texture_atlas_bounds_y[0] + pc.texture_atlas_half_texel.y, texture_atlas_bounds_y[1] - pc.texture_atlas_half_texel.y)
    );

    let clamped_stencil_uv = vec2<f32>(
        clamp(stencil_uv.x, stencil_atlas_bounds_x[0] + pc.stencil_atlas_half_texel.x, stencil_atlas_bounds_x[1] - pc.stencil_atlas_half_texel.x),
        clamp(stencil_uv.y, stencil_atlas_bounds_y[0] + pc.stencil_atlas_half_texel.y, stencil_atlas_bounds_y[1] - pc.stencil_atlas_half_texel.y)
    );

    let texture_color = textureSample(
        texture_atlas,
        texture_sampler,
        clamped_texture_uv,
        texture_atlas_page,
    );

    let stencil_color = textureSample(
        stencil_atlas,
        texture_sampler,
        clamped_stencil_uv,
        stencil_atlas_page,
    );

    let stencil = select(
        /*false*/ 1.0,
        /*true*/  stencil_color.r,
        use_stencil
    );

    let final_color = texture_color * stencil;

    return final_color;
}
