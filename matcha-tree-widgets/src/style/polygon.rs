use std::sync::Arc;

use crate::style::Style;
use gpu_utils::texture_atlas::atlas_simple::atlas::AtlasRegion;
use matcha_tree::{
    color::Color,
    ui_tree::{
        context::UiContext,
        metrics::{QRect, QSize},
    },
};

use parking_lot::Mutex;
use renderer::{
    vertex::colored_vertex::ColorVertex,
    widgets_renderer::vertex_color::{RenderData, TargetData, VertexColor},
};

#[cfg(not(target_arch = "wasm32"))]
type PolygonFn = dyn for<'a> Fn([f32; 2], &'a UiContext) -> Mesh + Send + Sync + 'static;
#[cfg(target_arch = "wasm32")]
type PolygonFn = dyn for<'a> Fn([f32; 2], &'a UiContext) -> Mesh + 'static;

#[cfg(not(target_arch = "wasm32"))]
type AdaptFn =
    dyn for<'a> Fn([f32; 2], &'a UiContext) -> nalgebra::Matrix4<f32> + Send + Sync + 'static;
#[cfg(target_arch = "wasm32")]
type AdaptFn =
    dyn for<'a> Fn([f32; 2], &'a UiContext) -> nalgebra::Matrix4<f32> + 'static;

/// Quantize factor used for cache keying of matrices.
/// Matches metrics SUB_PIXEL_QUANTIZE used for QSize / QRect quantization.
const MATRIX_QUANTIZE: f32 = 256_f32;

