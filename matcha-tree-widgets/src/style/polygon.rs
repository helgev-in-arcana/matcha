//! Resolve UI-dependent polygon geometry before recording GPU work.
//!
//! Vertex colour is a private widget-painting feature: the common rendering ABI
//! has position and UV only, so this painter rasterizes colour interpolation into
//! the widget's logical texture. The final renderer owns its GPU placement.
use std::sync::Arc;

use crate::{
    paint,
    style::{PreparedStyle, Style},
};
use matcha_tree::{
    color::Color,
    ui_tree::{
        context::UiContext,
        metrics::{Constraints, QRect},
    },
};
use parking_lot::Mutex;

/// Produces CPU geometry from current UI input. Cached adaptive polygons assume
/// unchanged boundary/affine inputs mean unchanged geometry. Use do_not_cache_mesh
/// when this callback depends on other changing state in the UI context/captures.
pub trait PolygonFn: for<'a> Fn([f32; 2], &'a UiContext) -> Mesh + utils::MaybeSendSync {}
impl<F> PolygonFn for F where F: for<'a> Fn([f32; 2], &'a UiContext) -> Mesh + utils::MaybeSendSync {}

pub trait AdaptFn:
    for<'a> Fn([f32; 2], &'a UiContext) -> nalgebra::Matrix4<f32> + utils::MaybeSendSync
{
}
impl<F> AdaptFn for F where
    F: for<'a> Fn([f32; 2], &'a UiContext) -> nalgebra::Matrix4<f32> + utils::MaybeSendSync
{
}

pub struct Polygon {
    polygon: Arc<dyn PolygonFn>,
    adaptive_affine: Arc<dyn AdaptFn>,
    cache_the_mesh: bool,
    resolved: Mutex<Option<Resolved>>,
    prepared: Mutex<Option<PreparedCache>>,
}

#[derive(Clone, Debug)]
pub enum Mesh {
    TriangleStrip {
        vertices: Vec<Vertex>,
    },
    TriangleList {
        vertices: Vec<Vertex>,
    },
    TriangleFan {
        vertices: Vec<Vertex>,
    },
    TriangleIndexed {
        indices: Vec<u16>,
        vertices: Vec<Vertex>,
    },
}

#[derive(Clone, Debug)]
pub struct Vertex {
    pub position: [f32; 2],
    pub color: Color,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CacheKey {
    boundary: [u32; 2],
    affine: [u32; 16],
}
impl CacheKey {
    fn new(boundary: [f32; 2], affine: &nalgebra::Matrix4<f32>) -> Self {
        Self {
            boundary: boundary.map(f32::to_bits),
            affine: std::array::from_fn(|index| affine.as_slice()[index].to_bits()),
        }
    }
}

#[derive(Clone)]
struct Resolved {
    key: CacheKey,
    mesh: Arc<Mesh>,
    affine: nalgebra::Matrix4<f32>,
    rect: Option<QRect>,
}
impl Resolved {
    fn new(mesh: Arc<Mesh>, boundary: [f32; 2], affine: nalgebra::Matrix4<f32>) -> Self {
        let rect = mesh_bounds(&mesh, &affine);
        Self {
            key: CacheKey::new(boundary, &affine),
            mesh,
            affine,
            rect,
        }
    }
}
struct PreparedCache {
    key: CacheKey,
    offset: [u32; 2],
    // Keeping the mesh alive makes pointer identity a safe cache shortcut:
    // allocator address reuse cannot confuse two different CPU mesh contents.
    mesh: Arc<Mesh>,
    paint: PreparedStyle,
}

impl Clone for Polygon {
    fn clone(&self) -> Self {
        Self {
            polygon: self.polygon.clone(),
            adaptive_affine: self.adaptive_affine.clone(),
            cache_the_mesh: self.cache_the_mesh,
            resolved: Mutex::new(None),
            prepared: Mutex::new(None),
        }
    }
}

impl Polygon {
    pub fn new(mesh: Mesh) -> Self {
        Self::new_adaptive(move |_, _| mesh.clone())
    }
    pub fn new_adaptive<F>(polygon: F) -> Self
    where
        F: Fn([f32; 2], &UiContext) -> Mesh + utils::MaybeSendSync + 'static,
    {
        Self {
            polygon: Arc::new(polygon),
            adaptive_affine: Arc::new(|_, _| nalgebra::Matrix4::identity()),
            cache_the_mesh: true,
            resolved: Mutex::new(None),
            prepared: Mutex::new(None),
        }
    }
    pub fn adaptive_affine<F>(mut self, affine: F) -> Self
    where
        F: Fn([f32; 2], &UiContext) -> nalgebra::Matrix4<f32> + utils::MaybeSendSync + 'static,
    {
        self.adaptive_affine = Arc::new(affine);
        *self.resolved.lock() = None;
        *self.prepared.lock() = None;
        self
    }
    pub fn do_not_cache_mesh(mut self) -> Self {
        self.cache_the_mesh = false;
        *self.resolved.lock() = None;
        *self.prepared.lock() = None;
        self
    }

