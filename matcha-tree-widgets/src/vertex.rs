mod colored_vertex;
pub use colored_vertex::*;
mod uv_vertex;
pub use uv_vertex::*;

use matcha_core::types::range::Range2D;
use nalgebra::Point3;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: Point3<f32>,
}

impl Vertex {
    pub const fn desc<'a>() -> wgpu::VertexBufferLayout<'a> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                offset: 0,
                format: wgpu::VertexFormat::Float32x3,
                shader_location: 0,
            }],
        }
    }
}

// MARK: Mesh

// pub struct Mesh {
//     pub vertices: Vec<Vertex>,
//     pub indices: Vec<u16>,
// }

// MARK: BoxMesh

pub struct BoxMesh {
    pub vertices: Vec<Vertex>,
    pub rect_indices: Vec<u16>,
    pub border_indices: Vec<u16>,
}

// MARK: BoxDescriptor

pub struct BoxDescriptor {
    pub range: Range2D<f32>,
    pub radius: f32,
    pub div: u16,
    pub border_width: f32,
}

impl Default for BoxDescriptor {
    fn default() -> Self {
        Self {
            range: Range2D::new([0.0, 0.0], [0.0, 0.0]),
            radius: 0.0,
            div: 1,
            border_width: 0.0,
        }
    }
}

impl BoxDescriptor {
    pub fn new(width: f32, height: f32, border_width: f32) -> Self {
        if width < 0.0 || height < 0.0 {
            panic!("Width and height must be greater than zero.");
        }

        Self {
            range: Range2D::new([0.0, width], [0.0, height]),
            radius: 0.0,
            div: 0,
            border_width: border_width.min(width / 2.0).min(height / 2.0),
        }
    }

    pub fn offset(mut self, x: f32, y: f32) -> Self {
        self.range = self.range.slide([x, y]);
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius.min(self.range.short_side() / 2.0);
        self.div = 16.min(radius as u16);
        self
    }

    pub fn division(mut self, div: u16) -> Self {
        self.div = div;
        self
    }
}

// MARK: box_mesh

pub fn box_mesh(desc: &BoxDescriptor) -> Option<BoxMesh> {
    if desc.border_width == 0.0 {
        // draw a simple rectangle
        single_rect(desc.range, desc.radius, desc.div)
    } else if desc.border_width >= desc.range.short_side() / 2.0 {
        // draw a simple rectangle but indices are at BoxMesh.border_indices
        let mesh = single_rect(desc.range, desc.radius, desc.div);
        // swap indices
        mesh.map(|mut m| {
            std::mem::swap(&mut m.rect_indices, &mut m.border_indices);
            m
        })
    } else {
        // draw a rectangle with border
        Some(double_rect(
            desc.range,
            desc.radius,
            desc.div,
            desc.border_width,
        ))
    }
}

// MARK: private functions

fn single_rect(range: Range2D<f32>, radius: f32, div: u16) -> Option<BoxMesh> {
    if range.short_side() == 0.0 {
        None
    } else if radius == 0.0 {
        Some(single_rect_no_round(range))
    } else {
        Some(single_rect_rounded(range, radius, div))
    }
}

fn single_rect_no_round(range: Range2D<f32>) -> BoxMesh {
    let x_range = range.x_range();
    let y_range = range.y_range();

    let vertices = vec![
        Vertex {
            position: Point3::new(x_range[0], -y_range[0], 0.0),
        },
        Vertex {
            position: Point3::new(x_range[0], -y_range[1], 0.0),
        },
        Vertex {
            position: Point3::new(x_range[1], -y_range[1], 0.0),
        },
        Vertex {
            position: Point3::new(x_range[1], -y_range[0], 0.0),
        },
    ];

    let indices = vec![0, 1, 2, 2, 3, 0];

    BoxMesh {
        vertices,
        rect_indices: indices,
        border_indices: vec![],
    }
}

fn single_rect_rounded(range: Range2D<f32>, radius: f32, div: u16) -> BoxMesh {
    let vertex = create_rounded_rect_vertex(range, radius, div);

    // Indices
    let mut indices = Vec::with_capacity(div as usize * 12);

    for i in 0..div * 2 {
        indices.push(0);
        indices.push(i + 1);
        indices.push(i + 2);

        indices.push(div * 2 + 2);
        indices.push(div * 2 + 2 + i + 1);
        indices.push(div * 2 + 2 + i + 2);
    }

    indices.push(0);
    indices.push(div * 2 + 1);
    indices.push(div * 2 + 2);

    indices.push(div * 2 + 2);
    indices.push(div * 4 + 3);
    indices.push(0);

    BoxMesh {
        vertices: vertex,
        rect_indices: indices,
        border_indices: vec![],
    }
}

