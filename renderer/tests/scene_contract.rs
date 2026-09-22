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
        .insert_mesh(matcha_paint::unit_quad())
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
        .insert_mesh(matcha_paint::unit_quad())
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
        .insert_mesh(matcha_paint::unit_quad())
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
        .insert_mesh(matcha_paint::unit_quad())
        .expect("fast quad");
    let original = matcha_paint::unit_quad();
    let mut desc = *original.descriptor();
    desc.non_overlapping = false;
    let general = projected
        .resources
        .insert_mesh(MeshSource::new(desc, move |c| original.prepare(c)))
        .expect("general quad");
    let green = color(&mut projected, [0, 255, 0, 255]);
    let gradient =
        matcha_paint::Bitmap::coverage([8, 8], (0..64).map(|i| (i % 8 * 32) as u8).collect())
            .expect("gradient")
            .register_mask(&mut projected.resources);
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
        .insert_mesh(matcha_paint::unit_quad())
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
    let node = matcha_paint::RenderNode::new()
        .with_texture(
            matcha_paint::Bitmap::rgba([1, 1], rgba.to_vec()).expect("color"),
            [36., 30.],
            Matrix4::identity(),
        )
        .with_stencil(
            matcha_paint::Bitmap::coverage([8, 8], bitmap).expect("mask"),
            [36., 30.],
            Matrix4::identity(),
        );
    let mut builder = matcha_paint::SceneBuilder::new();
    builder.begin();
    let clip = builder.push_clip(None, rect(12., 4., 36., 42.));
    builder.push(&node, transform, Some(clip), 0.6);
    builder.finish();
    let new_target = output(&device);
    let mut renderer = SceneRenderer::new(&device, &queue);
    render(&mut renderer, builder.scene(), &new_target, [64., 64.]);
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