    fn resolve(&self, boundary: [f32; 2], ctx: &UiContext) -> Resolved {
        let affine = (self.adaptive_affine)(boundary, ctx);
        let key = CacheKey::new(boundary, &affine);
        if self.cache_the_mesh {
            let cache = self.resolved.lock();
            if let Some(cached) = cache.as_ref().filter(|cached| cached.key == key) {
                return cached.clone();
            }
        }
        // User callbacks run outside the cache lock and never become GPU callbacks.
        let mesh = Arc::new((self.polygon)(boundary, ctx));
        let resolved = Resolved::new(mesh, boundary, affine);
        if self.cache_the_mesh {
            *self.resolved.lock() = Some(resolved.clone());
        }
        resolved
    }

    fn prepare_resolved(&self, resolved: &Resolved, offset: [f32; 2]) -> Option<PreparedStyle> {
        resolved.rect?;
        let offset_key = offset.map(f32::to_bits);
        if self.cache_the_mesh {
            let cache = self.prepared.lock();
            if let Some(cached) = cache.as_ref().filter(|cached| {
                cached.key == resolved.key
                    && cached.offset == offset_key
                    && Arc::ptr_eq(&cached.mesh, &resolved.mesh)
            }) {
                return Some(cached.paint.clone());
            }
        }
        let vertices = paint_vertices(&resolved.mesh, resolved.affine, offset).ok()?;
        if vertices.is_empty() {
            return None;
        }
        let painter =
            PreparedStyle::new(move |mut context| paint::draw(&mut context, &vertices, None))
                .with_output_layout(render_interface::PrepareOutputLayout::AnyRegion);
        if self.cache_the_mesh {
            *self.prepared.lock() = Some(PreparedCache {
                key: resolved.key,
                offset: offset_key,
                mesh: resolved.mesh.clone(),
                paint: painter.clone(),
            });
        }
        Some(painter)
    }
}

impl Style for Polygon {
    fn required_region(&self, constraints: &Constraints, ctx: &UiContext) -> Option<QRect> {
        self.resolve(constraints.max_size(), ctx).rect
    }
    fn is_inside(&self, position: [f32; 2], boundary: [f32; 2], ctx: &UiContext) -> bool {
        let resolved = self.resolve(boundary, ctx);
        let mut inside = false;
        let valid = resolved.mesh.for_each_triangle(|triangle| {
            let points = triangle.map(|vertex| project(vertex.position, &resolved.affine));
            if let [Some(a), Some(b), Some(c)] = points {
                inside |= is_inside_of_triangle(position, [a, b, c]);
            }
        });
        valid.is_ok() && inside
    }
    fn prepare(
        &self,
        boundary: [f32; 2],
        offset: [f32; 2],
        ctx: &UiContext,
    ) -> Option<PreparedStyle> {
        self.prepare_resolved(&self.resolve(boundary, ctx), offset)
    }
}

impl Mesh {
    fn for_each_triangle(&self, mut emit: impl FnMut([&Vertex; 3])) -> Result<(), &'static str> {
        match self {
            Self::TriangleStrip { vertices } => {
                for index in 0..vertices.len().saturating_sub(2) {
                    let order = if index % 2 == 0 {
                        [index, index + 1, index + 2]
                    } else {
                        [index + 1, index, index + 2]
                    };
                    emit(order.map(|index| &vertices[index]));
                }
            }
            Self::TriangleList { vertices } => {
                for triangle in vertices.chunks_exact(3) {
                    emit([&triangle[0], &triangle[1], &triangle[2]]);
                }
            }
            Self::TriangleFan { vertices } => {
                for index in 1..vertices.len().saturating_sub(1) {
                    emit([&vertices[0], &vertices[index], &vertices[index + 1]]);
                }
            }
            Self::TriangleIndexed { indices, vertices } => {
                for triangle in indices.chunks_exact(3) {
                    let a = vertices
                        .get(triangle[0] as usize)
                        .ok_or("polygon index outside its vertices")?;
                    let b = vertices
                        .get(triangle[1] as usize)
                        .ok_or("polygon index outside its vertices")?;
                    let c = vertices
                        .get(triangle[2] as usize)
                        .ok_or("polygon index outside its vertices")?;
                    emit([a, b, c]);
                }
            }
        }
        Ok(())
    }
}

