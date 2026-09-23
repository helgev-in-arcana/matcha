//! Real pixels and real GPU callbacks. No NOOP fallback: failure to obtain an
//! adapter fails this proof rather than reporting an unexecuted pixel test green.
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use render_interface::*;
use renderer::{SceneRenderer, SceneTarget};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
#[path = "../examples/support/effects.rs"]
mod effects;
#[path = "../examples/support/private_3d.rs"]
mod private_3d;
#[path = "../examples/support/sources.rs"]
mod sources;

fn gpu_descriptor(legacy: bool) -> GpuDescriptor {
    GpuDescriptor {
        backends: match std::env::var("MATCHA_TEST_BACKEND").as_deref() {
            Ok("dx12") => wgpu::Backends::DX12,
            Ok("vulkan") => wgpu::Backends::VULKAN,
            _ => wgpu::Backends::PRIMARY,
        },
        required_features: if legacy {
            GpuDescriptor::default().required_features
        } else {
            wgpu::Features::empty()
        },
        ..Default::default()
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Matrix4<f32> {
    Matrix4::new_translation(&nalgebra::Vector3::new(x, y, 0.))
        * Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(w, h, 1.))
}
fn color(scene: &mut Scene, rgba: [u8; 4]) -> TextureId {
    scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            move |mut c| upload_texture(&mut c.gpu, &c.target, &rgba),
        ))
        .expect("fresh ID")
}
fn coverage(scene: &mut Scene, value: u8) -> MaskId {
    scene
        .resources
        .insert_mask(MaskSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
            move |mut c| upload_texture(&mut c.gpu, &c.target, &[value]),
        ))
        .expect("fresh ID")
}
fn output(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("contract pixels"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}
fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: 64 * 64 * 4,
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
                rows_per_image: Some(64),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("readback mapping"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completes");
    buffer.slice(..).get_mapped_range().to_vec()
}
fn pixel(bytes: &[u8], x: usize, y: usize) -> [u8; 4] {
    bytes[(y * 64 + x) * 4..(y * 64 + x + 1) * 4]
        .try_into()
        .expect("four channels")
}
fn close(actual: [u8; 4], expected: [u8; 4]) {
    for i in 0..4 {
        assert!(
            actual[i].abs_diff(expected[i]) <= 2,
            "actual {actual:?}, expected {expected:?}"
        );
    }
}
fn render(
    renderer: &mut SceneRenderer,
    scene: &Scene,
    texture: &wgpu::Texture,
    viewport: [f32; 2],
) {
    renderer
        .render(
            scene,
            SceneTarget {
                view: &texture.create_view(&Default::default()),
                viewport,
                clear: wgpu::Color::BLACK,
                initial: None,
            },
        )
        .expect("valid scene renders");
}

#[test]
fn real_gpu_scene_contract() {
    let gpu =
        futures::executor::block_on(Gpu::new(gpu_descriptor(false))).expect("real GPU required");
    eprintln!("Scene contract adapter: {:?}", gpu.adapter().get_info());
    let (device, queue) = gpu.context().expect("initialized GPU");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = SceneRenderer::new(&device, &queue);
    let target = output(&device);
    let mut scene = Scene::default();
    let quad = scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("fresh quad");
    let red = color(&mut scene, [255, 0, 0, 255]);
    let blue = color(&mut scene, [0, 0, 255, 255]);
    let half = coverage(&mut scene, 128);
    let opaque = coverage(&mut scene, 255);
    scene.pixel_masks = vec![
        PixelMask {
            mesh: quad,
            texture: half,
            transform: rect(0., 0., 24., 32.),
            parent: None,
        },
        PixelMask {
            mesh: quad,
            texture: half,
            transform: rect(8., 0., 24., 32.),
            parent: Some(PixelMaskIndex(0)),
        },
    ];
    let mut masked = Object::new(quad, red, rect(0., 0., 32., 32.));
    masked.mask = Some(PixelMaskIndex(1));
    scene.phases = vec![Phase {
        objects: vec![masked],
    }];
    // Physical attachment is 2x UI pixels. Coverage should be ~0.25 only in
    // the exact intersection, independent of resource texel size.
    render(&mut renderer, &scene, &target, [32., 32.]);
    let first = pixels(&device, &queue, &target);
    close(pixel(&first, 20, 20), [64, 0, 0, 255]);
    close(pixel(&first, 8, 20), [0, 0, 0, 255]);
    close(pixel(&first, 52, 20), [0, 0, 0, 255]);
    render(&mut renderer, &scene, &target, [32., 32.]);
    assert_eq!(
        renderer.stats().prepared,
        0,
        "warm cache skips every generator"
    );
    assert_eq!(first, pixels(&device, &queue, &target));

    // Popup paints later and escapes ancestry by choosing no mask.
    scene.phases.push(Phase {
        objects: vec![Object::new(quad, blue, rect(0., 0., 10., 10.))],
    });
    render(&mut renderer, &scene, &target, [32., 32.]);
    close(
        pixel(&pixels(&device, &queue, &target), 8, 8),
        [0, 0, 255, 255],
    );

    // Arbitrary indexed triangle, then use that very mesh as a mask.
    let triangle = scene
        .resources
        .insert_mesh(MeshSource::new(MeshDescriptor::triangles(3, 3), |mut c| {
            let v = [
                Vertex {
                    position: [0., 0., 0.],
                    uv: [0., 0.],
                },
                Vertex {
                    position: [1., 0., 0.],
                    uv: [1., 0.],
                },
                Vertex {
                    position: [0., 1., 0.],
                    uv: [0., 1.],
                },
            ];
            upload_buffer(&mut c.gpu, c.target.vertices, bytemuck::cast_slice(&v))?;
            upload_buffer(
                &mut c.gpu,
                c.target.indices.expect("indexed mesh"),
                bytemuck::cast_slice(&[0u32, 1, 2]),
            )
        }))
        .expect("fresh triangle");
    scene.pixel_masks.clear();
    scene.phases = vec![Phase {
        objects: vec![Object::new(triangle, red, rect(0., 0., 64., 64.))],
    }];
    render(&mut renderer, &scene, &target, [64., 64.]);
    let triangle_pixels = pixels(&device, &queue, &target);
    close(pixel(&triangle_pixels, 5, 5), [255, 0, 0, 255]);
    close(pixel(&triangle_pixels, 55, 55), [0, 0, 0, 255]);
    scene.pixel_masks.push(PixelMask {
        mesh: triangle,
        texture: opaque,
        transform: rect(0., 0., 64., 64.),
        parent: None,
    });
    scene.phases[0].objects[0] = Object {
        mask: Some(PixelMaskIndex(0)),
        ..Object::new(quad, red, rect(0., 0., 64., 64.))
    };
    render(&mut renderer, &scene, &target, [64., 64.]);
    assert_eq!(
        triangle_pixels,
        pixels(&device, &queue, &target),
        "mask mesh silhouette equals drawable mesh silhouette"
    );

    // CPU generator counts, pool-only definitions, phase snapshot and first-use.
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let copy = scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([64, 64], wgpu::TextureFormat::Rgba16Float),
            move |c| {
                count.fetch_add(1, Ordering::SeqCst);
                c.gpu.encoder.copy_texture_to_texture(
                    c.gpu.snapshot.color_texture.as_image_copy(),
                    c.target.texture.as_image_copy(),
                    c.target.texture.size(),
                );
                Ok(())
            },
        ))
        .expect("fresh snapshot copy");
    let unused = scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            |_| panic!("unused pool entry must never generate"),
        ))
        .expect("fresh unused source");
    scene.pixel_masks.clear();
    scene.phases = vec![
        Phase {
            objects: vec![Object::new(quad, red, rect(0., 0., 64., 64.))],
        },
        Phase {
            objects: vec![
                Object::new(quad, blue, rect(0., 0., 64., 64.)),
                Object::new(quad, copy, rect(0., 0., 64., 64.)),
            ],
        },
        Phase {
            objects: vec![Object::new(quad, copy, rect(0., 0., 64., 64.))],
        },
    ];
    render(&mut renderer, &scene, &target, [64., 64.]);
    close(
        pixel(&pixels(&device, &queue, &target), 32, 32),
        [255, 0, 0, 255],
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    render(&mut renderer, &scene, &target, [64., 64.]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    renderer.clear_cache();
    render(&mut renderer, &scene, &target, [64., 64.]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(scene.resources.texture(unused).is_some());

    // Warm caches cannot rescue a scene missing its definition.
    scene.resources.retain_textures(|id| id != red);
    assert!(
        renderer
            .render(
                &scene,
                SceneTarget {
                    view: &target.create_view(&Default::default()),
                    viewport: [64., 64.],
                    clear: wgpu::Color::BLACK,
                    initial: None
                }
            )
            .is_err()
    );

    // Failure after another successful preparation must not poison the cache.
    let mut failing = Scene::default();
    let q = failing
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("fresh mesh");
    let attempts = Arc::new(AtomicUsize::new(0));
    let a = attempts.clone();
    let fail = failing
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            move |mut c| {
                if a.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("intentional callback failure".into());
                }
                upload_texture(&mut c.gpu, &c.target, &[0, 255, 0, 255])
            },
        ))
        .expect("fresh failure source");
    failing.phases.push(Phase {
        objects: vec![Object::new(q, fail, rect(0., 0., 64., 64.))],
    });
    let prior = pixels(&device, &queue, &target);
    assert!(
        renderer
            .render(
                &failing,
                SceneTarget {
                    view: &target.create_view(&Default::default()),
                    viewport: [64., 64.],
                    clear: wgpu::Color::BLACK,
                    initial: None
                }
            )
            .is_err()
    );
    assert_eq!(
        prior,
        pixels(&device, &queue, &target),
        "failed frame never changes output"
    );
    render(&mut renderer, &failing, &target, [64., 64.]);
    assert_eq!(
        renderer.stats().prepared,
        2,
        "mesh must be re-recorded after rollback"
    );
    close(
        pixel(&pixels(&device, &queue, &target), 20, 20),
        [0, 255, 0, 255],
    );
    renderer.set_cache_budget(0);
    render(&mut renderer, &Scene::default(), &target, [64., 64.]);
    assert_eq!(
        renderer.stats().cache_bytes,
        0,
        "unreferenced cached resources evict under pressure"
    );
    render(&mut renderer, &failing, &target, [64., 64.]);
    assert_eq!(renderer.stats().prepared, 2);
    // Real compute mesh, background blur/refraction, procedural mask and final
    // whole-image processing. Each is a source; none owns final compositing.
    let mut effect_scene = Scene::default();
    let q = effect_scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("quad");
    let triangle = effect_scene
        .resources
        .insert_mesh(effects::compute_triangle())
        .expect("compute mesh");
    let white = color(&mut effect_scene, [255; 4]);
    effect_scene.phases = vec![Phase {
        objects: vec![Object::new(triangle, white, rect(0., 0., 64., 64.))],
    }];
    render(&mut renderer, &effect_scene, &target, [64., 64.]);
    let base = pixels(&device, &queue, &target);
    close(pixel(&base, 8, 8), [255; 4]);
    close(pixel(&base, 56, 56), [0, 0, 0, 255]);
    let blur = effect_scene
        .resources
        .insert_texture(effects::effect([64, 64], 0))
        .expect("blur");
    effect_scene.phases.push(Phase {
        objects: vec![Object::new(q, blur, rect(0., 0., 64., 64.))],
    });
    render(&mut renderer, &effect_scene, &target, [64., 64.]);
    let blurred = pixels(&device, &queue, &target);
    let edge = pixel(&blurred, 31, 32)[0];
    assert!(
        (60..190).contains(&edge),
        "blur mixes across triangle edge: {edge}"
    );
    let refract = effect_scene
        .resources
        .insert_texture(effects::effect([64, 64], 1))
        .expect("refraction");
    effect_scene.phases[1].objects[0].texture = refract;
    render(&mut renderer, &effect_scene, &target, [64., 64.]);
    assert_ne!(
        base,
        pixels(&device, &queue, &target),
        "wave refraction changes the silhouette"
    );
    let fractal = effect_scene
        .resources
        .insert_mask(effects::fractal([64, 64]))
        .expect("fractal");
    effect_scene.phases = vec![Phase {
        objects: vec![Object {
            mask: Some(PixelMaskIndex(0)),
            ..Object::new(q, white, rect(0., 0., 64., 64.))
        }],
    }];
    effect_scene.pixel_masks = vec![PixelMask {
        mesh: q,
        texture: fractal,
        transform: rect(0., 0., 64., 64.),
        parent: None,
    }];
    render(&mut renderer, &effect_scene, &target, [64., 64.]);
    let fractal_pixels = pixels(&device, &queue, &target);
    assert!(fractal_pixels.chunks_exact(4).any(|p| p[0] == 255));
    assert!(fractal_pixels.chunks_exact(4).any(|p| p[0] < 32));
    let invert = effect_scene
        .resources
        .insert_texture(effects::effect([64, 64], 3))
        .expect("final effect");
    effect_scene.phases.push(Phase {
        objects: vec![Object::new(q, invert, rect(0., 0., 64., 64.))],
    });
    render(&mut renderer, &effect_scene, &target, [64., 64.]);
    let inverted = pixels(&device, &queue, &target);
    for (a, b) in fractal_pixels.chunks_exact(4).zip(inverted.chunks_exact(4)) {
        assert!((255 - i16::from(a[0]) - i16::from(b[0])).abs() <= 2);
    }

    // More ancestors than the four cached prefix slots: exercise the bounded
    // ping-pong tail and then a sibling whose prefix shares only the root.
    effect_scene.pixel_masks.clear();
    let soft = coverage(&mut effect_scene, 230);
    for i in 0..10 {
        effect_scene.pixel_masks.push(PixelMask {
            mesh: q,
            texture: soft,
            transform: rect(i as f32, 0., 64. - i as f32, 64.),
            parent: (i > 0).then(|| PixelMaskIndex(i - 1)),
        });
    }
    effect_scene.phases = vec![Phase {
        objects: vec![Object {
            mask: Some(PixelMaskIndex(9)),
            ..Object::new(q, white, rect(0., 0., 64., 64.))
        }],
    }];
    render(&mut renderer, &effect_scene, &target, [64., 64.]);
    let deep = pixels(&device, &queue, &target);
    let expected = (255. * (230f32 / 255.).powi(10)).round() as u8;
    close(pixel(&deep, 32, 32), [expected, expected, expected, 255]);
    // Clear is opaque, so alpha remains one regardless of mask coverage.
    assert_eq!(pixel(&deep, 32, 32)[3], 255);
    // Optimized coincident coverage must match general rasterized coverage
    // under a projective transform, including draw-time opacity.
    let mut projected = Scene::default();
    let fast = projected
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("fast quad");
    let original = sources::unit_quad();
    let mut desc = *original.descriptor();
    desc.non_overlapping = false;
    let general = projected
        .resources
        .insert_mesh(MeshSource::new(desc, move |c| original.prepare(c)))
        .expect("general quad");
    let green = color(&mut projected, [0, 255, 0, 255]);
    let gradient = projected
        .resources
        .insert_mask(sources::coverage(
            [8, 8],
            (0..64).map(|i| (i % 8 * 32) as u8).collect(),
        ))
        .expect("gradient");
    let mut transform = rect(4., 4., 48., 48.);
    transform[(3, 0)] = 0.4;
    projected.pixel_masks.push(PixelMask {
        mesh: fast,
        texture: gradient,
        transform,
        parent: None,
    });
    projected.phases.push(Phase {
        objects: vec![Object {
            mask: Some(PixelMaskIndex(0)),
            opacity: 0.45,
            ..Object::new(fast, green, transform)
        }],
    });
    render(&mut renderer, &projected, &target, [64., 64.]);
    let optimized = pixels(&device, &queue, &target);
    assert_eq!(renderer.stats().mask_passes, 0);
    projected.phases[0].objects[0].mesh = general;
    projected.pixel_masks[0].mesh = general;
    render(&mut renderer, &projected, &target, [64., 64.]);
    let fallback = pixels(&device, &queue, &target);
    assert_eq!(renderer.stats().mask_passes, 1);
    assert!(
        optimized
            .iter()
            .zip(&fallback)
            .all(|(a, b)| a.abs_diff(*b) <= 1)
    );
    assert!(optimized.chunks_exact(4).filter(|p| p[1] > 20).count() > 300);

    // Render-pass-generated source (not just uploads/compute), and phase-zero
    // initial image. The second render reads the previous *explicit* target,
    // not an implicit previous-frame history inside the renderer.
    let mut initial_scene = Scene::default();
    let q = initial_scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("quad");
    let mut desc = TextureDescriptor::new([4, 4], wgpu::TextureFormat::Rgba8Unorm);
    desc.usages = wgpu::TextureUsages::RENDER_ATTACHMENT;
    let generated = initial_scene
        .resources
        .insert_texture(TextureSource::new(desc, |c| {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: c.target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let _pass = c
                .gpu
                .encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &attachments,
                    ..Default::default()
                });
            Ok(())
        }))
        .expect("render source");
    initial_scene.phases.push(Phase {
        objects: vec![Object::new(q, generated, rect(0., 0., 64., 64.))],
    });
    render(&mut renderer, &initial_scene, &target, [64., 64.]);
    close(
        pixel(&pixels(&device, &queue, &target), 30, 30),
        [0, 255, 0, 255],
    );
    let initial_target = output(&device);
    renderer
        .render(
            &Scene::default(),
            SceneTarget {
                view: &initial_target.create_view(&Default::default()),
                viewport: [64., 64.],
                clear: wgpu::Color::BLACK,
                initial: Some(&target.create_view(&Default::default())),
            },
        )
        .expect("explicit initial image");
    close(
        pixel(&pixels(&device, &queue, &initial_target), 30, 30),
        [0, 255, 0, 255],
    );

    // Malformed mask links fail before any callback or GPU recording.
    projected.pixel_masks[0].parent = Some(PixelMaskIndex(0));
    assert!(
        renderer
            .render(
                &projected,
                SceneTarget {
                    view: &target.create_view(&Default::default()),
                    viewport: [64., 64.],
                    clear: wgpu::Color::BLACK,
                    initial: None
                }
            )
            .is_err()
    );
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "no WebGPU validation errors: {error:?}");
}

