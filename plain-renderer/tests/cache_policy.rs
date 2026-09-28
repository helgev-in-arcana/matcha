//! GPU residency budgets and transient-output budgets have different lifetimes.
//! These proofs observe generated content, not private eviction-policy types.
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use plain_renderer::{PlainError, PlainRenderer, PlainTarget};
use render_interface::*;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicUsize, Ordering},
};

#[path = "../examples/support/sources.rs"]
#[allow(dead_code)]
mod sources;

fn gpu_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
fn gpu() -> Gpu {
    futures::executor::block_on(Gpu::new(GpuDescriptor {
        backends: match std::env::var("MATCHA_TEST_BACKEND").as_deref() {
            Ok("dx12") => wgpu::Backends::DX12,
            Ok("vulkan") => wgpu::Backends::VULKAN,
            _ => wgpu::Backends::PRIMARY,
        },
        required_features: wgpu::Features::empty(),
        ..Default::default()
    }))
    .expect("real GPU required for cache proofs")
}
fn output(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cache proof"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cache readback"),
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
    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
        result.expect("readback mapping")
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completion");
    buffer.slice(..).get_mapped_range().to_vec()
}
fn render(
    renderer: &mut PlainRenderer,
    scene: &Scene,
    target: &wgpu::Texture,
) -> Result<(), PlainError> {
    renderer.render(
        scene,
        PlainTarget {
            view: &target.create_view(&Default::default()),
            format: target.format(),
            viewport: [64., 64.],
            clear: wgpu::Color::BLACK,
            initial: None,
        },
    )
}
fn rect(x: f32, y: f32, w: f32, h: f32) -> Matrix4<f32> {
    Matrix4::new_translation(&nalgebra::Vector3::new(x, y, 0.))
        * Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(w, h, 1.))
}
fn texture(size: [u32; 2], rgba: [u8; 4], calls: &Arc<AtomicUsize>) -> TextureSource {
    let calls = calls.clone();
    TextureSource::new(
        TextureDescriptor::new(size, wgpu::TextureFormat::Rgba8Unorm),
        move |mut ctx| {
            calls.fetch_add(1, Ordering::SeqCst);
            upload_texture(
                &mut ctx.gpu,
                &ctx.target,
                &rgba.repeat((size[0] * size[1]) as usize),
            )
        },
    )
}
fn scene(mesh: &MeshSource, definitions: &[&TextureSource], draws: &[TextureId]) -> Scene {
    let mut scene = Scene::default();
    scene.resources.share_mesh(mesh).expect("mesh definition");
    for texture in definitions {
        scene
            .resources
            .share_texture(texture)
            .expect("texture definition");
    }
    scene.phases = draws
        .iter()
        .map(|texture| Phase {
            objects: vec![Object::new(mesh.id(), *texture, rect(0., 0., 64., 64.))],
        })
        .collect();
    scene
}

