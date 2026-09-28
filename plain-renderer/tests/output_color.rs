//! Attachment encoding is a numeric output contract, separate from browser
//! canvas presentation. In particular, sRGB encoding must not affect alpha or
//! become unconditional gamma encoding for linear offscreen render targets.
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use plain_renderer::{PlainRenderer, PlainTarget};
use render_interface::*;

#[path = "../examples/support/sources.rs"]
#[allow(dead_code)]
mod sources;

const WIDTH: u32 = 8;

fn float_color(scene: &mut Scene, rgba: [f64; 4]) -> TextureId {
    let mut desc = TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba16Float);
    desc.usages = wgpu::TextureUsages::RENDER_ATTACHMENT;
    scene
        .resources
        .insert_texture(TextureSource::new(desc, move |mut context| {
            let _pass = context.target.region.begin_render_pass(
                &mut context.gpu,
                RegionRenderPassDescriptor {
                    label: Some("exact binary-fraction source"),
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: rgba[0],
                        g: rgba[1],
                        b: rgba[2],
                        a: rgba[3],
                    }),
                    ..Default::default()
                },
            )?;
            Ok(())
        }))
        .expect("fresh linear source")
}

fn output_bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut PlainRenderer,
    scene: &Scene,
    storage: wgpu::TextureFormat,
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    let view_formats: Vec<_> = (storage != format).then_some(format).into_iter().collect();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("output colour proof"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: storage,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &view_formats,
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        format: Some(format),
        ..Default::default()
    });
    renderer
        .render(
            scene,
            PlainTarget {
                view: &view,
                format,
                viewport: [WIDTH as f32, 1.],
                clear: wgpu::Color::TRANSPARENT,
                initial: None,
            },
        )
        .expect("supported full-size output view");

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("raw colour readback"),
        size: 256,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(1),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
        result.expect("map raw pixels")
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("readback completes");
    let bytes = (WIDTH * storage.block_copy_size(None).expect("uncompressed RGBA")) as usize;
    buffer.slice(..).get_mapped_range()[..bytes].to_vec()
}

fn close(actual: &[u8], expected: [u8; 4]) {
    assert!(
        actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
        "actual {actual:?}, expected {expected:?}"
    );
}

#[test]
fn linear_srgb_and_reinterpreted_outputs_preserve_their_numeric_and_alpha_contract() {
    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor {
        backends: match std::env::var("MATCHA_TEST_BACKEND").as_deref() {
            Ok("vulkan") => wgpu::Backends::VULKAN,
            Ok("dx12") => wgpu::Backends::DX12,
            _ => wgpu::Backends::PRIMARY,
        },
        ..GpuDescriptor::standard()
    }))
    .expect("real GPU required for colour encoding proof");
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = PlainRenderer::new(&device, &queue);
    let mut scene = Scene::default();
    let mesh = scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("quad");
    let transparent = float_color(&mut scene, [0., 0., 0., 0.]);
    let half = float_color(&mut scene, [0.5, 0.25, 0.125, 0.5]);
    let opaque = float_color(&mut scene, [0.5, 0.25, 0.125, 1.]);
    let black = float_color(&mut scene, [0., 0., 0., 1.]);
    let encoded = scene
        .resources
        .insert_texture(sources::rgba([1, 1], vec![188, 137, 99, 255]))
        .expect("sRGB source decodes when sampled");
    scene.phases.push(Phase {
        objects: [
            (transparent, 1.),
            (half, 1.),
            (opaque, 1.),
            (opaque, 0.5),
            (black, 1.),
            (encoded, 1.),
        ]
        .into_iter()
        .enumerate()
        .map(|(x, (texture, opacity))| Object {
            opacity,
            ..Object::new(
                mesh,
                texture,
                Matrix4::new_translation(&nalgebra::Vector3::new(x as f32, 0., 0.)),
            )
        })
        .collect(),
    });

    use wgpu::TextureFormat::{Rgba8Unorm, Rgba8UnormSrgb, Rgba16Float};
    let linear = output_bytes(
        &device,
        &queue,
        &mut renderer,
        &scene,
        Rgba8Unorm,
        Rgba8Unorm,
    );
    let encoded = output_bytes(
        &device,
        &queue,
        &mut renderer,
        &scene,
        Rgba8UnormSrgb,
        Rgba8UnormSrgb,
    );
    let reinterpret = output_bytes(
        &device,
        &queue,
        &mut renderer,
        &scene,
        Rgba8Unorm,
        Rgba8UnormSrgb,
    );
    let float = output_bytes(
        &device,
        &queue,
        &mut renderer,
        &scene,
        Rgba16Float,
        Rgba16Float,
    );
    assert_eq!(
        encoded, reinterpret,
        "view encoding controls the attachment write"
    );
    // Numeric expectations are independent of the shader implementation. Alpha
    // is unchanged by the transfer function, including exact zero and one.
    let expected_linear = [
        [0, 0, 0, 0],
        [128, 64, 32, 128],
        [128, 64, 32, 255],
        [64, 32, 16, 128],
        [0, 0, 0, 255],
        [128, 64, 32, 255],
    ];
    let expected_srgb = [
        [0, 0, 0, 0],
        [188, 137, 99, 128],
        [188, 137, 99, 255],
        [137, 99, 71, 128],
        [0, 0, 0, 255],
        [188, 137, 99, 255],
    ];
    for (index, (linear_expected, srgb_expected)) in
        expected_linear.into_iter().zip(expected_srgb).enumerate()
    {
        close(&linear[index * 4..index * 4 + 4], linear_expected);
        close(&encoded[index * 4..index * 4 + 4], srgb_expected);
        assert_eq!(
            linear[index * 4 + 3],
            encoded[index * 4 + 3],
            "alpha is linear"
        );
    }
    // Binary16 representations of exact binary fractions prove that the float
    // path preserves linear-light values, without relying on another UNORM pass.
    let expected_half = [
        [0, 0, 0, 0],
        [0x3800, 0x3400, 0x3000, 0x3800],
        [0x3800, 0x3400, 0x3000, 0x3c00],
        [0x3400, 0x3000, 0x2c00, 0x3800],
        [0, 0, 0, 0x3c00],
    ];
    for (index, expected) in expected_half.into_iter().enumerate() {
        let actual: Vec<_> = float[index * 8..index * 8 + 8]
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(actual, expected, "linear float pixel {index}");
    }
    // Encoding linear-premultiplied 0.5 gives ~0.735, larger than alpha 0.5.
    // A canvas interpreting encoded-premultiplied RGB needs a separate boundary
    // conversion; merely selecting an sRGB view cannot reorder premultiplication.
    assert!(encoded[4] > encoded[7]);
    assert!(
        futures::executor::block_on(validation.pop()).is_none(),
        "no wgpu validation errors"
    );
}