#[test]
fn unchanged_legacy_renderer_is_a_pixel_baseline() {
    use gpu_utils::texture_atlas::TextureAtlas;
    let gpu = futures::executor::block_on(Gpu::new(gpu_descriptor(true)))
        .expect("legacy-capable real GPU");
    let (device, queue) = gpu.context().expect("GPU");
    let size = wgpu::Extent3d {
        width: 128,
        height: 128,
        depth_or_array_layers: 1,
    };
    let colors = TextureAtlas::new(&device, size, wgpu::TextureFormat::Rgba8UnormSrgb, 1);
    let masks = TextureAtlas::new(&device, size, wgpu::TextureFormat::R8Unorm, 1);
    let rgba = [180, 65, 32, 200];
    let color_region = colors
        .allocate(&device, &queue, [1, 1])
        .expect("legacy color");
    color_region
        .write_data(&queue, &rgba)
        .expect("color upload");
    let bitmap: Vec<u8> = (0..64).map(|i| ((i % 8) * 32) as u8).collect();
    let mask_region = masks
        .allocate(&device, &queue, [8, 8])
        .expect("legacy mask");
    mask_region
        .write_data(&queue, &bitmap)
        .expect("mask upload");
    let clip_region = masks
        .allocate(&device, &queue, [1, 1])
        .expect("legacy clip");
    clip_region.write_data(&queue, &[255]).expect("clip upload");
    let old_node = renderer::RenderNode::new()
        .with_texture(color_region, [36., 30.], Matrix4::identity())
        .with_stencil(mask_region, [36., 30.], Matrix4::identity());
    let transform = Matrix4::new_translation(&nalgebra::Vector3::new(8., 12., 0.));
    let old_target = output(&device);
    renderer::CoreRenderer::new(&device)
        .render_flat(
            &device,
            &queue,
            wgpu::TextureFormat::Rgba8Unorm,
            &old_target.create_view(&Default::default()),
            [64., 64.],
            &[renderer::FlatItem::new(Arc::new(old_node), transform)
                .with_clip(Some(0))
                .with_alpha(0.6)],
            &[renderer::MaskNode {
                parent: None,
                transform: rect(12., 4., 36., 42.),
                region: clip_region,
            }],
            wgpu::Color::BLACK,
            &colors.texture(),
            &masks.texture(),
        )
        .expect("unchanged main renderer");
    let old_pixels = pixels(&device, &queue, &old_target);
    let mut scene = Scene::default();
    let mesh = scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("quad");
    let texture = scene
        .resources
        .insert_texture(sources::rgba([1, 1], rgba.to_vec()))
        .expect("color");
    let mask = scene
        .resources
        .insert_mask(sources::coverage([8, 8], bitmap))
        .expect("gradient");
    let clip = scene
        .resources
        .insert_mask(sources::coverage([1, 1], vec![255]))
        .expect("clip");
    scene.pixel_masks = vec![
        PixelMask {
            mesh,
            texture: clip,
            transform: rect(12., 4., 36., 42.),
            parent: None,
        },
        PixelMask {
            mesh,
            texture: mask,
            transform: transform * rect(0., 0., 36., 30.),
            parent: Some(PixelMaskIndex(0)),
        },
    ];
    scene.phases.push(Phase {
        objects: vec![Object {
            mask: Some(PixelMaskIndex(1)),
            opacity: 0.6,
            ..Object::new(mesh, texture, transform * rect(0., 0., 36., 30.))
        }],
    });
    let new_target = output(&device);
    let mut renderer = SceneRenderer::new(&device, &queue);
    render(&mut renderer, &scene, &new_target, [64., 64.]);
    let new_pixels = pixels(&device, &queue, &new_target);
    let max = old_pixels
        .iter()
        .zip(&new_pixels)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .expect("pixels");
    let changed = old_pixels
        .chunks_exact(4)
        .zip(new_pixels.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    eprintln!("Legacy baseline: max channel delta {max}, changed pixels {changed}/4096");
    assert!(
        max <= 2,
        "legacy pixel parity exceeded two quantization steps: {max}"
    );
}

#[test]
fn atlas_pages_relocation_reuse_and_regeneration_preserve_content_ids() {
    use renderer::scene_renderer::AtlasConfig;
    let gpu = futures::executor::block_on(Gpu::new(gpu_descriptor(false))).expect("real GPU");
    let (device, queue) = gpu.context().expect("GPU");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut backend = SceneRenderer::new(&device, &queue);
    backend
        .set_atlas_config(AtlasConfig {
            texture_edge: 16,
            mesh_page_bytes: 512,
        })
        .expect("small stress arenas");
    let target = output(&device);
    let mut scene = Scene::default();
    let generated = Arc::new(AtomicUsize::new(0));
    let mut expected = vec![0u8; 64 * 64 * 4];
    scene.phases.push(Phase::default());
    for i in 0..64u32 {
        let original = sources::unit_quad();
        let count = generated.clone();
        let mesh = scene
            .resources
            .insert_mesh(MeshSource::new(*original.descriptor(), move |c| {
                count.fetch_add(1, Ordering::SeqCst);
                original.prepare(c)
            }))
            .expect("unique mesh");
        let rgba = [
            (i * 37 % 256) as u8,
            (i * 73 % 256) as u8,
            (i * 113 % 256) as u8,
            255,
        ];
        let size = if i == 63 {
            [65, 33]
        } else {
            [3 + i % 5, 2 + i % 3]
        };
        let count = generated.clone();
        let texture = scene
            .resources
            .insert_texture(TextureSource::new(
                TextureDescriptor::new(size, wgpu::TextureFormat::Rgba8Unorm),
                move |mut c| {
                    count.fetch_add(1, Ordering::SeqCst);
                    let mut bytes = Vec::new();
                    for _ in 0..size[0] * size[1] {
                        bytes.extend_from_slice(&rgba);
                    }
                    upload_texture(&mut c.gpu, &c.target, &bytes)
                },
            ))
            .expect("unique texture");
        scene.phases[0].objects.push(Object::new(
            mesh,
            texture,
            rect((i % 8 * 8) as f32, (i / 8 * 8) as f32, 8., 8.),
        ));
        for y in i / 8 * 8..i / 8 * 8 + 8 {
            for x in i % 8 * 8..i % 8 * 8 + 8 {
                let at = (y * 64 + x) as usize * 4;
                expected[at..at + 4].copy_from_slice(&rgba);
            }
        }
    }
    scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([8, 8], wgpu::TextureFormat::Rgba8Unorm),
            |_| panic!("pool-only source must remain lazy"),
        ))
        .expect("unused candidate");
    let snapshot_calls = Arc::new(AtomicUsize::new(0));
    let calls = snapshot_calls.clone();
    let copy = scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([64, 64], wgpu::TextureFormat::Rgba16Float),
            move |c| {
                calls.fetch_add(1, Ordering::SeqCst);
                c.gpu.encoder.copy_texture_to_texture(
                    c.gpu.snapshot.color_texture.as_image_copy(),
                    c.target.texture.as_image_copy(),
                    c.target.texture.size(),
                );
                Ok(())
            },
        ))
        .expect("snapshot source");
    let quad = scene.phases[0].objects[0].mesh;
    scene.phases.push(Phase {
        objects: vec![Object::new(quad, copy, rect(0., 0., 64., 64.))],
    });
    render(&mut backend, &scene, &target, [64., 64.]);
    let before = pixels(&device, &queue, &target);
    assert!(
        before
            .iter()
            .zip(&expected)
            .all(|(a, b)| a.abs_diff(*b) <= 1),
        "no inter-region filter bleed, including oversized fallback"
    );
    let small = backend.stats().placement;
    assert!(
        small.texture_pages > 3 && small.mesh_pages > 3,
        "stress must force several pages: {small:?}"
    );
    assert_eq!(generated.load(Ordering::SeqCst), 128);
    // Relocate without waiting for prior frames; queue order protects old reads.
    for _ in 0..3 {
        render(&mut backend, &scene, &target, [64., 64.]);
    }
    let moved = backend
        .compact_resources(AtlasConfig {
            texture_edge: 128,
            mesh_page_bytes: 8192,
        })
        .expect("GPU-only relocation");
    render(&mut backend, &scene, &target, [64., 64.]);
    assert_eq!(before, pixels(&device, &queue, &target));
    assert_eq!(generated.load(Ordering::SeqCst), 128);
    assert_eq!(
        snapshot_calls.load(Ordering::SeqCst),
        1,
        "relocation cannot resample a phase"
    );
    assert_eq!(backend.stats().prepared, 0);
    assert_eq!(backend.stats().snapshot_copies, 0);
    assert!(moved.placement.texture_pages < small.texture_pages);
    assert!(moved.placement.mesh_pages < small.mesh_pages);
    // Eviction removes lease ownership, not Scene definitions. Empty pages drop.
    backend.set_cache_budget(0);
    render(&mut backend, &Scene::default(), &target, [64., 64.]);
    assert_eq!(backend.stats().placement.texture_pages, 0);
    assert_eq!(backend.stats().placement.mesh_pages, 0);
    render(&mut backend, &scene, &target, [64., 64.]);
    assert_eq!(before, pixels(&device, &queue, &target));
    assert_eq!(generated.load(Ordering::SeqCst), 256);
    assert_eq!(snapshot_calls.load(Ordering::SeqCst), 2);
    // Retire half the allocations while the remaining half keeps shared pages
    // alive, then refill with new IDs. Every live tile retains its own colour.
    scene.phases.truncate(1);
    let removed = scene.phases[0].objects.split_off(32);
    render(&mut backend, &scene, &target, [64., 64.]);
    assert!(backend.stats().evicted >= 64);
    for (n, old) in removed.iter().enumerate() {
        let rgba = [255, 255 - (n * 7) as u8, (n * 5) as u8, 255];
        let texture = scene
            .resources
            .insert_texture(TextureSource::new(
                TextureDescriptor::new([4, 3], wgpu::TextureFormat::Rgba8Unorm),
                move |mut c| upload_texture(&mut c.gpu, &c.target, &rgba.repeat(12)),
            ))
            .expect("fresh refill");
        scene.phases[0]
            .objects
            .push(Object::new(old.mesh, texture, old.transform));
        let tile = n + 32;
        for y in tile / 8 * 8..tile / 8 * 8 + 8 {
            for x in tile % 8 * 8..tile % 8 * 8 + 8 {
                let at = (y * 64 + x) * 4;
                expected[at..at + 4].copy_from_slice(&rgba);
            }
        }
    }
    render(&mut backend, &scene, &target, [64., 64.]);
    let after = pixels(&device, &queue, &target);
    assert!(
        after
            .iter()
            .zip(&expected)
            .all(|(a, b)| a.abs_diff(*b) <= 1),
        "reused regions may not overwrite still-live resources"
    );
    eprintln!(
        "Atlas stress: small={small:?}; relocation={moved:?}; refill={:?}",
        backend.stats()
    );
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn diagnostic_snapshot_content_identity_must_be_updated_by_the_caller() {
    let gpu = futures::executor::block_on(Gpu::new(gpu_descriptor(false))).expect("real GPU");
    let (device, queue) = gpu.context().expect("GPU");
    let mut backend = SceneRenderer::new(&device, &queue);
    let target = output(&device);
    let mut scene = Scene::default();
    let mesh = scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("quad");
    let red = color(&mut scene, [255, 0, 0, 255]);
    let blue = color(&mut scene, [0, 0, 255, 255]);
    let copy = || {
        TextureSource::new(
            TextureDescriptor::new([64, 64], wgpu::TextureFormat::Rgba16Float),
            |c| {
                c.gpu.encoder.copy_texture_to_texture(
                    c.gpu.snapshot.color_texture.as_image_copy(),
                    c.target.texture.as_image_copy(),
                    c.target.texture.size(),
                );
                Ok(())
            },
        )
    };
    let texture = scene
        .resources
        .insert_texture(copy())
        .expect("snapshot source");
    scene.phases = vec![
        Phase {
            objects: vec![Object::new(mesh, red, rect(0., 0., 64., 64.))],
        },
        Phase {
            objects: vec![Object::new(mesh, texture, rect(0., 0., 64., 64.))],
        },
    ];
    render(&mut backend, &scene, &target, [64., 64.]);
    close(
        pixel(&pixels(&device, &queue, &target), 20, 20),
        [255, 0, 0, 255],
    );
    // Deliberate contract violation: background changes but its consumer's ID
    // does not. This diagnostic records the observable ergonomic failure mode.
    scene.phases[0].objects[0].texture = blue;
    render(&mut backend, &scene, &target, [64., 64.]);
    close(
        pixel(&pixels(&device, &queue, &target), 20, 20),
        [255, 0, 0, 255],
    );
    backend.clear_cache();
    render(&mut backend, &scene, &target, [64., 64.]);
    close(
        pixel(&pixels(&device, &queue, &target), 20, 20),
        [0, 0, 255, 255],
    );
    // The supported construction uses a fresh immutable-content definition.
    scene.phases[1].objects[0].texture = scene
        .resources
        .insert_texture(copy())
        .expect("new content ID");
    render(&mut backend, &scene, &target, [64., 64.]);
    close(
        pixel(&pixels(&device, &queue, &target), 20, 20),
        [0, 0, 255, 255],
    );
    eprintln!(
        "Design diagnostic: reusing a background-dependent ID produced warm/cold disagreement; fresh ID restored the invariant."
    );
}