fn double_rect(range: Range2D<f32>, radius: f32, div: u16, border_width: f32) -> BoxMesh {
    // it is ensured that border_width * 2,0 < range.short_side() > 0.0
    if radius == 0.0 {
        // two rectangles
        double_rect_no_round(range, border_width)
    } else if radius <= border_width {
        // outer rounded rectangle and inner simple rectangle
        double_rect_outer_round(range, radius, div, border_width)
    } else {
        // two rounded rectangles
        double_rect_full_round(range, radius, div, border_width)
    }
}

fn double_rect_no_round(range: Range2D<f32>, border_width: f32) -> BoxMesh {
    // two rectangles
    let x_range = range.x_range();
    let y_range = range.y_range();

    let vertices = vec![
        // outer
        Vertex {
            position: Point3::new(x_range[0], -y_range[0], 0.0),
        },
        Vertex {
            position: Point3::new(x_range[0], -y_range[1], 0.0),
        },
        Vertex {
            position: Point3::new(x_range[1], -y_range[1], 0.0),
        },
        Vertex {
            position: Point3::new(x_range[1], -y_range[0], 0.0),
        },
        // inner
        Vertex {
            position: Point3::new(x_range[0] + border_width, -(y_range[0] + border_width), 0.0),
        },
        Vertex {
            position: Point3::new(x_range[0] + border_width, -(y_range[1] - border_width), 0.0),
        },
        Vertex {
            position: Point3::new(x_range[1] - border_width, -(y_range[1] - border_width), 0.0),
        },
        Vertex {
            position: Point3::new(x_range[1] - border_width, -(y_range[0] + border_width), 0.0),
        },
    ];

    let rect_indices = vec![4, 5, 6, 6, 7, 4];

    let border_indices = vec![
        0, 4, 7, 7, 3, 0, // top
        3, 7, 6, 6, 2, 3, // right
        2, 6, 5, 5, 1, 2, // bottom
        1, 5, 4, 4, 0, 1, // left
    ];

    BoxMesh {
        vertices,
        rect_indices,
        border_indices,
    }
}

fn double_rect_outer_round(
    range: Range2D<f32>,
    radius: f32,
    div: u16,
    border_width: f32,
) -> BoxMesh {
    // outer rounded rectangle and inner simple rectangle
    let x_range = range.x_range();
    let y_range = range.y_range();

    let mut vertex = create_rounded_rect_vertex(range, radius, div);

    // push inner vertices
    vertex.push(Vertex {
        position: Point3::new(x_range[0] + border_width, -(y_range[0] + border_width), 0.0),
    });

    vertex.push(Vertex {
        position: Point3::new(x_range[0] + border_width, -(y_range[1] - border_width), 0.0),
    });

    vertex.push(Vertex {
        position: Point3::new(x_range[1] - border_width, -(y_range[1] - border_width), 0.0),
    });

    vertex.push(Vertex {
        position: Point3::new(x_range[1] - border_width, -(y_range[0] + border_width), 0.0),
    });

    // rect indices

    let rect_indices = vec![
        div * 4 + 4,
        div * 4 + 5,
        div * 4 + 6,
        div * 4 + 6,
        div * 4 + 7,
        div * 4 + 4,
    ];

    // border indices

    let border_polygons = div * 4 + 8;
    let mut border_indices = Vec::with_capacity((border_polygons * 3) as usize);

    // corners

    // upper left
    for i in 0..div {
        border_indices.push(i);
        border_indices.push(i + 1);
        border_indices.push(div * 4 + 4);
    }

    // left
    border_indices.push(div);
    border_indices.push(div + 1);
    border_indices.push(div * 4 + 4);
    border_indices.push(div * 4 + 4);
    border_indices.push(div + 1);
    border_indices.push(div * 4 + 5);

    // lower left
    for i in 0..div {
        border_indices.push(div + 1 + i);
        border_indices.push(div + 1 + i + 1);
        border_indices.push(div * 4 + 5);
    }

    // bottom
    border_indices.push(div * 2 + 1);
    border_indices.push(div * 2 + 2);
    border_indices.push(div * 4 + 5);
    border_indices.push(div * 4 + 5);
    border_indices.push(div * 2 + 2);
    border_indices.push(div * 4 + 6);

    // lower right
    for i in 0..div {
        border_indices.push(div * 2 + 2 + i);
        border_indices.push(div * 2 + 2 + i + 1);
        border_indices.push(div * 4 + 6);
    }

    // right
    border_indices.push(div * 3 + 2);
    border_indices.push(div * 3 + 3);
    border_indices.push(div * 4 + 6);
    border_indices.push(div * 4 + 6);
    border_indices.push(div * 3 + 3);
    border_indices.push(div * 4 + 7);

    // upper right
    for i in 0..div {
        border_indices.push(div * 3 + 3 + i);
        border_indices.push(div * 3 + 3 + i + 1);
        border_indices.push(div * 4 + 7);
    }

    // top
    border_indices.push(div * 4 + 3);
    border_indices.push(0);
    border_indices.push(div * 4 + 7);
    border_indices.push(div * 4 + 7);
    border_indices.push(0);
    border_indices.push(div * 4 + 4);

    BoxMesh {
        vertices: vertex,
        rect_indices,
        border_indices,
    }
}

