//// InstanceData describes a single textured instance uploaded from the host.
//// Semantics:
//// - `viewport_position`: 4x4 matrix that maps the unit quad vertices
////   (defined as {[0, 0], [0, 1], [1, 1], [1, 0]} in this renderer)
////   into the destination coordinate space prior to normalization. The shader
////   multiplies this with the push-constant `normalize_matrix` to produce
////   clip-space positions.
//// - `atlas_page`: index of the texture array layer (page) inside the texture atlas.
//// - `in_atlas_offset`: (x, y) offset of the sub-image inside the atlas page.
////   Expected units: NORMALIZED UVs (0.0 .. 1.0) relative to the atlas page.
////   If the atlas implementation provides pixel coordinates, the host MUST
////   convert them to normalized coordinates before writing InstanceData into GPU memory.
//// - `in_atlas_size`: (width, height) size of the sub-image. Expected as NORMALIZED
////   values (0.0 .. 1.0). If atlas returns pixel sizes, normalize on the host side.
//// - `alpha`: draw-time opacity multiplied into the sampled colour.
//// - `mask_offset` / `mask_count`: the half-open range
////   `mask_indices[offset .. offset + count]`. The instance's coverage is the
////   PRODUCT of every mask in that range, so `count == 0` means "unmasked".
////
//// NOTE: Keep WGSL-side layout (field order and explicit padding) compatible with the
//// Rust `InstanceData` declaration. When changing fields, update both Rust and WGSL.
struct InstanceData {
    viewport_position: mat4x4<f32>,
    atlas_page: u32,
    alpha: f32,
    in_atlas_offset: vec2<f32>,
    in_atlas_size: vec2<f32>,
    mask_offset: u32,
    mask_count: u32,
};

//// MaskData describes one element of an instance's mask chain. See the Rust
//// `MaskData` declaration for the full semantics; culling only needs
//// `viewport_position` (the mask quad in UI space) and `inverse_exists`.
////
//// NOTE: Maintain identical memory layout between this WGSL struct and the Rust
//// `MaskData` declaration (including explicit padding fields). Update both
//// definitions when changing sizes/types.
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

@group(0) @binding(0) var<storage, read> all_instances: array<InstanceData>;
@group(0) @binding(1) var<storage, read> all_masks: array<MaskData>;
@group(0) @binding(5) var<storage, read_write> visibility_flags: array<u32>;
@group(0) @binding(7) var<storage, read> mask_indices: array<u32>;

struct Pc {
    normalize_matrix: mat4x4<f32>,
    instance_count: u32,
    _pad: vec3<u32>,
};
var<immediate> pc: Pc;

// vertices:
// 0 - 3
// |   |
// 1 - 2
const QUAD_VERTICES = array<vec4<f32>, 4>(
    vec4<f32>(0.0, 0.0, 0.0, 1.0),
    vec4<f32>(0.0, 1.0, 0.0, 1.0),
    vec4<f32>(1.0, 1.0, 0.0, 1.0),
    vec4<f32>(1.0, 0.0, 0.0, 1.0),
);

// Viewport corners in clip space, perimeter order.
const CLIP_VERTICES = array<vec2<f32>, 4>(
    vec2<f32>(-1.0,  1.0),
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>( 1.0,  1.0),
);

