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
// - `alpha`: draw-time opacity multiplied into the sampled colour.
// - `mask_offset` / `mask_count`: the half-open range
//   `mask_indices[offset .. offset + count]`. The instance's coverage is the
//   PRODUCT of every mask in that range, so `count == 0` means "unmasked".
//
// NOTE: Keep WGSL-side layout (field order and explicit padding) compatible with the
// Rust `InstanceData` declaration. When changing fields, update both Rust and WGSL.
struct InstanceData {
    viewport_position: mat4x4<f32>,
    atlas_page: u32,
    alpha: f32,
    in_atlas_offset: vec2<f32>,
    in_atlas_size: vec2<f32>,
    mask_offset: u32,
    mask_count: u32,
};

// MaskData describes one element of an instance's mask chain: a quad whose
// coverage texture attenuates whatever the instance draws.
//
// A mask is transformed exactly like a texture, but applied in SCREEN space:
// the fragment shader maps its own position back into the mask's local unit
// square and samples there. That is what makes a mask behave like a hole in a
// box (a portal) rather than something baked into the instance's own surface,
// and it stays correct when mask and instance are not coplanar.
//
// Semantics:
// - `viewport_position`: transform mapping the unit quad into UI space, before
//   normalization. Used for culling, and as the source of `mask_from_screen`.
// - `mask_from_screen`: inverse of that transform's PLANAR HOMOGRAPHY, mapping a
//   screen position back to the mask's local unit square. A mask's local
//   coordinates are (u, v, 0, 1), so only rows/columns {0, 1, 3} of the 4x4 ever
//   contribute; restricting to that 3x3 and inverting is exact for any affine or
//   projective transform, whereas inverting the full 4x4 would presuppose that
//   the fragment lies on the mask's plane.
// - `inverse_exists`: 0 if the transform is degenerate. Such a mask is skipped.
// - `kind`: reserved for analytic mask shapes. Only 0 (coverage texture) exists.
// - `atlas_page`: index of the mask atlas page (texture array layer).
// - `in_atlas_offset` / `in_atlas_size`: offset and size of the coverage image
//   inside the atlas page. Expected to be NORMALIZED UVs (0.0 .. 1.0). If the
//   atlas returns pixel coordinates, the host MUST normalize them before
//   uploading to GPU.
//
// NOTE: Maintain identical memory layout between this WGSL struct and the Rust
// `MaskData` declaration (including explicit padding fields). Update both
// definitions when changing sizes/types.
struct MaskData {
    viewport_position: mat4x4<f32>,
    mask_from_screen: mat3x3<f32>,
    kind: u32,
    inverse_exists: u32,
    atlas_page: u32,
    _padding1: u32,
    in_atlas_offset: vec2<f32>,
    in_atlas_size: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // texture
    @location(0) texture_uv: vec2<f32>,
    @location(1) texture_atlas_page: u32,
    @location(2) texture_atlas_bounds_x: vec2<f32>,
    @location(3) texture_atlas_bounds_y: vec2<f32>,
    // Masks carry no per-vertex data: each one is resolved from the fragment's
    // own screen position, so all the fragment stage needs is where this
    // instance's chain lives.
    @location(4) @interpolate(flat) mask_offset: u32,
    @location(5) @interpolate(flat) mask_count: u32,
    @location(6) @interpolate(flat) alpha: f32,
};

@group(0) @binding(0) var texture_sampler: sampler;
@group(0) @binding(1) var texture_atlas: texture_2d_array<f32>;
@group(0) @binding(2) var stencil_atlas: texture_2d_array<f32>; // R channel only be used.

@group(1) @binding(0) var<storage, read> all_instances: array<InstanceData>;
@group(1) @binding(1) var<storage, read> all_masks: array<MaskData>;
@group(1) @binding(2) var<storage, read_write> visible_instances: array<u32>;
@group(1) @binding(7) var<storage, read> mask_indices: array<u32>;

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

    // vertex position
    let pre = instance.viewport_position * VERTICES[vertex_index];
    let vertex_position = pc.normalize_matrix * pre;
    // Per-vertex UV, interpolated by the GPU. That interpolation is
    // perspective-correct for free, which is why textures stay on this path
    // while masks do not.
    let texture_uv = instance.in_atlas_offset + instance.in_atlas_size * UVS[vertex_index];

    // output
    var output: VertexOutput;
    output.position = vertex_position;
    output.texture_uv = texture_uv;
    output.texture_atlas_page = instance.atlas_page;
    output.texture_atlas_bounds_x = vec2<f32>(instance.in_atlas_offset.x, instance.in_atlas_offset.x + instance.in_atlas_size.x);
    output.texture_atlas_bounds_y = vec2<f32>(instance.in_atlas_offset.y, instance.in_atlas_offset.y + instance.in_atlas_size.y);
    output.mask_offset = instance.mask_offset;
    output.mask_count = instance.mask_count;
    output.alpha = instance.alpha;
    return output;
}