#[test]
fn diagnostic_gpu_validation_at_finish_does_not_wait_for_execution() {
    // Native wgpu 29 records this copy first, then validates its formats at finish.
    // Observe the handler synchronously: a Future-returning scope API alone would
    // not tell us when validation happened. No queue submission/poll is needed.
    let gpu = futures::executor::block_on(Gpu::new(gpu_descriptor(false))).expect("real GPU");
    let (device, _queue) = gpu.context().expect("GPU");
    let errors = Arc::new(AtomicUsize::new(0));
    let observed = errors.clone();
    device.on_uncaptured_error(Arc::new(move |error| {
        assert!(matches!(error, wgpu::Error::Validation { .. }));
        observed.fetch_add(1, Ordering::SeqCst);
    }));
    let texture = |format| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("validation timing diagnostic"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    };
    let source = texture(wgpu::TextureFormat::Rgba16Float);
    let target = texture(wgpu::TextureFormat::Rgba8Unorm);
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_texture(
        source.as_image_copy(),
        target.as_image_copy(),
        target.size(),
    );
    assert_eq!(errors.load(Ordering::SeqCst), 0);
    let invalid_commands = encoder.finish();
    assert_eq!(errors.load(Ordering::SeqCst), 1);
    drop(invalid_commands);
    eprintln!(
        "Native validation timing: copy returned with 0 errors; finish returned with 1 validation error already delivered; no submit/poll."
    );
}