fn project(position: [f32; 2], transform: &nalgebra::Matrix4<f32>) -> Option<[f32; 2]> {
    let point = transform * nalgebra::Vector4::new(position[0], position[1], 0., 1.);
    (point.w > 0. && point.iter().all(|value| value.is_finite()))
        .then(|| [point.x / point.w, point.y / point.w])
}

fn mesh_bounds(mesh: &Mesh, transform: &nalgebra::Matrix4<f32>) -> Option<QRect> {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let mut valid = true;
    if let Err(error) = mesh.for_each_triangle(|triangle| {
        for vertex in triangle {
            if let Some(point) = project(vertex.position, transform) {
                for index in 0..2 {
                    min[index] = min[index].min(point[index]);
                    max[index] = max[index].max(point[index]);
                }
            } else {
                valid = false;
            }
        }
    }) {
        log::warn!("invalid polygon geometry: {error}");
        return None;
    }
    if valid
        && min.iter().chain(&max).all(|value| value.is_finite())
        && max[0] > min[0]
        && max[1] > min[1]
    {
        let rect = QRect::new(min, [max[0] - min[0], max[1] - min[1]]);
        (rect.area() > 0.).then_some(rect)
    } else {
        None
    }
}

fn paint_vertices(
    mesh: &Mesh,
    transform: nalgebra::Matrix4<f32>,
    offset: [f32; 2],
) -> Result<Vec<paint::Vertex>, &'static str> {
    let mut vertices = Vec::new();
    mesh.for_each_triangle(|triangle| {
        vertices.extend(triangle.map(|vertex| {
            paint::vertex(
                vertex.position,
                vertex.color.to_rgba_f32(),
                [0., 0.],
                transform,
                offset,
            )
        }));
    })?;
    Ok(vertices)
}