// Coverage below this contributes less than one step of an 8-bit channel, so
// the fragment can be dropped outright rather than blended.
const COVERAGE_EPSILON: f32 = 1.0 / 512.0;

// Coverage of one mask at `screen_pos`, in [0, 1].
//
// The mask is inverse-mapped from the screen rather than interpolated across
// the instance's quad. That is what gives a mask its portal-like behaviour --
// it masks by where it *appears* -- so it stays correct even when the mask and
// the instance are not coplanar, and it is exact under projective transforms,
// which a per-vertex UV would not be.
fn mask_coverage(mask: MaskData, screen_pos: vec2<f32>) -> f32 {
    // Degenerate transform: no usable inverse. Treated as absent (fully
    // transparent to what it would mask) rather than as occluding, so a
    // collapsed mask cannot make a widget silently vanish. Culling agrees.
    if (mask.inverse_exists == 0u) {
        return 1.0;
    }

    let h = mask.mask_from_screen * vec3<f32>(screen_pos, 1.0);
    // Behind the eye: this fragment is on the far side of the mask's plane and
    // sees no part of it.
    if (h.z <= 0.0) {
        return 0.0;
    }
    let local_uv = h.xy / h.z;

    // Outside the mask quad is REJECTED, not clamped. Clamping would let a
    // stretched mask report full coverage everywhere beyond its own edge,
    // which is precisely the opposite of clipping. The tolerance is half a
    // texel of the mask's own atlas region expressed in local units, so a
    // fragment sitting exactly on the boundary is not dropped by rounding.
    let tolerance = pc.stencil_atlas_half_texel / max(mask.in_atlas_size, vec2<f32>(1e-9));
    if (any(local_uv < -tolerance) || any(local_uv > 1.0 + tolerance)) {
        return 0.0;
    }

    // Map into the atlas sub-rectangle, then clamp to its bounds inset by half
    // a texel -- see `fragment_main` for why the inset is needed.
    let uv = mask.in_atlas_offset + mask.in_atlas_size * local_uv;
    let lo = mask.in_atlas_offset + pc.stencil_atlas_half_texel;
    let hi = mask.in_atlas_offset + mask.in_atlas_size - pc.stencil_atlas_half_texel;
    let clamped_uv = clamp(uv, min(lo, hi), max(lo, hi));

    // `textureSampleLevel`, not `textureSample`: this runs under non-uniform
    // control flow (early returns above, and a data-dependent loop in the
    // caller), where implicit-derivative sampling is not allowed. The atlases
    // have a single mip level, so an explicit LOD of 0 is exactly equivalent.
    return textureSampleLevel(stencil_atlas, texture_sampler, clamped_uv, mask.atlas_page, 0.0).r;
}

@fragment
fn fragment_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Clamp texture_uv to the atlas bounds, inset by half a texel on each
    // side. Clamping to the exact bounds edge would let bilinear filtering
    // sample exactly halfway between the last real texel and the next
    // (zero-initialised margin) texel, bleeding the margin's transparent
    // black into every atlas region's border. Insetting to the last texel's
    // centre instead guarantees a pure sample there.
    let clamped_texture_uv = vec2<f32>(
        clamp(in.texture_uv.x, in.texture_atlas_bounds_x[0] + pc.texture_atlas_half_texel.x, in.texture_atlas_bounds_x[1] - pc.texture_atlas_half_texel.x),
        clamp(in.texture_uv.y, in.texture_atlas_bounds_y[0] + pc.texture_atlas_half_texel.y, in.texture_atlas_bounds_y[1] - pc.texture_atlas_half_texel.y)
    );

    let texture_color = textureSample(
        texture_atlas,
        texture_sampler,
        clamped_texture_uv,
        in.texture_atlas_page,
    );

    // The instance's coverage is the product of its whole mask chain. Every
    // element attenuates independently, so an element that fully rejects this
    // fragment ends the loop early. Both an instance's own coverage mask (a
    // glyph stencil) and the clips it inherited from enclosing scopes live in
    // this one chain -- multiplication does not care which is which.
    var coverage = 1.0;
    for (var i = 0u; i < in.mask_count; i++) {
        coverage *= mask_coverage(all_masks[mask_indices[in.mask_offset + i]], in.position.xy);
        if (coverage < COVERAGE_EPSILON) {
            discard;
        }
    }

    return texture_color * coverage * in.alpha;
}