pub struct Polygon {
    polygon: Arc<PolygonFn>,
    adaptive_affine: Arc<AdaptFn>,
    cache_the_mesh: bool,
    caches: Mutex<utils::cache::Cache<CacheKey, Caches>>,
    renderer: VertexColor,
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

#[derive(Clone)]
struct Caches {
    mesh: Mesh,
    rect: Option<QRect>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey(QSize, [i32; 16]);

impl CacheKey {
    fn new(boundary: [f32; 2], adaptive_affine: &nalgebra::Matrix4<f32>) -> Self {
        let qsize = QSize::from(boundary);
        let mut arr = [0i32; 16];
        let slice = adaptive_affine.as_slice();
        for i in 0..16 {
            // quantize matrix entries to avoid floating point precision issues in keys
            arr[i] = (slice[i] * MATRIX_QUANTIZE) as i32;
        }
        CacheKey(qsize, arr)
    }
}

// constructor

impl Clone for Polygon {
    fn clone(&self) -> Self {
        Self {
            polygon: self.polygon.clone(),
            adaptive_affine: self.adaptive_affine.clone(),
            cache_the_mesh: self.cache_the_mesh,
            caches: Mutex::new(utils::cache::Cache::default()),
            renderer: VertexColor::default(),
        }
    }
}

impl Polygon {
    pub fn new(mesh: Mesh) -> Self {
        Self {
            polygon: Arc::new(move |_, _| mesh.clone()),
            adaptive_affine: Arc::new(|_, _| nalgebra::Matrix4::identity()),
            cache_the_mesh: true,
            caches: Mutex::new(utils::cache::Cache::default()),
            renderer: VertexColor::default(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_adaptive<F>(polygon: F) -> Self
    where
        F: Fn([f32; 2], &UiContext) -> Mesh + Send + Sync + 'static,
    {
        Self {
            polygon: Arc::new(polygon),
            adaptive_affine: Arc::new(|_, _| nalgebra::Matrix4::identity()),
            cache_the_mesh: true,
            caches: Mutex::new(utils::cache::Cache::default()),
            renderer: VertexColor::default(),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn new_adaptive<F>(polygon: F) -> Self
    where
        F: Fn([f32; 2], &UiContext) -> Mesh + 'static,
    {
        Self {
            polygon: Arc::new(polygon),
            adaptive_affine: Arc::new(|_, _| nalgebra::Matrix4::identity()),
            cache_the_mesh: true,
            caches: Mutex::new(utils::cache::Cache::default()),
            renderer: VertexColor::default(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn adaptive_affine<F>(mut self, affine: F) -> Self
    where
        F: Fn([f32; 2], &UiContext) -> nalgebra::Matrix4<f32> + Send + Sync + 'static,
    {
        self.adaptive_affine = Arc::new(affine);
        self
    }

    #[cfg(target_arch = "wasm32")]
    pub fn adaptive_affine<F>(mut self, affine: F) -> Self
    where
        F: Fn([f32; 2], &UiContext) -> nalgebra::Matrix4<f32> + 'static,
    {
        self.adaptive_affine = Arc::new(affine);
        self
    }

    pub fn do_not_cache_mesh(mut self) -> Self {
        self.cache_the_mesh = false;
        self
    }
}

// MARK: Style

impl Style for Polygon {
    fn required_region(
        &self,
        constraints: &matcha_tree::ui_tree::metrics::Constraints,
        ctx: &UiContext,
    ) -> Option<matcha_tree::ui_tree::metrics::QRect> {
        let boundary = constraints.max_size();
        // compute adaptive_affine (it may affect bounding rect)
        let adaptive_affine = (self.adaptive_affine)(boundary, ctx);
        let key = CacheKey::new(boundary, &adaptive_affine);

        let mut cache = self.caches.lock();

        if self.cache_the_mesh {
            let (_k, v) = cache.get_or_insert_with(&key, || Caches {
                mesh: (self.polygon)(boundary, ctx),
                rect: None,
            });

            if let Some(rect) = v.rect {
                if rect.area() > 0.0 {
                    return Some(rect);
                } else {
                    return None;
                }
            }

            // compute bounding rect from mesh, applying adaptive_affine
            let (min_x, min_y, max_x, max_y) = {
                let mut min_x = f32::INFINITY;
                let mut min_y = f32::INFINITY;
                let mut max_x = f32::NEG_INFINITY;
                let mut max_y = f32::NEG_INFINITY;

                let apply = |pos: [f32; 2]| -> [f32; 2] {
                    let v = nalgebra::Vector4::new(pos[0], pos[1], 0.0, 1.0);
                    let r = adaptive_affine * v;
                    [r.x, r.y]
                };

                match &v.mesh {
                    Mesh::TriangleStrip { vertices }
                    | Mesh::TriangleList { vertices }
                    | Mesh::TriangleFan { vertices } => {
                        for vert in vertices {
                            let p = apply(vert.position);
                            min_x = min_x.min(p[0]);
                            min_y = min_y.min(p[1]);
                            max_x = max_x.max(p[0]);
                            max_y = max_y.max(p[1]);
                        }
                    }
                    Mesh::TriangleIndexed { indices, vertices } => {
                        for &i in indices {
                            let vert = &vertices[i as usize];
                            let p = apply(vert.position);
                            min_x = min_x.min(p[0]);
                            min_y = min_y.min(p[1]);
                            max_x = max_x.max(p[0]);
                            max_y = max_y.max(p[1]);
                        }
                    }
                }
                (min_x, min_y, max_x, max_y)
            };

            let rect = if min_x.is_finite() && min_y.is_finite() && max_x > min_x && max_y > min_y {
                QRect::new([min_x, min_y], [max_x - min_x, max_y - min_y])
            } else {
                QRect::zero()
            };

            v.rect = Some(rect);
            if v.rect.unwrap().area() > 0.0 {
                Some(v.rect.unwrap())
            } else {
                None
            }
        } else {
            let mesh = (self.polygon)(boundary, ctx);

            let (min_x, min_y, max_x, max_y) = {
                let mut min_x = f32::INFINITY;
                let mut min_y = f32::INFINITY;
                let mut max_x = f32::NEG_INFINITY;
                let mut max_y = f32::NEG_INFINITY;

                let apply = |pos: [f32; 2]| -> [f32; 2] {
                    let v = nalgebra::Vector4::new(pos[0], pos[1], 0.0, 1.0);
                    let r = adaptive_affine * v;
                    [r.x, r.y]
                };

                match &mesh {
                    Mesh::TriangleStrip { vertices }
                    | Mesh::TriangleList { vertices }
                    | Mesh::TriangleFan { vertices } => {
                        for vert in vertices {
                            let p = apply(vert.position);
                            min_x = min_x.min(p[0]);
                            min_y = min_y.min(p[1]);
                            max_x = max_x.max(p[0]);
                            max_y = max_y.max(p[1]);
                        }
                    }
                    Mesh::TriangleIndexed { indices, vertices } => {
                        for &i in indices {
                            let vert = &vertices[i as usize];
                            let p = apply(vert.position);
                            min_x = min_x.min(p[0]);
                            min_y = min_y.min(p[1]);
                            max_x = max_x.max(p[0]);
                            max_y = max_y.max(p[1]);
                        }
                    }
                }
                (min_x, min_y, max_x, max_y)
            };

            let rect = if min_x.is_finite() && min_y.is_finite() && max_x > min_x && max_y > min_y {
                QRect::new([min_x, min_y], [max_x - min_x, max_y - min_y])
            } else {
                QRect::zero()
            };

            if rect.area() > 0.0 { Some(rect) } else { None }
        }
    }

    fn is_inside(&self, position: [f32; 2], boundary_size: [f32; 2], ctx: &UiContext) -> bool {
        // include adaptive_affine in key so hit-test matches rendering/rect
        let adaptive_affine = (self.adaptive_affine)(boundary_size, ctx);
        let key = CacheKey::new(boundary_size, &adaptive_affine);

        let mut cache = self.caches.lock();

        // obtain mesh either from cache or freshly computed
        let mesh = if self.cache_the_mesh {
            cache
                .get_or_insert_with(&key, || Caches {
                    mesh: (self.polygon)(boundary_size, ctx),
                    rect: None,
                })
                .1
                .mesh
                .clone()
        } else {
            (self.polygon)(boundary_size, ctx)
        };

        // helper: apply adaptive_affine to vertex
        let apply = |pos: [f32; 2]| -> [f32; 2] {
            let v = nalgebra::Vector4::new(pos[0], pos[1], 0.0, 1.0);
            let r = adaptive_affine * v;
            [r.x, r.y]
        };

        match mesh {
            Mesh::TriangleStrip { vertices } => {
                if vertices.len() < 3 {
                    return false;
                }
                vertices.windows(3).any(|window| {
                    let triangle = [
                        apply(window[0].position),
                        apply(window[1].position),
                        apply(window[2].position),
                    ];
                    is_inside_of_triangle(position, triangle)
                })
            }
            Mesh::TriangleList { vertices } => {
                if vertices.len() < 3 {
                    return false;
                }
                vertices.chunks(3).any(|chunk| {
                    if chunk.len() == 3 {
                        let triangle = [
                            apply(chunk[0].position),
                            apply(chunk[1].position),
                            apply(chunk[2].position),
                        ];
                        is_inside_of_triangle(position, triangle)
                    } else {
                        false
                    }
                })
            }
            Mesh::TriangleFan { vertices } => {
                if vertices.len() < 3 {
                    return false;
                }
                let center = apply(vertices[0].position);
                vertices[1..].windows(2).any(|window| {
                    let triangle = [center, apply(window[0].position), apply(window[1].position)];
                    is_inside_of_triangle(position, triangle)
                })
            }
            Mesh::TriangleIndexed { indices, vertices } => {
                if indices.len() < 3 {
                    return false;
                }
                indices.chunks(3).any(|chunk| {
                    if chunk.len() == 3 {
                        let triangle = [
                            apply(vertices[chunk[0] as usize].position),
                            apply(vertices[chunk[1] as usize].position),
                            apply(vertices[chunk[2] as usize].position),
                        ];
                        is_inside_of_triangle(position, triangle)
                    } else {
                        false
                    }
                })
            }
        }
    }

    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &AtlasRegion,
        boundary_size: [f32; 2],
        offset: [f32; 2],
        ctx: &UiContext,
    ) {
        let target_size = target.texture_size();
        let target_format = target.format();
        let mut render_pass = match target.begin_render_pass(encoder) {
            Ok(rp) => rp,
            Err(_) => return,
        };
        let mut cache = self.caches.lock();

        // compute adaptive affine and include in cache key
        let adaptive_affine = (self.adaptive_affine)(boundary_size, ctx);
        let key = CacheKey::new(boundary_size, &adaptive_affine);

        let mesh = if self.cache_the_mesh {
            cache
                .get_or_insert_with(&key, || Caches {
                    mesh: (self.polygon)(boundary_size, ctx),
                    rect: None,
                })
                .1
                .mesh
                .clone()
        } else {
            (self.polygon)(boundary_size, ctx)
        };

        // build ColorVertex list and indices safely
        let (vertices, indices): (Vec<ColorVertex>, Vec<u16>) = match &mesh {
            Mesh::TriangleStrip { vertices } => {
                if vertices.len() < 3 {
                    return;
                }
                let color_vertices = vertices
                    .iter()
                    .map(|v| ColorVertex {
                        position: nalgebra::Point3::new(v.position[0], v.position[1], 0.0),
                        color: v.color.to_rgba_f32(),
                    })
                    .collect();
                let indices = (0..vertices.len() - 2)
                    .flat_map(|i| [i as u16, (i + 1) as u16, (i + 2) as u16])
                    .collect();
                (color_vertices, indices)
            }
            Mesh::TriangleList { vertices } => {
                if vertices.len() < 3 {
                    return;
                }
                let color_vertices = vertices
                    .iter()
                    .map(|v| ColorVertex {
                        position: nalgebra::Point3::new(v.position[0], v.position[1], 0.0),
                        color: v.color.to_rgba_f32(),
                    })
                    .collect();
                let indices = (0..vertices.len() as u16).collect();
                (color_vertices, indices)
            }
            Mesh::TriangleFan { vertices } => {
                if vertices.len() < 3 {
                    return;
                }
                let color_vertices = vertices
                    .iter()
                    .map(|v| ColorVertex {
                        position: nalgebra::Point3::new(v.position[0], v.position[1], 0.0),
                        color: v.color.to_rgba_f32(),
                    })
                    .collect();
                let indices = (1..vertices.len() - 1)
                    .flat_map(|i| [0u16, i as u16, (i + 1) as u16])
                    .collect();
                (color_vertices, indices)
            }
            Mesh::TriangleIndexed { indices, vertices } => {
                if indices.len() < 3 || vertices.is_empty() {
                    return;
                }
                let color_vertices = vertices
                    .iter()
                    .map(|v| ColorVertex {
                        position: nalgebra::Point3::new(v.position[0], v.position[1], 0.0),
                        color: v.color.to_rgba_f32(),
                    })
                    .collect();
                (color_vertices, indices.clone())
            }
        };

        if vertices.is_empty() || indices.is_empty() {
            return;
        }

        let transform =
            nalgebra::Matrix4::new_translation(&nalgebra::Vector3::new(offset[0], offset[1], 0.0))
                * adaptive_affine;

        // Pass adaptive_affine through RenderData so renderer can compose final push-constant matrix.
        self.renderer.render(
            ctx.gpu_queue(),
            &mut render_pass,
            TargetData {
                target_size,
                target_format,
            },
            RenderData {
                vertices: &vertices,
                indices: &indices,
                transform,
            },
            ctx.gpu_device(),
        );
    }
}

fn is_inside_of_triangle(position: [f32; 2], triangle: [[f32; 2]; 3]) -> bool {
    let [a, b, c] = triangle;

    // use cross product to determine if the point is inside the triangle

    let pa = [position[0] - a[0], position[1] - a[1]];
    let pb = [position[0] - b[0], position[1] - b[1]];
    let pc = [position[0] - c[0], position[1] - c[1]];

    let cross_ab_positive = cross(pa, pb) >= 0.0;
    let cross_bc_positive = cross(pb, pc) >= 0.0;
    let cross_ca_positive = cross(pc, pa) >= 0.0;

    (cross_ab_positive && cross_bc_positive && cross_ca_positive)
        || (!cross_ab_positive && !cross_bc_positive && !cross_ca_positive)
}

fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_inside_of_triangle() {
        let triangle = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        assert!(is_inside_of_triangle([0.5, 0.5], triangle));
        assert!(is_inside_of_triangle([0.0, 0.0], triangle));
        assert!(!is_inside_of_triangle([1.5, 0.5], triangle));
        assert!(!is_inside_of_triangle([-0.5, -0.5], triangle));
    }
}