@compute @workgroup_size(64)
fn culling_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let instance_index = global_id.x;
    if (instance_index >= pc.instance_count) {
        return;
    }
    let instance = all_instances[instance_index];

    // Only the first element of the chain is honoured here; evaluating the full
    // chain lands in a later step. Chains are currently never longer than 1.
    let use_stencil = instance.mask_count > 0u;
    let stencil_index = select(0u, mask_indices[instance.mask_offset], use_stencil);
    let stencil = all_masks[stencil_index];

    // Visible conditions (conservative: every pixel the render pass can shade
    // lies inside texture-quad ∩ viewport, and — when a stencil masks the
    // instance — inside texture ∩ stencil and stencil ∩ viewport as well, so
    // the instance may be culled as soon as ANY of those pairwise
    // intersections is provably empty):
    // 1. instance quad overlaps the viewport
    // 2. (no active stencil) or (stencil quad overlaps the viewport
    //    and instance quad overlaps stencil quad)
    //
    // NOTE: positions are divided by w for form's sake, but this test still
    // assumes affine transforms (w == 1): a quad crossing the w == 0 plane
    // projects to something a convex-hull SAT test cannot describe. The render
    // shader itself no longer has that restriction — masks are inverse-mapped
    // per fragment via an exact planar homography.

    var texture_position: array<vec2<f32>, 4>;
    for (var i = 0u; i < 4u; i++) {
        let p = pc.normalize_matrix * instance.viewport_position * QUAD_VERTICES[i];
        texture_position[i] = p.xy / p.w;
    }

    var stencil_position: array<vec2<f32>, 4>;
    for (var i = 0u; i < 4u; i++) {
        let p = pc.normalize_matrix * stencil.viewport_position * QUAD_VERTICES[i];
        stencil_position[i] = p.xy / p.w;
    }

    // Mirror the render shader's stencil fallback: a non-invertible stencil
    // transform draws the instance unmasked there, so it must not participate
    // in culling here either (its degenerate quad could otherwise cull an
    // instance the render pass would draw).
    let stencil_active = use_stencil && (stencil.inverse_exists != 0u);

    let texture_is_in_viewport = is_overlapping(texture_position, CLIP_VERTICES);
    let stencil_is_in_viewport = is_overlapping(stencil_position, CLIP_VERTICES);
    let texture_and_stencil_overlap = is_overlapping(texture_position, stencil_position);

    let is_visible = texture_is_in_viewport && (
        !stencil_active || (stencil_is_in_viewport && texture_and_stencil_overlap)
    );

    // IMPORTANT: compaction must preserve submission order. Instances are
    // alpha-blended UI quads whose paint order IS their stacking order (a panel
    // background must draw before the label glyphs on top of it), and the render
    // pass draws `all_instances[visible_instances[i]]` in `i` order. An earlier
    // version compacted here with `visible_instances[atomicAdd(&count)] = index`,
    // which shuffles the array in GPU-scheduling order and intermittently drew
    // backgrounds over their own children (observed as text glyphs/images
    // flickering out). This stage therefore only emits a 0/1 visibility flag;
    // the prefix-sum + scatter stages downstream turn the flags into an
    // order-preserving compaction of `visible_instances`.
    visibility_flags[instance_index] = select(0u, 1u, is_visible);
}

//// Convex-quad overlap test via the separating axis theorem (SAT).
////
//// Both quads are affine images of the unit quad in perimeter order, i.e.
//// convex (parallelograms), so SAT with the edge normals of both quads is
//// exact. Two convex shapes are disjoint iff some edge normal is a
//// separating axis; testing all 8 edges (with strict interval comparison
//// inside `axis_separates`) therefore:
////   - treats identical quads and quads sharing only a vertex/edge as
////     overlapping (boundary-inclusive — critical because glyph instances
////     use a stencil quad bit-identical to their texture quad; the previous
////     vertex-containment test judged those "not overlapping" and culled
////     every glyph),
////   - catches cross-shaped overlaps where neither quad contains a vertex
////     of the other (e.g. a bar wider than the viewport), which the previous
////     test also missed.
//// A degenerate (zero-area) quad projects to a point/segment per axis and
//// still yields the conservative result.
fn is_overlapping(
    a: array<vec2<f32>, 4>,
    b: array<vec2<f32>, 4>
) -> bool {
    for (var i = 0u; i < 4u; i++) {
        let next = (i + 1u) % 4u;
        let edge_a = a[next] - a[i];
        if (axis_separates(a, b, vec2<f32>(-edge_a.y, edge_a.x))) {
            return false;
        }
        let edge_b = b[next] - b[i];
        if (axis_separates(a, b, vec2<f32>(-edge_b.y, edge_b.x))) {
            return false;
        }
    }
    return true;
}

//// True if the projections of `a` and `b` onto `axis` are strictly disjoint
//// intervals. A zero-length edge yields a zero axis, whose projections all
//// collapse to 0 — never reported as separating, which is the safe (keep)
//// direction.
fn axis_separates(
    a: array<vec2<f32>, 4>,
    b: array<vec2<f32>, 4>,
    axis: vec2<f32>
) -> bool {
    var min_a = dot(a[0], axis);
    var max_a = min_a;
    var min_b = dot(b[0], axis);
    var max_b = min_b;
    for (var i = 1u; i < 4u; i++) {
        let pa = dot(a[i], axis);
        min_a = min(min_a, pa);
        max_a = max(max_a, pa);
        let pb = dot(b[i], axis);
        min_b = min(min_b, pb);
        max_b = max(max_b, pb);
    }
    return max_a < min_b || max_b < min_a;
}