#[test]
fn diagnostic_gpu_validation_is_distinct_from_prepare_result() {
    let gpu = futures::executor::block_on(Gpu::new(gpu_descriptor(false))).expect("real GPU");
    let (device, queue) = gpu.context().expect("GPU");
    let mut backend = SceneRenderer::new(&device, &queue);
    let target = output(&device);
    let mut scene = Scene::default();
    let mesh = scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("mesh");
    let texture = scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([64, 64], wgpu::TextureFormat::Rgba8Unorm),
            |c| {
                // Invalid format conversion via Copy (RGBA16Float -> RGBA8Unorm).
                // A valid conversion must use Render/Compute, not this copy command.
                c.gpu.encoder.copy_texture_to_texture(
                    c.gpu.snapshot.color_texture.as_image_copy(),
                    c.target.texture.as_image_copy(),
                    c.target.texture.size(),
                );
                Ok(())
            },
        ))
        .expect("invalid-command producer");
    scene.phases.push(Phase {
        objects: vec![Object::new(mesh, texture, rect(0., 0., 64., 64.))],
    });
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let cpu_result = backend.render(
        &scene,
        SceneTarget {
            view: &target.create_view(&Default::default()),
            viewport: [64., 64.],
            clear: wgpu::Color::BLACK,
            initial: None,
        },
    );
    let gpu_error = futures::executor::block_on(scope.pop());
    assert!(cpu_result.is_ok());
    assert!(gpu_error.is_some());
    backend.clear_cache();
    eprintln!(
        "Design diagnostic: callback Ok/CPU render Ok did not imply GPU validation success; wgpu error scope reported the invalid copy."
    );
}

