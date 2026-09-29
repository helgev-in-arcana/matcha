//! Small native-contract fixtures shared by the GPU examples/tests.
use render_interface::*;
pub fn unit_quad() -> MeshSource {
    let mut desc = MeshDescriptor::triangles(6, 0);
    desc.bounds = Some([[0., 0., 0.], [1., 1., 0.]]);
    desc.non_overlapping = true;
    MeshSource::new(desc, |mut c| {
        let vertices = [
            Vertex {
                position: [0., 0., 0.],
                uv: [0., 0.],
            },
            Vertex {
                position: [0., 1., 0.],
                uv: [0., 1.],
            },
            Vertex {
                position: [1., 1., 0.],
                uv: [1., 1.],
            },
            Vertex {
                position: [0., 0., 0.],
                uv: [0., 0.],
            },
            Vertex {
                position: [1., 1., 0.],
                uv: [1., 1.],
            },
            Vertex {
                position: [1., 0., 0.],
                uv: [1., 0.],
            },
        ];
        upload_buffer(
            &mut c.gpu,
            c.target.vertices,
            bytemuck::cast_slice(&vertices),
        )
    })
}
pub fn rgba(size: [u32; 2], bytes: Vec<u8>) -> TextureSource {
    TextureSource::new(
        TextureDescriptor::new(size, wgpu::TextureFormat::Rgba8UnormSrgb),
        move |mut c| upload_texture(&mut c.gpu, &c.target, &bytes),
    )
}
pub fn coverage(size: [u32; 2], bytes: Vec<u8>) -> MaskSource {
    MaskSource::new(
        MaskDescriptor::new(size, wgpu::TextureFormat::R8Unorm),
        move |mut c| upload_texture(&mut c.gpu, &c.target, &bytes),
    )
}
