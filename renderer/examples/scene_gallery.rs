//! Reproducible visual PoC: `cargo run -p renderer --example scene_gallery -- target`.
//! All images come from the public Scene API on a real GPU, not CPU mockups.
#[path = "support/effects.rs"]
mod effects;
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use render_interface::*;
use renderer::{SceneRenderer, SceneTarget};
fn rect(x: f32, y: f32, w: f32, h: f32) -> Matrix4<f32> {
    Matrix4::new_translation(&nalgebra::Vector3::new(x, y, 0.))
        * Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(w, h, 1.))
}
fn read(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 256 * 4,
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
                bytes_per_row: Some(1024),
                rows_per_image: Some(256),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("readback"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completion");
    buffer.slice(..).get_mapped_range().to_vec()
}
fn main() {
    let directory = std::env::args().nth(1).unwrap_or_else(|| "target".into());
    std::fs::create_dir_all(&directory).expect("output directory");
    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor {
        required_features: wgpu::Features::empty(),
        ..Default::default()
    }))
    .expect("real GPU");
    let (device, queue) = gpu.context().expect("GPU ready");
    let mut renderer = SceneRenderer::new(&device, &queue);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gallery"),
        size: wgpu::Extent3d {
            width: 256,
            height: 256,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut report = format!("Adapter: {:?}\n", gpu.adapter().get_info());
    let mut sheet = image::RgbaImage::new(256 * 3, 256 * 2);
    for (index, name) in [
        "mesh",
        "nested-mask-popup",
        "blur",
        "refraction",
        "fractal-mask",
        "final-processing",
    ]
    .iter()
    .enumerate()
    {
        let mut scene = Scene::default();
        let quad = scene
            .resources
            .insert_mesh(matcha_paint::unit_quad())
            .expect("quad");
        let mut bytes = Vec::new();
        for y in 0..256 {
            for x in 0..256 {
                let on = (x / 16 + y / 16) % 2 == 0;
                bytes.extend_from_slice(if on {
                    &[38, 170, 220, 255]
                } else {
                    &[12, 28, 65, 255]
                });
            }
        }
        let checker = matcha_paint::Bitmap::rgba([256, 256], bytes)
            .expect("checker")
            .register_texture(&mut scene.resources);
        let white = matcha_paint::Bitmap::rgba([1, 1], vec![245, 160, 42, 255])
            .expect("solid")
            .register_texture(&mut scene.resources);
        scene.phases.push(Phase {
            objects: vec![Object::new(quad, checker, rect(0., 0., 256., 256.))],
        });
        match index {
            0 => {
                let triangle = scene
                    .resources
                    .insert_mesh(effects::compute_triangle())
                    .expect("mesh");
                scene.phases[0].objects.push(Object::new(
                    triangle,
                    white,
                    rect(30., 30., 195., 195.),
                ));
            }
            1 => {
                let mask = matcha_paint::Bitmap::coverage([1, 1], vec![190])
                    .expect("coverage")
                    .register_mask(&mut scene.resources);
                scene.pixel_masks = vec![
                    PixelMask {
                        mesh: quad,
                        texture: mask,
                        transform: rect(25., 25., 150., 180.),
                        parent: None,
                    },
                    PixelMask {
                        mesh: quad,
                        texture: mask,
                        transform: rect(65., 65., 170., 160.),
                        parent: Some(PixelMaskIndex(0)),
                    },
                ];
                scene.phases[0].objects.push(Object {
                    mask: Some(PixelMaskIndex(1)),
                    ..Object::new(quad, white, rect(0., 0., 256., 256.))
                });
                scene.phases.push(Phase {
                    objects: vec![Object::new(quad, white, rect(160., 15., 80., 35.))],
                });
            }
            2 | 3 => {
                let texture = scene
                    .resources
                    .insert_texture(effects::effect([256, 256], if index == 2 { 0 } else { 1 }))
                    .expect("effect");
                scene.phases.push(Phase {
                    objects: vec![Object::new(quad, texture, rect(0., 0., 256., 256.))],
                });
            }
            4 | 5 => {
                let mask = scene
                    .resources
                    .insert_mask(effects::fractal([256, 256]))
                    .expect("mask");
                scene.pixel_masks.push(PixelMask {
                    mesh: quad,
                    texture: mask,
                    transform: rect(0., 0., 256., 256.),
                    parent: None,
                });
                scene.phases[0].objects.push(Object {
                    mask: Some(PixelMaskIndex(0)),
                    ..Object::new(quad, white, rect(0., 0., 256., 256.))
                });
                if index == 5 {
                    let texture = scene
                        .resources
                        .insert_texture(effects::effect([256, 256], 3))
                        .expect("final processing");
                    scene.phases.push(Phase {
                        objects: vec![Object::new(quad, texture, rect(0., 0., 256., 256.))],
                    });
                }
            }
            _ => unreachable!(),
        }
        renderer
            .render(
                &scene,
                SceneTarget {
                    view: &view,
                    viewport: [256., 256.],
                    clear: wgpu::Color::BLACK,
                    initial: None,
                },
            )
            .expect("gallery render");
        let pixels = read(&device, &queue, &target);
        let path = std::path::Path::new(&directory).join(format!("scene-{name}.png"));
        image::save_buffer(&path, &pixels, 256, 256, image::ColorType::Rgba8).expect("save case");
        let tile = image::RgbaImage::from_raw(256, 256, pixels).expect("tile");
        image::imageops::replace(
            &mut sheet,
            &tile,
            ((index % 3) * 256) as i64,
            ((index / 3) * 256) as i64,
        );
        report.push_str(&format!("{name}: {:?}\n", renderer.stats()));
    }
    sheet
        .save(std::path::Path::new(&directory).join("scene-gallery.png"))
        .expect("save sheet");
    std::fs::write(
        std::path::Path::new(&directory).join("scene-gallery.txt"),
        &report,
    )
    .expect("save report");
    println!("{report}");
}