fn double_rect_full_round(
    range: Range2D<f32>,
    radius: f32,
    div: u16,
    border_width: f32,
) -> BoxMesh {
    // two rounded rectangles
    let mut vertex = create_rounded_rect_vertex(range, radius, div);
    vertex.extend(create_rounded_rect_vertex(
        range.reduction(border_width).unwrap(),
        radius - border_width,
        div,
    ));

    let offset = div * 4 + 4;

    // rect indices
    let rect_polygons = div * 4 + 2;
    let mut rect_indices = Vec::with_capacity((rect_polygons * 3) as usize);

    for i in 0..rect_polygons {
        rect_indices.push(offset);
        rect_indices.push(offset + i + 1);
        rect_indices.push(offset + i + 2);
    }

    // border indices
    let border_polygons = div * 8 + 8;
    let mut border_indices = Vec::with_capacity((border_polygons * 3) as usize);

    for i in 0..(div * 4 + 3) {
        border_indices.push(i);
        border_indices.push(i + 1);
        border_indices.push(offset + i);
        border_indices.push(offset + i);
        border_indices.push(i + 1);
        border_indices.push(offset + i + 1);
    }

    border_indices.push(div * 4 + 3);
    border_indices.push(0);
    border_indices.push(div * 8 + 7); // offset + div * 4 + 3
    border_indices.push(div * 8 + 7); // offset + div * 4 + 3
    border_indices.push(0); // 0
    border_indices.push(div * 4 + 4); // offset

    BoxMesh {
        vertices: vertex,
        rect_indices,
        border_indices,
    }
}

// MARK: make rounded rect vertex

fn create_rounded_rect_vertex(range: Range2D<f32>, radius: f32, div: u16) -> Vec<Vertex> {
    let x_range = range.x_range();
    let y_range = range.y_range();
    let mut vertex = Vec::with_capacity(div as usize * 4 + 4);

    // arrangement of vertices
    //
    // round A ---- round D
    //       |            |
    //       |            |
    //       |            |
    // round B ---- round C

    // A -> B -> C -> D

    // Vertices

    let div_angle = std::f32::consts::PI / (2.0 * div as f32);

    // A

    for i in 0..=div {
        let angle = div_angle * i as f32;
        let x = x_range[0] + radius * (1.0 - angle.sin());
        let y = -(y_range[0] + radius * (1.0 - angle.cos()));
        vertex.push(Vertex {
            position: Point3::new(x, y, 0.0),
        });
    }

    // B

    for i in 0..=div {
        let angle = div_angle * i as f32;
        let x = x_range[0] + radius * (1.0 - angle.cos());
        let y = -y_range[1] + radius * (1.0 - angle.sin());
        vertex.push(Vertex {
            position: Point3::new(x, y, 0.0),
        });
    }

    // C

    for i in 0..=div {
        let angle = div_angle * i as f32;
        let x = x_range[1] - radius * (1.0 - angle.sin());
        let y = -y_range[1] + radius * (1.0 - angle.cos());
        vertex.push(Vertex {
            position: Point3::new(x, y, 0.0),
        });
    }

    // D

    for i in 0..=div {
        let angle = div_angle * i as f32;
        let x = x_range[1] - radius * (1.0 - angle.cos());
        let y = -y_range[0] - radius * (1.0 - angle.sin());
        vertex.push(Vertex {
            position: Point3::new(x, y, 0.0),
        });
    }

    vertex
}