#[test]
fn gpu_deformed_mesh_survives_packing_and_relocation() {
    let gpu = futures::executor::block_on(Gpu::new(gpu_descriptor(false))).expect("real GPU");
    let (device, queue) = gpu.context().expect("GPU");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut backend = SceneRenderer::new(&device, &queue);
    backend
        .set_atlas_config(renderer::scene_renderer::AtlasConfig {
            texture_edge: 16,
            mesh_page_bytes: 256,
        })
        .expect("oversized mesh stress");
    let target = output(&device);
    let mut scene = Scene::default();
    let texture = color(&mut scene, [0, 255, 255, 255]);
    let clip_mesh = scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("clip quad");
    let mask = coverage(&mut scene, 255);
    scene.pixel_masks.push(PixelMask {
        mesh: clip_mesh,
        texture: mask,
        transform: rect(8., 0., 48., 64.),
        parent: None,
    });
    scene.phases.push(Phase::default());
    let mut frames = Vec::new();
    for phase in [0., 1.2, 2.4] {
        let mesh = scene
            .resources
            .insert_mesh(effects::ribbon(256, phase))
            .expect("fresh animated mesh");
        scene.phases[0].objects.clear();
        scene.phases[0].objects.push(Object {
            mask: Some(PixelMaskIndex(0)),
            ..Object::new(mesh, texture, rect(0., 0., 64., 64.))
        });
        render(&mut backend, &scene, &target, [64., 64.]);
        let bytes = pixels(&device, &queue, &target);
        assert!(bytes.chunks_exact(4).filter(|p| p[1] > 200).count() > 150);
        for y in 0..64 {
            assert_eq!(pixel(&bytes, 2, y), [0, 0, 0, 255]);
        }
        frames.push(bytes);
    }
    assert_ne!(frames[0], frames[1]);
    assert_ne!(frames[1], frames[2]);
    let moved = backend
        .compact_resources(renderer::scene_renderer::AtlasConfig {
            texture_edge: 32,
            mesh_page_bytes: 128 * 1024,
        })
        .expect("relocate generated geometry");
    render(&mut backend, &scene, &target, [64., 64.]);
    assert_eq!(frames[2], pixels(&device, &queue, &target));
    assert_eq!(backend.stats().prepared, 0);
    eprintln!(
        "GPU animated mesh: 1536 generated vertices/frame, clipping and relocation exact; {moved:?}"
    );
    // Equal size AND usages must still lease distinct simultaneous logical
    // outputs. Reusing the same scratch buffer here would corrupt vertices.
    let mut desc = MeshDescriptor::triangles(3, 15);
    desc.usages = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX;
    let mesh = scene
        .resources
        .insert_mesh(MeshSource::new(desc, |mut c| {
            let indices = c.target.indices.expect("indexed source");
            assert_ne!(c.target.vertices, indices);
            let vertices = [
                Vertex {
                    position: [0., 0., 0.],
                    uv: [0., 0.],
                },
                Vertex {
                    position: [1., 0., 0.],
                    uv: [1., 0.],
                },
                Vertex {
                    position: [0., 1., 0.],
                    uv: [0., 1.],
                },
            ];
            upload_buffer(
                &mut c.gpu,
                c.target.vertices,
                bytemuck::cast_slice(&vertices),
            )?;
            upload_buffer(
                &mut c.gpu,
                indices,
                bytemuck::cast_slice(&[0u32, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2]),
            )
        }))
        .expect("two simultaneous outputs");
    scene.phases[0].objects = vec![Object::new(mesh, texture, rect(0., 0., 64., 64.))];
    render(&mut backend, &scene, &target, [64., 64.]);
    let bytes = pixels(&device, &queue, &target);
    close(pixel(&bytes, 4, 4), [0, 255, 255, 255]);
    close(pixel(&bytes, 55, 55), [0, 0, 0, 255]);
    assert_eq!(backend.stats().output_buffer_allocations, 2);
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_generator_can_render_private_3d_with_depth_then_join_ui_composition() {
    let gpu = futures::executor::block_on(Gpu::new(gpu_descriptor(false))).expect("real GPU");
    let (device, queue) = gpu.context().expect("GPU");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut backend = SceneRenderer::new(&device, &queue);
    let target = output(&device);
    let mut scene = Scene::default();
    let mesh = scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("quad");
    let background = color(&mut scene, [25, 30, 40, 255]);
    let cube = scene
        .resources
        .insert_texture(private_3d::cube([64, 64], 0.4))
        .expect("private 3D source");
    scene.phases.push(Phase {
        objects: vec![
            Object::new(mesh, background, rect(0., 0., 64., 64.)),
            Object::new(mesh, cube, rect(0., 0., 64., 64.)),
        ],
    });
    render(&mut backend, &scene, &target, [64., 64.]);
    let before = pixels(&device, &queue, &target);
    close(pixel(&before, 0, 0), [25, 30, 40, 255]);
    assert!(
        before
            .chunks_exact(4)
            .filter(|p| p[0] > 70 || p[1] > 70 || p[2] > 70)
            .count()
            > 500,
        "3D interior is visible"
    );
    backend
        .compact_resources(renderer::scene_renderer::AtlasConfig {
            texture_edge: 128,
            mesh_page_bytes: 4096,
        })
        .expect("relocate rendered texture");
    render(&mut backend, &scene, &target, [64., 64.]);
    assert_eq!(before, pixels(&device, &queue, &target));
    assert_eq!(backend.stats().prepared, 0);
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn diagnostic_pixel_art_needs_extra_geometry_with_the_fixed_linear_sampler() {
    let gpu = futures::executor::block_on(Gpu::new(gpu_descriptor(false))).expect("real GPU");
    let (device, queue) = gpu.context().expect("GPU");
    let mut backend = SceneRenderer::new(&device, &queue);
    let target = output(&device);
    let mut scene = Scene::default();
    let texture = scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([2, 1], wgpu::TextureFormat::Rgba8Unorm),
            |mut c| upload_texture(&mut c.gpu, &c.target, &[255, 0, 0, 255, 0, 0, 255, 255]),
        ))
        .expect("two texels");
    let quad = scene
        .resources
        .insert_mesh(sources::unit_quad())
        .expect("quad");
    scene.phases.push(Phase {
        objects: vec![Object::new(quad, texture, rect(0., 0., 64., 64.))],
    });
    render(&mut backend, &scene, &target, [64., 64.]);
    let linear = pixels(&device, &queue, &target);
    let boundary = pixel(&linear, 32, 32);
    assert!(
        boundary[0] > 100 && boundary[2] > 100,
        "linear interpolation blends the two texels"
    );
    // Existing ABI can express nearest-looking pixels using constant UV per
    // texel quad, but that scales to six vertices per texel instead of a sampler
    // choice. This is an explicit design-feedback experiment, not a new policy.
    let mesh = scene
        .resources
        .insert_mesh(MeshSource::new(
            MeshDescriptor::triangles(12, 0),
            |mut c| {
                let mut vertices = Vec::new();
                for (left, right, u) in [(0., 0.5, 0.25), (0.5, 1., 0.75)] {
                    for [x, y] in [
                        [left, 0.],
                        [left, 1.],
                        [right, 1.],
                        [left, 0.],
                        [right, 1.],
                        [right, 0.],
                    ] {
                        vertices.push(Vertex {
                            position: [x, y, 0.],
                            uv: [u, 0.5],
                        });
                    }
                }
                upload_buffer(
                    &mut c.gpu,
                    c.target.vertices,
                    bytemuck::cast_slice(&vertices),
                )
            },
        ))
        .expect("explicit pixel mesh");
    scene.phases[0].objects[0].mesh = mesh;
    render(&mut backend, &scene, &target, [64., 64.]);
    let discrete = pixels(&device, &queue, &target);
    for y in 0..64 {
        for x in 0..64 {
            assert_eq!(
                pixel(&discrete, x, y),
                if x < 32 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                }
            );
        }
    }
    eprintln!(
        "Design diagnostic: pixel art is expressible with texel geometry, but efficient nearest-vs-linear sampling needs an explicit drawing-semantic decision."
    );
}
