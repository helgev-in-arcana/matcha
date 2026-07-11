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
//// - `stencil_index`: index+1 of the associated stencil in the stencil data array.
////   0 indicates "no stencil". The shader uses `stencil_index - 1` to access the stencil.
////
//// NOTE: Keep WGSL-side layout (field order and explicit padding) compatible with the
//// Rust `InstanceData` declaration. When changing fields, update both Rust and WGSL.
struct InstanceData {
    viewport_position: mat4x4<f32>,
    atlas_page: u32,
    _padding1: u32,
    in_atlas_offset: vec2<f32>,
    in_atlas_size: vec2<f32>,
    stencil_index: u32,
    _padding2: u32,
};

//// StencilData describes a stencil polygon used to mask instances.
//// Semantics:
//// - `viewport_position`: transform mapping the unit quad into stencil space.
//// - `viewport_position_inverse_exists`: non-zero if `viewport_position` is invertible.
//// - `viewport_position_inverse`: inverse matrix used by the vertex shader to compute
////   stencil-space UV coordinates for masking.
//// - `atlas_page`: index of the stencil atlas page (texture array layer).
//// - `in_atlas_offset` / `in_atlas_size`: offset and size of the stencil image inside
////   the atlas page. Expected to be NORMALIZED UVs (0.0 .. 1.0). If the atlas returns
////   pixel coordinates, the host MUST normalize them before uploading to GPU.
////
//// NOTE: Maintain identical memory layout between this WGSL struct and the Rust
//// `StencilData` declaration (including explicit padding fields). Update both
//// definitions when changing sizes/types.
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

@group(0) @binding(0) var<storage, read> all_instances: array<InstanceData>;
@group(0) @binding(1) var<storage, read> all_stencils: array<StencilData>;
@group(0) @binding(5) var<storage, read_write> visibility_flags: array<u32>;

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

    let stencil_index_add_1 = instance.stencil_index;
    let use_stencil = stencil_index_add_1 > 0u;
    let stencil_index = max(stencil_index_add_1 - 1u, 0u);
    let stencil = all_stencils[stencil_index];

    // Visible conditions (conservative: every pixel the render pass can shade
    // lies inside texture-quad ∩ viewport, and — when a stencil masks the
    // instance — inside texture ∩ stencil and stencil ∩ viewport as well, so
    // the instance may be culled as soon as ANY of those pairwise
    // intersections is provably empty):
    // 1. instance quad overlaps the viewport
    // 2. (no active stencil) or (stencil quad overlaps the viewport
    //    and instance quad overlaps stencil quad)
    //
    // NOTE: positions are divided by w for form's sake, but the whole pipeline
    // assumes affine transforms (w == 1); the render shader interpolates
    // stencil UVs computed per-vertex, which would also break under a real
    // projective transform.

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
    let stencil_active = use_stencil && (stencil.viewport_position_inverse_exists != 0u);

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