fn is_inside_of_triangle(position: [f32; 2], triangle: [[f32; 2]; 3]) -> bool {
    let [a, b, c] = triangle;
    if cross([b[0] - a[0], b[1] - a[1]], [c[0] - a[0], c[1] - a[1]]) == 0. {
        return false;
    }
    let pa = [position[0] - a[0], position[1] - a[1]];
    let pb = [position[0] - b[0], position[1] - b[1]];
    let pc = [position[0] - c[0], position[1] - c[1]];
    let ab = cross(pa, pb) >= 0.;
    let bc = cross(pb, pc) >= 0.;
    let ca = cross(pc, pa) >= 0.;
    (ab && bc && ca) || (!ab && !bc && !ca)
}
fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vertices() -> Vec<Vertex> {
        [[0., 0.], [4., 0.], [0., 4.], [4., 4.]]
            .map(|position| Vertex {
                position,
                color: Color::RgbF32 {
                    r: 1.,
                    g: 0.,
                    b: 0.,
                },
            })
            .to_vec()
    }
    fn triangle() -> Mesh {
        Mesh::TriangleList {
            vertices: vertices()[..3].to_vec(),
        }
    }

    #[test]
    fn topology_conversion_and_indices_produce_owned_triangle_lists() {
        let meshes = [
            Mesh::TriangleStrip {
                vertices: vertices(),
            },
            Mesh::TriangleFan {
                vertices: vertices(),
            },
            Mesh::TriangleList {
                vertices: vertices(),
            },
            Mesh::TriangleIndexed {
                vertices: vertices(),
                indices: vec![0, 1, 2, 1, 3, 2],
            },
        ];
        for (mesh, count) in meshes.iter().zip([6, 6, 3, 6]) {
            assert_eq!(
                paint_vertices(mesh, nalgebra::Matrix4::identity(), [0., 0.])
                    .expect("valid mesh")
                    .len(),
                count
            );
        }
        let bad = Mesh::TriangleIndexed {
            vertices: vertices(),
            indices: vec![0, 1, 9],
        };
        assert!(paint_vertices(&bad, nalgebra::Matrix4::identity(), [0., 0.]).is_err());
    }

    #[test]
    fn triangle_lists_do_not_truncate_at_u16_vertex_limits() {
        let mesh = Mesh::TriangleList {
            vertices: vec![vertices()[0].clone(); 65_538],
        };
        assert_eq!(
            paint_vertices(&mesh, nalgebra::Matrix4::identity(), [0., 0.])
                .expect("nonindexed large mesh")
                .len(),
            65_538
        );
    }

    #[test]
    fn resolved_affine_and_output_origin_agree_with_bounds() {
        let transform = nalgebra::Matrix4::new_translation(&nalgebra::Vector3::new(10., 20., 0.));
        let rect = mesh_bounds(&triangle(), &transform).expect("nonempty bounds");
        assert_eq!(
            [rect.min_x(), rect.min_y(), rect.width(), rect.height()],
            [10., 20., 4., 4.]
        );
        let vertices = paint_vertices(&triangle(), transform, [10., 20.]).expect("valid mesh");
        assert_eq!(vertices[0].position, [0., 0., 0., 1.]);
        assert_eq!(vertices[1].position, [4., 0., 0., 1.]);
    }

    #[test]
    fn exact_inputs_and_mesh_ownership_control_prepared_identity() {
        let polygon = Polygon::new(triangle());
        let resolved = Resolved::new(
            Arc::new(triangle()),
            [8., 8.],
            nalgebra::Matrix4::identity(),
        );
        let a = polygon
            .prepare_resolved(&resolved, [0., 0.])
            .expect("painter");
        assert_eq!(
            a.ids(),
            polygon
                .prepare_resolved(&resolved, [0., 0.])
                .expect("same painter")
                .ids()
        );
        let translated = polygon
            .prepare_resolved(&resolved, [1. / 4096., 0.])
            .expect("different output origin");
        assert_ne!(a.ids(), translated.ids());
        let mut affine = nalgebra::Matrix4::identity();
        affine[(0, 3)] = 1. / 4096.;
        let changed = Resolved::new(resolved.mesh.clone(), [8., 8.], affine);
        assert_ne!(
            translated.ids(),
            polygon
                .prepare_resolved(&changed, [1. / 4096., 0.])
                .expect("different affine")
                .ids()
        );
        let different_content = Resolved::new(Arc::new(triangle()), [8., 8.], affine);
        let before = polygon
            .prepare_resolved(&changed, [0., 0.])
            .expect("old content");
        assert_ne!(
            before.ids(),
            polygon
                .prepare_resolved(&different_content, [0., 0.])
                .expect("different content owner")
                .ids()
        );
        let dynamic = polygon.do_not_cache_mesh();
        let a = dynamic
            .prepare_resolved(&resolved, [0., 0.])
            .expect("dynamic painter");
        let b = dynamic
            .prepare_resolved(&resolved, [0., 0.])
            .expect("fresh dynamic painter");
        assert_ne!(a.ids(), b.ids());
    }

    #[test]
    fn hit_testing_rejects_degenerate_triangles() {
        let triangle = [[0., 0.], [1., 0.], [0., 1.]];
        assert!(is_inside_of_triangle([0.5, 0.5], triangle));
        assert!(is_inside_of_triangle([0., 0.], triangle));
        assert!(!is_inside_of_triangle([1.5, 0.5], triangle));
        assert!(!is_inside_of_triangle(
            [0.5, 0.],
            [[0., 0.], [1., 0.], [2., 0.]]
        ));
    }
}