#[test]
fn least_recent_use_is_evicted_but_pool_presence_is_a_soft_retention_hint() {
    let _serial = gpu_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = output(&device);
    // Run both variants with fresh residency: equal hints choose the oldest
    // actual use; a retained definition can outweigh a newer absent definition.
    for keep_old_hint_only in [false, true] {
        let mut backend = PlainRenderer::new(&device, &queue);
        let mesh = sources::unit_quad();
        let counts: [Arc<AtomicUsize>; 3] = std::array::from_fn(|_| Arc::new(AtomicUsize::new(0)));
        let a = texture([8, 8], [255, 0, 0, 255], &counts[0]);
        let b = texture([8, 8], [0, 255, 0, 255], &counts[1]);
        let c = texture([8, 8], [0, 0, 255, 255], &counts[2]);
        // One quad (120 bytes) and exactly two color images (256 bytes each).
        backend.set_cache_budget(120 + 2 * 256);
        let all = [&a, &b, &c];
        render(&mut backend, &scene(&mesh, &all, &[a.id()]), &target).expect("first use");
        assert_eq!(
            counts[1].load(Ordering::SeqCst),
            0,
            "registering b does not generate it"
        );
        assert_eq!(
            counts[2].load(Ordering::SeqCst),
            0,
            "registering c does not generate it"
        );
        render(&mut backend, &scene(&mesh, &all, &[b.id()]), &target).expect("second use");
        let pressure_definitions: Vec<_> = if keep_old_hint_only {
            vec![&a, &c]
        } else {
            all.to_vec()
        };
        render(
            &mut backend,
            &scene(&mesh, &pressure_definitions, &[c.id()]),
            &target,
        )
        .expect("pressure");
        assert_eq!(backend.stats().evicted, 1);
        assert_eq!(backend.stats().cache_bytes, 120 + 2 * 256);
        let (survivor, victim, victim_calls) = if keep_old_hint_only {
            (&a, &b, &counts[1])
        } else {
            (&b, &a, &counts[0])
        };
        render(&mut backend, &scene(&mesh, &all, &[survivor.id()]), &target)
            .expect("surviving cache entry");
        assert_eq!(
            backend.stats().prepared,
            0,
            "correct entry survived pressure"
        );
        render(&mut backend, &scene(&mesh, &all, &[victim.id()]), &target)
            .expect("evicted entry regenerates");
        assert_eq!(victim_calls.load(Ordering::SeqCst), 2);
        let expected = if keep_old_hint_only {
            [0, 255, 0, 255]
        } else {
            [255, 0, 0, 255]
        };
        assert!(
            pixels(&device, &queue, &target)
                .chunks_exact(4)
                .all(|p| p == expected)
        );
        // Even a submitted pool containing all definitions does not pin entries
        // when no Object references them and no budget remains.
        backend.set_cache_budget(0);
        assert_eq!(
            backend.stats().over_budget_bytes,
            backend.stats().cache_bytes,
            "changing the budget updates statistics before the next render"
        );
        render(&mut backend, &scene(&mesh, &all, &[]), &target).expect("hints are not pins");
        assert_eq!(backend.stats().cache_bytes, 0);
        assert_eq!(backend.stats().placement, Default::default());
    }
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn zero_budget_protects_the_whole_frame_reference_set_across_phases() {
    let _serial = gpu_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = output(&device);
    let mut backend = PlainRenderer::new(&device, &queue);
    backend.set_cache_budget(0);
    let mesh = sources::unit_quad();
    let calls_a = Arc::new(AtomicUsize::new(0));
    let calls_b = Arc::new(AtomicUsize::new(0));
    let a = texture([8, 8], [90, 120, 180, 255], &calls_a);
    let b = texture([8, 8], [180, 50, 20, 255], &calls_b);
    let scene = scene(&mesh, &[&a, &b], &[a.id(), b.id(), a.id()]);
    render(&mut backend, &scene, &target).expect("required working set exceeds soft budget");
    assert_eq!(backend.stats().prepared, 3);
    assert_eq!(backend.stats().cache_bytes, 120 + 2 * 256);
    assert_eq!(
        backend.stats().over_budget_bytes,
        backend.stats().cache_bytes
    );
    assert_eq!(
        calls_a.load(Ordering::SeqCst),
        1,
        "first/last phase share one generation"
    );
    assert_eq!(calls_b.load(Ordering::SeqCst), 1);
    let before = pixels(&device, &queue, &target);
    assert!(before.chunks_exact(4).all(|p| p == [90, 120, 180, 255]));
    render(&mut backend, &scene, &target).expect("required resources remain resident");
    assert_eq!(backend.stats().prepared, 0);
    render(&mut backend, &Scene::default(), &target).expect("no required working set");
    assert_eq!(backend.stats().evicted, 3);
    assert_eq!(backend.stats().cache_bytes, 0);
    assert_eq!(backend.stats().over_budget_bytes, 0);
    assert_eq!(backend.stats().placement, Default::default());
    render(&mut backend, &scene, &target).expect("same immutable IDs regenerate");
    assert_eq!(calls_a.load(Ordering::SeqCst), 2);
    assert_eq!(calls_b.load(Ordering::SeqCst), 2);
    assert_eq!(before, pixels(&device, &queue, &target));
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn sequential_equal_outputs_reuse_scratch_without_overwriting_prior_content() {
    let _serial = gpu_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = output(&device);
    let mut backend = PlainRenderer::new(&device, &queue);
    backend.set_scratch_budget(1024);
    let mesh = sources::unit_quad();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut scene = Scene::default();
    scene.resources.share_mesh(&mesh).expect("mesh");
    scene.phases.push(Phase::default());
    for i in 0..64 {
        let rgba = [i * 3, 255 - i * 2, i, 255];
        let texture = scene
            .resources
            .insert_texture(texture([8, 8], rgba, &calls))
            .expect("unique content");
        scene.phases[0].objects.push(Object::new(
            mesh.id(),
            texture,
            rect(f32::from(i % 8) * 8., f32::from(i / 8) * 8., 8., 8.),
        ));
    }
    render(&mut backend, &scene, &target).expect("same-descriptor outputs are sequential");
    assert_eq!(calls.load(Ordering::SeqCst), 64);
    assert_eq!(backend.stats().output_texture_allocations, 1);
    assert!(backend.stats().scratch_reuses >= 63);
    assert!(backend.stats().scratch_bytes <= 1024);
    let before = pixels(&device, &queue, &target);
    for y in 0..64 {
        for x in 0..64 {
            let i = (y / 8 * 8 + x / 8) as u8;
            let at = (y * 64 + x) * 4;
            assert_eq!(&before[at..at + 4], &[i * 3, 255 - i * 2, i, 255]);
        }
    }
    render(&mut backend, &scene, &target)
        .expect("warm resident content does not check out outputs");
    assert_eq!(backend.stats().prepared, 0);
    assert_eq!(backend.stats().output_texture_allocations, 0);
    assert_eq!(backend.stats().output_buffer_allocations, 0);
    assert_eq!(before, pixels(&device, &queue, &target));
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn varying_size_churn_bounds_retained_scratch_and_abort_clears_outstanding_outputs() {
    let _serial = gpu_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = output(&device);
    let mut backend = PlainRenderer::new(&device, &queue);
    const SCRATCH_BUDGET: u64 = 2048;
    backend.set_scratch_budget(SCRATCH_BUDGET);
    backend.set_cache_budget(0);
    let mesh = sources::unit_quad();
    let calls = Arc::new(AtomicUsize::new(0));
    for frame in 0..64u32 {
        let size = [1 + frame * 7 % 53, 1 + frame * 11 % 41];
        let rgba = [(frame * 3) as u8, (255 - frame * 2) as u8, frame as u8, 255];
        let source = texture(size, rgba, &calls);
        let scene = scene(&mesh, &[&source], &[source.id()]);
        render(&mut backend, &scene, &target).expect("changing logical dimensions");
        let stats = backend.stats();
        let image_bytes = u64::from(size[0]) * u64::from(size[1]) * 4;
        assert!(
            stats.scratch_bytes <= SCRATCH_BUDGET,
            "frame {frame}: {stats:?}"
        );
        assert!(
            stats.scratch_peak_bytes >= image_bytes,
            "peak includes active logical output"
        );
        assert_eq!(
            stats.cache_bytes,
            120 + image_bytes,
            "only required contents stay resident"
        );
        assert_eq!(stats.placement.live_texture_bytes, image_bytes);
        if frame % 8 == 0 || frame == 63 {
            assert!(
                pixels(&device, &queue, &target)
                    .chunks_exact(4)
                    .all(|p| p == rgba)
            );
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 64);
    let before = pixels(&device, &queue, &target);
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_callback = attempts.clone();
    let failing = TextureSource::new(
        TextureDescriptor::new([37, 29], wgpu::TextureFormat::Rgba8Unorm),
        move |mut ctx| {
            upload_texture(
                &mut ctx.gpu,
                &ctx.target,
                &[50, 100, 150, 255].repeat(37 * 29),
            )?;
            if attempts_callback.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("abandon a live output checkout".into())
            } else {
                Ok(())
            }
        },
    );
    let retry = scene(&mesh, &[&failing], &[failing.id()]);
    let error = render(&mut backend, &retry, &target);
    assert!(
        matches!(error, Err(PlainError::Prepare { .. })),
        "{error:?}"
    );
    assert!(backend.stats().scratch_bytes <= SCRATCH_BUDGET);
    assert_eq!(before, pixels(&device, &queue, &target));
    render(&mut backend, &retry, &target).expect("abort reconciled every checked-out output");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(
        pixels(&device, &queue, &target)
            .chunks_exact(4)
            .all(|p| p == [50, 100, 150, 255])
    );
    assert!(backend.stats().scratch_bytes <= SCRATCH_BUDGET);
    backend.set_scratch_budget(0);
    render(&mut backend, &Scene::default(), &target)
        .expect("empty working set releases retained outputs");
    assert_eq!(backend.stats().scratch_bytes, 0);
    assert_eq!(backend.stats().cache_bytes, 0);
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}
