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

const CLIP_VERTICES = array<vec4<f32>, 4>(
    vec4<f32>(-1.0,  1.0, 0.0, 1.0),
    vec4<f32>(-1.0, -1.0, 0.0, 1.0),
    vec4<f32>( 1.0, -1.0, 0.0, 1.0),
    vec4<f32>( 1.0,  1.0, 0.0, 1.0),
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

    // Visible conditions:
    // 1. instance is within the viewport
    // 2. (no stencil) or (stencil is within the viewport)
    // 3. instance's polygon and stencil's polygon have overlap

    var texture_position: array<vec4<f32>, 4>;
    for (var i = 0u; i < 4u; i++) {
        texture_position[i] = pc.normalize_matrix * instance.viewport_position * QUAD_VERTICES[i];
    }

    var stencil_position: array<vec4<f32>, 4>;
    for (var i = 0u; i < 4u; i++) {
        stencil_position[i] = pc.normalize_matrix * stencil.viewport_position * QUAD_VERTICES[i];
    }

    let texture_is_in_viewport = is_overlapping(texture_position, CLIP_VERTICES);
    let stencil_is_in_viewport = is_overlapping(stencil_position, CLIP_VERTICES);
    let texture_and_stencil_overlap = is_overlapping(texture_position, stencil_position);

    let is_visible = texture_is_in_viewport && (
        !use_stencil || (stencil_is_in_viewport && texture_and_stencil_overlap)
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
    //
    // TODO(follow-up): the visibility test above has a known bug, so every
    // instance is kept for now (`is_visible` is computed but unused). Once the
    // test is fixed, switch to: visibility_flags[instance_index] = select(0u, 1u, is_visible);
    _ = is_visible;
    visibility_flags[instance_index] = 1u;
}

fn is_overlapping(
    a: array<vec4<f32>, 4>,
    b: array<vec4<f32>, 4>
) -> bool {
    var flag = false;
    for (var i = 0u; i < 4u; i++) {
        flag = flag || point_in_polygon(a[i], b);
    }
    for (var i = 0u; i < 4u; i++) {
        flag = flag || point_in_polygon(b[i], a);
    }
    return flag;
}

fn cross_2d(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

fn point_in_polygon(
    point: vec4<f32>,
    polygon: array<vec4<f32>, 4>
) -> bool {
    // use cross product to determine if the point is inside the polygon
    let points = array<vec2<f32>, 4>(
        polygon[0].xy - point.xy,
        polygon[1].xy - point.xy,
        polygon[2].xy - point.xy,
        polygon[3].xy - point.xy,
    );
    let lines = array<vec2<f32>, 4>(
        polygon[1].xy - polygon[0].xy,
        polygon[2].xy - polygon[1].xy,
        polygon[3].xy - polygon[2].xy,
        polygon[0].xy - polygon[3].xy,
    );

    let signs = array<bool, 4>(
        cross_2d(points[0], lines[0]) > 0.0,
        cross_2d(points[1], lines[1]) > 0.0,
        cross_2d(points[2], lines[2]) > 0.0,
        cross_2d(points[3], lines[3]) > 0.0,
    );

    return signs[0] == signs[1] && signs[1] == signs[2] && signs[2] == signs[3];
}
