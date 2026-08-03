use nalgebra::Point3;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ColorVertex {
    pub position: Point3<f32>,
    pub color: [f32; 4],
}

impl ColorVertex {
    pub const fn desc<'a>() -> wgpu::VertexBufferLayout<'a> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ColorVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

impl ColorVertex {
    pub fn transform(&self, transform: &nalgebra::Matrix4<f32>) -> Self {
        let position = transform.transform_point(&self.position);
        let color = self.color;

        ColorVertex { position, color }
    }
}

// MARK: Color Mesh

#[derive(Default)]
pub struct ColorMesh {
    pub vertices: Vec<ColorVertex>,
    pub indices: Vec<u16>,
}

impl ColorMesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mesh_integrate(&mut self, other: &mut ColorMesh) {
        // check if texture are same
        if self.vertices.len() + other.vertices.len() < u16::MAX as usize {
            let offset = self.vertices.len() as u16;
            self.vertices.append(&mut other.vertices);
            self.indices
                .append(&mut other.indices.iter().map(|i| i + offset).collect());
        } else {
            panic!("Mesh too large");
        }
    }

    pub fn integrate_all(meshes: Vec<ColorMesh>) -> Self {
        let mut new_mesh = Self::new();
        for mut mesh in meshes {
            new_mesh.mesh_integrate(&mut mesh);
        }
        new_mesh
    }
}
