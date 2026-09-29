//! Shared mask ancestry should cost once per actual compositor operation, not
//! once per possible ancestor of every object. Reusable frame state must also
//! survive resource replacement and aborted preparation.
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use plain_renderer::{PlacementMode, PlainError, PlainRenderer, PlainTarget};
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
        // Deliberately rule out the former hundreds-of-MiB conservative arena.
        required_limits: Some(wgpu::Limits {
            max_buffer_size: 64 * 1024 * 1024,
            max_storage_buffer_binding_size: 64 * 1024 * 1024,
            ..Default::default()
        }),
        ..Default::default()
    }))
    .expect("real GPU required for frame workspace proofs")
}
fn output(device: &wgpu::Device, size: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame workspace proof"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
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
    let row_bytes = texture.width() * 4;
    let stride = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frame workspace readback"),
        size: u64::from(stride) * u64::from(texture.height()),
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
                bytes_per_row: Some(stride),
                rows_per_image: Some(texture.height()),
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
    buffer
        .slice(..)
        .get_mapped_range()
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..row_bytes as usize].iter().copied())
        .collect()
}
fn render(
    renderer: &mut PlainRenderer,
    scene: &Scene,
    target: &wgpu::Texture,
) -> Result<(), PlainError> {
    renderer.render(
        scene,
        PlainTarget {
            region: render_interface::TextureRegion::whole(
                &target.create_view(&Default::default()),
                target.format(),
            )
            .expect("whole output region"),
            logical_size: [16., 16.],
            clear: wgpu::Color::BLACK,
            initial: None,
        },
    )
}
const RGBA: [u8; 4] = [32, 96, 192, 255];

fn scene(depth: u32, objects: usize) -> Scene {
    let mut scene = Scene::default();
    let original = sources::unit_quad();
    let mut descriptor = *original.descriptor();
    // Exercise general mask rasterization rather than the coincident-UV shortcut.
    descriptor.non_overlapping = false;
    let mesh = scene
        .resources
        .insert_mesh(MeshSource::new(descriptor, move |ctx| {
            original.prepare(ctx)
        }))
        .expect("quad mesh");
    let color = scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            |mut ctx| upload_texture(&mut ctx.gpu, &ctx.target, &RGBA),
        ))
        .expect("solid color");
    let mask = scene
        .resources
        .insert_mask(MaskSource::new(
            MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
            |mut ctx| upload_texture(&mut ctx.gpu, &ctx.target, &[255]),
        ))
        .expect("full coverage");
    let transform = Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(16., 16., 1.));
    scene.pixel_masks = (0..depth)
        .map(|i| PixelMask {
            mesh,
            texture: mask,
            transform,
            parent: if i == 0 {
                None
            } else {
                Some(PixelMaskIndex(i - 1))
            },
        })
        .collect();
    scene.phases.push(Phase {
        objects: (0..objects)
            .map(|_| Object {
                mask: Some(PixelMaskIndex(depth - 1)),
                ..Object::new(mesh, color, transform)
            })
            .collect(),
    });
    scene
}
fn assert_color(device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::Texture) {
    assert!(
        pixels(device, queue, target)
            .chunks_exact(4)
            .all(|pixel| pixel == RGBA)
    );
}

