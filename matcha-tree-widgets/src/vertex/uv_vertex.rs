use std::sync::Arc;

use nalgebra::{Point2, Point3};

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UvVertex {
    pub position: Point3<f32>,
    pub uv: Point2<f32>,
}

impl UvVertex {
    pub const fn desc<'a>() -> wgpu::VertexBufferLayout<'a> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<UvVertex>() as wgpu::BufferAddress,
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
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

impl UvVertex {
    pub fn transform(&self, transform: &nalgebra::Matrix4<f32>) -> Self {
        let position = transform.transform_point(&self.position);
        let uv = self.uv;

        UvVertex { position, uv }
    }
}

// MARK: Uv Mesh

pub struct UvMesh {
    pub vertices: Vec<UvVertex>,
    pub indices: Vec<u16>,
    pub texture: Arc<wgpu::Texture>,
}

impl UvMesh {
    pub fn data_len(&self) -> usize {
        self.vertices.len() + self.indices.len()
    }

    pub fn mesh_integrate(&mut self, other: &mut UvMesh) -> Result<(), ()> {
        // check if texture are same
        if Arc::ptr_eq(&self.texture, &other.texture) {
            let offset = self.vertices.len() as u16;

            self.vertices.append(&mut other.vertices);

            self.indices
                .extend(other.indices.drain(..).map(|i| i + offset));

            Ok(())
        } else {
            Err(())
        }
    }
}

fn uv_mesh_integrate(meshes: Vec<UvMesh>) -> Vec<UvMesh> {
    // integrate meshes as much as possible

    let mut new_meshes: Vec<UvMesh> = Vec::new();

    meshes.into_iter().for_each(|mut mesh| {
        let mut is_integrated = false;

        for new_mesh in new_meshes.iter_mut() {
            if new_mesh.mesh_integrate(&mut mesh).is_ok() {
                is_integrated = true;
                break;
            }
        }

        if !is_integrated {
            new_meshes.push(mesh);
        }
    });

    new_meshes
}
