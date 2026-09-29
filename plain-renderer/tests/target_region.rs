//! Region destinations preserve unrelated pixels and keep Scene coordinates local.
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use plain_renderer::{PlainError, PlainRenderer, PlainTarget};
use render_interface::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[path = "../examples/support/sources.rs"]
#[allow(dead_code)]
mod sources;

const GUARD: [u8; 4] = [17, 37, 91, 255];
static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu() -> Gpu {
    futures::executor::block_on(Gpu::new(GpuDescriptor {
        backends: match std::env::var("MATCHA_TEST_BACKEND").as_deref() {
            Ok("dx12") => wgpu::Backends::DX12,
            _ => wgpu::Backends::VULKAN,
        },
        ..GpuDescriptor::standard()
    }))
    .expect("real GPU for target regions")
}

fn image(
    device: &wgpu::Device,
    size: [u32; 2],
    usage: wgpu::TextureUsages,
    srgb: bool,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("region proof parent image"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: usage | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: if srgb {
            &[wgpu::TextureFormat::Rgba8UnormSrgb]
        } else {
            &[]
        },
    })
}
fn fill(queue: &wgpu::Queue, texture: &wgpu::Texture, data: &[u8]) {
    queue.write_texture(
        texture.as_image_copy(),
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texture.width() * 4),
            rows_per_image: Some(texture.height()),
        },
        texture.size(),
    );
}
fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let row = (texture.width() * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row) * u64::from(texture.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(texture.height()),
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
    buffer
        .slice(..)
        .get_mapped_range()
        .chunks_exact(row as usize)
        .flat_map(|row| row[..texture.width() as usize * 4].iter().copied())
        .collect()
}
fn overwrite(
    expected: &mut [u8],
    width: u32,
    origin: [u32; 2],
    size: [u32; 2],
    color: impl Fn(u32, u32) -> [u8; 4],
) {
    for y in 0..size[1] {
        for x in 0..size[0] {
            let offset = (((origin[1] + y) * width + origin[0] + x) * 4) as usize;
            expected[offset..offset + 4].copy_from_slice(&color(x, y));
        }
    }
}

fn assert_region_pixels(
    actual: &[u8],
    expected: &[u8],
    width: u32,
    origin: [u32; 2],
    size: [u32; 2],
) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_chunks::<4>().0.iter())
        .enumerate()
    {
        let x = index as u32 % width;
        let y = index as u32 / width;
        if (origin[0]..origin[0] + size[0]).contains(&x)
            && (origin[1]..origin[1] + size[1]).contains(&y)
        {
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(a, e)| a.abs_diff(*e) <= 1),
                "region pixel {x},{y}: {actual:?} != {expected:?}"
            );
        } else {
            assert_eq!(
                actual, expected,
                "outside pixel {x},{y} must stay bitwise unchanged"
            );
        }
    }
}
fn transform(x: f32, y: f32, w: f32, h: f32) -> Matrix4<f32> {
    Matrix4::new_translation(&nalgebra::Vector3::new(x, y, 0.))
        * Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(w, h, 1.))
}
fn color(value: [u8; 4]) -> TextureSource {
    TextureSource::new(
        TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
        move |mut c| upload_texture(&mut c.gpu, &c.target, &value),
    )
    .with_output_layout(PrepareOutputLayout::AnyRegion)
}
fn region(view: &wgpu::TextureView, origin: [u32; 2], size: [u32; 2]) -> TextureRegion<'_> {
    TextureRegion::new(view, wgpu::TextureFormat::Rgba8Unorm, origin, size).expect("valid region")
}