#[test]
fn ten_thousand_objects_share_one_hundred_masks_with_a_small_uniform_arena() {
    let _serial = gpu_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut backend = PlainRenderer::new(&device, &queue);
    let target = output(&device, 16);
    const OBJECTS: usize = 10_000;
    const DEPTH: u32 = 100;
    let scene = scene(DEPTH, OBJECTS);
    let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment);
    let stride = 112u64.next_multiple_of(alignment);
    let old_capacity = stride * (2 + OBJECTS as u64 * (1 + 2 * u64::from(DEPTH)));
    assert!(
        old_capacity > device.limits().max_buffer_size,
        "the old conservative estimate must exceed the deliberately limited device"
    );
    let start = std::time::Instant::now();
    render(&mut backend, &scene, &target).expect("actual operations fit in the device limit");
    let cold = backend.stats();
    assert_eq!(cold.prepared, 3);
    assert_eq!(cold.draw_calls, OBJECTS);
    assert!(
        cold.mask_passes <= 2 * DEPTH as usize,
        "shared ancestry must be composed once"
    );
    assert!(
        cold.parameter_bytes <= stride * 10_201,
        "100 masks need at most 200 parameters plus 10000 objects and one final draw: {cold:?}"
    );
    assert!(
        cold.parameter_bytes <= 3 * 1024 * 1024,
        "compact arena: {cold:?}"
    );
    assert!(cold.bind_groups > 0);
    assert_color(&device, &queue, &target);
    let cold_elapsed = start.elapsed();
    let start = std::time::Instant::now();
    render(&mut backend, &scene, &target).expect("warm frame reuses its workspace");
    let warm = backend.stats();
    assert_eq!(warm.prepared, 0);
    assert_eq!(warm.bind_groups, 0, "warm frame creates no new bind groups");
    assert_eq!(warm.parameter_bytes, cold.parameter_bytes);
    assert_color(&device, &queue, &target);
    eprintln!(
        "10000 objects / 100 masks: old estimate={old_capacity} bytes, actual arena={} bytes, cold={cold_elapsed:?}, warm={:?}, cold groups={}, warm groups={}",
        cold.parameter_bytes,
        start.elapsed(),
        cold.bind_groups,
        warm.bind_groups
    );
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn workspace_and_bind_groups_recover_after_resize_cache_replacement_and_prepare_failure() {
    let _serial = gpu_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut backend = PlainRenderer::new(&device, &queue);
    let mut scene = scene(4, 8);
    let small = output(&device, 16);
    render(&mut backend, &scene, &small).expect("first frame");
    assert!(backend.stats().bind_groups > 0);
    render(&mut backend, &scene, &small).expect("warm frame");
    assert_eq!(backend.stats().bind_groups, 0);
    let target = output(&device, 32);
    render(&mut backend, &scene, &target).expect("resized working attachments");
    assert_eq!(backend.stats().prepared, 0);
    assert!(
        backend.stats().bind_groups > 0,
        "new working images need new bindings"
    );
    assert_color(&device, &queue, &target);
    backend.clear_cache();
    render(&mut backend, &scene, &target).expect("regenerated placements");
    assert_eq!(backend.stats().prepared, 3);
    assert!(backend.stats().bind_groups > 0);
    assert_color(&device, &queue, &target);
    backend.set_placement_mode(PlacementMode::Dedicated);
    render(&mut backend, &scene, &target).expect("new dedicated handles");
    assert_eq!(backend.stats().prepared, 3);
    assert!(backend.stats().bind_groups > 0);
    assert_color(&device, &queue, &target);
    backend
        .compact_resources()
        .expect("dedicated compaction is unnecessary");
    render(&mut backend, &scene, &target).expect("dedicated bindings remain reusable");
    assert_eq!(backend.stats().prepared, 0);
    assert_eq!(backend.stats().bind_groups, 0);
    assert_color(&device, &queue, &target);
    backend.set_placement_mode(PlacementMode::Atlas);
    render(&mut backend, &scene, &target).expect("new shared handles");
    assert_eq!(backend.stats().prepared, 3);
    backend
        .compact_resources()
        .expect("replacement shared handles");
    render(&mut backend, &scene, &target).expect("bindings follow relocated shared resources");
    assert_eq!(backend.stats().prepared, 0);
    assert!(backend.stats().bind_groups > 0);
    assert_color(&device, &queue, &target);
    render(&mut backend, &scene, &target).expect("new handles become warm");
    assert_eq!(backend.stats().bind_groups, 0);

    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    let texture = scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            move |mut ctx| {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("abort after taking the reusable frame workspace".into());
                }
                upload_texture(&mut ctx.gpu, &ctx.target, &RGBA)
            },
        ))
        .expect("fallible content");
    scene.phases[0].objects[0].texture = texture;
    let error = render(&mut backend, &scene, &target);
    assert!(
        matches!(error, Err(PlainError::Prepare { .. })),
        "{error:?}"
    );
    assert_color(&device, &queue, &target);
    render(&mut backend, &scene, &target).expect("workspace returned after abort");
    assert_eq!(backend.stats().prepared, 1);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_color(&device, &queue, &target);
    render(&mut backend, &scene, &target).expect("recovered workspace remains reusable");
    assert_eq!(backend.stats().prepared, 0);
    assert_eq!(backend.stats().bind_groups, 0);
    assert_color(&device, &queue, &target);
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}