#[test]
fn moving_resizing_masked_targets_and_failed_preparations_preserve_neighbors() {
    let _serial = GPU_LOCK.lock().expect("GPU lock");
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("device");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let size = [23, 17];
    let target = image(&device, size, wgpu::TextureUsages::RENDER_ATTACHMENT, false);
    let view = target.create_view(&Default::default());
    let mut expected = GUARD.repeat((size[0] * size[1]) as usize);
    fill(&queue, &target, &expected);
    let quad = sources::unit_quad().with_output_layout(PrepareOutputLayout::AnyRegion);
    let mut scene = Scene::default();
    let mesh = scene.resources.share_mesh(&quad).expect("mesh");
    let red = scene
        .resources
        .insert_texture(color([255, 0, 0, 255]))
        .expect("red");
    let green = scene
        .resources
        .insert_texture(color([0, 255, 0, 255]))
        .expect("green");
    let coverage = scene
        .resources
        .insert_mask(
            MaskSource::new(
                TextureDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
                |mut c| upload_texture(&mut c.gpu, &c.target, &[255]),
            )
            .with_output_layout(PrepareOutputLayout::AnyRegion),
        )
        .expect("coverage");
    scene.pixel_masks.push(PixelMask {
        mesh,
        texture: coverage,
        transform: transform(0., 1., 3., 1.),
        parent: None,
    });
    scene.phases.push(Phase {
        objects: vec![
            Object::new(mesh, red, transform(0., 0., 4., 3.)),
            Object {
                mask: Some(PixelMaskIndex(0)),
                ..Object::new(mesh, green, transform(1., 0., 3., 3.))
            },
        ],
    });
    let snapshot_calls = Arc::new(AtomicUsize::new(0));
    let calls = snapshot_calls.clone();
    let copied = scene
        .resources
        .insert_texture(
            TextureSource::new(
                TextureDescriptor::new([8, 6], wgpu::TextureFormat::Rgba16Float),
                move |c| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(c.gpu.snapshot.color.origin(), [0, 0]);
                    assert_eq!(
                        c.gpu.snapshot.color.size(),
                        [8, 6],
                        "snapshot uses logical region pixels, not the enclosing target"
                    );
                    assert_eq!(
                        c.gpu.snapshot.color.copy_to(
                            c.gpu.encoder,
                            &c.target.region,
                            [0, 0],
                            [0, 0],
                            [8, 6]
                        )?,
                        [8, 6]
                    );
                    Ok(())
                },
            )
            .with_output_layout(PrepareOutputLayout::AnyRegion),
        )
        .expect("snapshot copy");
    scene.phases.push(Phase {
        objects: vec![Object::new(mesh, copied, transform(0., 0., 4., 3.))],
    });
    let mut renderer = PlainRenderer::new(&device, &queue);
    for origin in [[2, 3], [15, 11]] {
        renderer
            .render(
                &scene,
                PlainTarget {
                    region: region(&view, origin, [8, 6]),
                    logical_size: [4., 3.],
                    clear: wgpu::Color::TRANSPARENT,
                    initial: None,
                },
            )
            .expect("masked local scene in a target region");
        overwrite(&mut expected, size[0], origin, [8, 6], |x, y| {
            if (2..6).contains(&x) && (2..4).contains(&y) {
                [0, 255, 0, 255]
            } else {
                [255, 0, 0, 255]
            }
        });
        assert_eq!(pixels(&device, &queue, &target), expected);
        assert_eq!(renderer.stats().working_bytes, 8 * 6 * 14);
        if origin == [15, 11] {
            assert_eq!(
                renderer.stats().prepared,
                0,
                "moving the output does not regenerate unchanged content"
            );
        }
    }
    assert_eq!(snapshot_calls.load(Ordering::SeqCst), 1);
    // Fully transparent replacement must remove old contents, not alpha-blend over them.
    renderer
        .render(
            &Scene::default(),
            PlainTarget {
                region: region(&view, [2, 3], [8, 6]),
                logical_size: [4., 3.],
                clear: wgpu::Color::TRANSPARENT,
                initial: None,
            },
        )
        .expect("empty regional frame");
    overwrite(&mut expected, size[0], [2, 3], [8, 6], |_, _| [0; 4]);
    assert_eq!(pixels(&device, &queue, &target), expected);

    let small = region(&view, [10, 1], [3, 2]);
    renderer
        .render(
            &Scene::default(),
            PlainTarget {
                region: small,
                logical_size: [6., 4.],
                clear: wgpu::Color {
                    r: 0.125,
                    g: 0.25,
                    b: 0.5,
                    a: 0.5,
                },
                initial: None,
            },
        )
        .expect("resize local work surfaces");
    overwrite(&mut expected, size[0], [10, 1], [3, 2], |_, _| {
        [32, 64, 128, 128]
    });
    assert_eq!(pixels(&device, &queue, &target), expected);
    assert_eq!(renderer.stats().working_bytes, 3 * 2 * 14);

    let fail = Arc::new(AtomicBool::new(true));
    let flag = fail.clone();
    let mut fallible = Scene::default();
    let mesh = fallible.resources.share_mesh(&quad).expect("shared mesh");
    let texture = fallible
        .resources
        .insert_texture(
            TextureSource::new(
                TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
                move |mut c| {
                    assert_eq!(c.gpu.snapshot.color.size(), [3, 2]);
                    upload_texture(&mut c.gpu, &c.target, &[0, 0, 255, 255])?;
                    if flag.swap(false, Ordering::SeqCst) {
                        Err("abort regional frame after recording an upload".into())
                    } else {
                        Ok(())
                    }
                },
            )
            .with_output_layout(PrepareOutputLayout::AnyRegion),
        )
        .expect("fallible image");
    fallible.phases.push(Phase {
        objects: vec![Object::new(mesh, texture, transform(0., 0., 6., 4.))],
    });
    let result = renderer.render(
        &fallible,
        PlainTarget {
            region: small,
            logical_size: [6., 4.],
            clear: wgpu::Color::WHITE,
            initial: None,
        },
    );
    assert!(matches!(result, Err(PlainError::Prepare { .. })));
    assert_eq!(
        pixels(&device, &queue, &target),
        expected,
        "failed recording preserves the entire parent texture"
    );
    renderer
        .render(
            &fallible,
            PlainTarget {
                region: small,
                logical_size: [6., 4.],
                clear: wgpu::Color::WHITE,
                initial: None,
            },
        )
        .expect("retry after failure");
    overwrite(&mut expected, size[0], [10, 1], [3, 2], |_, _| {
        [0, 0, 255, 255]
    });
    assert_eq!(pixels(&device, &queue, &target), expected);
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn initial_regions_have_independent_parent_sizes_and_uvs_and_validate_before_preparation() {
    let _serial = GPU_LOCK.lock().expect("GPU lock");
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("device");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = image(
        &device,
        [25, 17],
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        false,
    );
    let view = target.create_view(&Default::default());
    let destination = region(&view, [3, 4], [8, 6]);
    let mut expected = GUARD.repeat(25 * 17);
    fill(&queue, &target, &expected);
    let initial = image(
        &device,
        [23, 15],
        wgpu::TextureUsages::TEXTURE_BINDING,
        false,
    );
    let initial_view = initial.create_view(&Default::default());
    let mut source = [255, 0, 255, 255].repeat(23 * 15);
    for (i, origin) in [[2, 1], [13, 8]].into_iter().enumerate() {
        overwrite(&mut source, 23, origin, [8, 6], |x, y| {
            [
                10 + x as u8 * 7 + i as u8 * 20,
                15 + y as u8 * 9 + i as u8 * 10,
                30,
                128,
            ]
        });
    }
    fill(&queue, &initial, &source);
    let mut renderer = PlainRenderer::new(&device, &queue);
    for (i, origin) in [[2, 1], [13, 8]].into_iter().enumerate() {
        renderer
            .render(
                &Scene::default(),
                PlainTarget {
                    region: destination,
                    logical_size: [16., 12.],
                    clear: wgpu::Color::BLUE,
                    initial: Some(region(&initial_view, origin, [8, 6])),
                },
            )
            .expect("sample an initial subregion without neighboring texels");
        overwrite(&mut expected, 25, [3, 4], [8, 6], |x, y| {
            [
                10 + x as u8 * 7 + i as u8 * 20,
                15 + y as u8 * 9 + i as u8 * 10,
                157,
                255,
            ]
        });
        let actual = pixels(&device, &queue, &target);
        assert_region_pixels(&actual, &expected, 25, [3, 4], [8, 6]);
    }
    let before = pixels(&device, &queue, &target);
    let mut scene = Scene::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mesh = scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("mesh");
    let texture = scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        ))
        .expect("must not prepare");
    scene.phases.push(Phase {
        objects: vec![Object::new(mesh, texture, Matrix4::identity())],
    });
    // Size mismatch and same physical destination are both preflight failures.
    for bad_initial in [
        region(&initial_view, [2, 1], [7, 6]),
        region(&view, [15, 1], [8, 6]),
    ] {
        let result = renderer.render(
            &scene,
            PlainTarget {
                region: destination,
                logical_size: [8., 6.],
                clear: wgpu::Color::WHITE,
                initial: Some(bad_initial),
            },
        );
        assert!(matches!(result, Err(PlainError::Invalid(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(pixels(&device, &queue, &target), before);
    }
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn regional_output_and_initial_sampling_preserve_srgb_view_semantics() {
    let _serial = GPU_LOCK.lock().expect("GPU lock");
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("device");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = image(
        &device,
        [11, 9],
        wgpu::TextureUsages::RENDER_ATTACHMENT,
        true,
    );
    let view = target.create_view(&wgpu::TextureViewDescriptor {
        format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
        ..Default::default()
    });
    let destination =
        TextureRegion::new(&view, wgpu::TextureFormat::Rgba8UnormSrgb, [3, 2], [4, 3])
            .expect("sRGB region");
    let mut expected = GUARD.repeat(11 * 9);
    fill(&queue, &target, &expected);
    let mut renderer = PlainRenderer::new(&device, &queue);
    renderer
        .render(
            &Scene::default(),
            PlainTarget {
                region: destination,
                logical_size: [7., 5.],
                clear: wgpu::Color {
                    r: 0.125,
                    g: 0.25,
                    b: 0.5,
                    a: 0.5,
                },
                initial: None,
            },
        )
        .expect("sRGB subregion clear");
    overwrite(&mut expected, 11, [3, 2], [4, 3], |_, _| {
        [99, 137, 188, 128]
    });
    let actual = pixels(&device, &queue, &target);
    assert_region_pixels(&actual, &expected, 11, [3, 2], [4, 3]);
    let initial = image(&device, [10, 8], wgpu::TextureUsages::TEXTURE_BINDING, true);
    let initial_view = initial.create_view(&wgpu::TextureViewDescriptor {
        format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
        ..Default::default()
    });
    let mut data = [255, 0, 255, 255].repeat(10 * 8);
    overwrite(&mut data, 10, [2, 1], [4, 3], |_, _| [128, 64, 32, 128]);
    fill(&queue, &initial, &data);
    renderer
        .render(
            &Scene::default(),
            PlainTarget {
                region: destination,
                logical_size: [7., 5.],
                clear: wgpu::Color::TRANSPARENT,
                initial: Some(
                    TextureRegion::new(
                        &initial_view,
                        wgpu::TextureFormat::Rgba8UnormSrgb,
                        [2, 1],
                        [4, 3],
                    )
                    .expect("sRGB initial region"),
                ),
            },
        )
        .expect("sRGB decode and output encode");
    overwrite(&mut expected, 11, [3, 2], [4, 3], |_, _| [128, 64, 32, 128]);
    let actual = pixels(&device, &queue, &target);
    assert_region_pixels(&actual, &expected, 11, [3, 2], [4, 3]);
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}
