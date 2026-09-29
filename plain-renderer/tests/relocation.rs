//! Relocation copies immutable contents and changes placement transactionally.
//! No source callbacks or GPU waits are needed to move resident content.
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use plain_renderer::{AtlasConfig, PlacementMode, PlainError, PlainRenderer, PlainTarget};
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
    .expect("real GPU required for relocation proofs")
}
fn output(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("relocation proof"),
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
        label: Some("relocation readback"),
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
fn render(renderer: &mut PlainRenderer, scene: &Scene, target: &wgpu::Texture) {
    renderer
        .render(
            scene,
            PlainTarget {
                region: render_interface::TextureRegion::whole(
                    &target.create_view(&Default::default()),
                    target.format(),
                )
                .expect("whole output region"),
                viewport: [64., 64.],
                clear: wgpu::Color::BLACK,
                initial: None,
            },
        )
        .expect("valid relocation scene");
}
fn full_transform() -> Matrix4<f32> {
    Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(64., 64., 1.))
}

fn counted_scene(rgba: [u8; 4], calls: &Arc<AtomicUsize>) -> Scene {
    let original = sources::unit_quad();
    let counter = calls.clone();
    let mesh = MeshSource::new(*original.descriptor(), move |ctx| {
        counter.fetch_add(1, Ordering::SeqCst);
        original.prepare(ctx)
    });
    let mut scene = Scene::default();
    let mesh = scene.resources.insert_mesh(mesh).expect("unique mesh");
    let counter = calls.clone();
    let texture = scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([8, 8], wgpu::TextureFormat::Rgba8Unorm),
            move |mut ctx| {
                counter.fetch_add(1, Ordering::SeqCst);
                upload_texture(&mut ctx.gpu, &ctx.target, &rgba.repeat(64))
            },
        ))
        .expect("unique color");
    let counter = calls.clone();
    let mask = scene
        .resources
        .insert_mask(MaskSource::new(
            MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
            move |mut ctx| {
                counter.fetch_add(1, Ordering::SeqCst);
                upload_texture(&mut ctx.gpu, &ctx.target, &[255])
            },
        ))
        .expect("unique mask");
    scene.pixel_masks.push(PixelMask {
        mesh,
        texture: mask,
        transform: full_transform(),
        parent: None,
    });
    scene.phases.push(Phase {
        objects: vec![Object {
            mask: Some(PixelMaskIndex(0)),
            ..Object::new(mesh, texture, full_transform())
        }],
    });
    scene
}

#[test]
fn relocation_limit_is_atomic_and_exact_budget_moves_snapshot_dependent_content() {
    let _serial = gpu_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    for mode in [PlacementMode::Dedicated, PlacementMode::Atlas] {
        let mut backend = PlainRenderer::new(&device, &queue);
        backend
            .set_atlas_config(AtlasConfig {
                texture_edge: 16,
                mesh_page_bytes: 128,
            })
            .expect("small pages");
        backend.set_placement_mode(mode);
        let calls = Arc::new(AtomicUsize::new(0));
        let snapshot_calls = Arc::new(AtomicUsize::new(0));
        let mut scene = counted_scene([43, 91, 173, 255], &calls);
        let quad = scene.phases[0].objects[0].mesh;
        let color = scene.phases[0].objects[0].texture;
        // Include both vertex and index ranges in the byte-limit boundary.
        let counter = calls.clone();
        let triangle = scene
            .resources
            .insert_mesh(MeshSource::new(
                MeshDescriptor::triangles(3, 3),
                move |mut ctx| {
                    counter.fetch_add(1, Ordering::SeqCst);
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
                        &mut ctx.gpu,
                        ctx.target.vertices,
                        bytemuck::cast_slice(&vertices),
                    )?;
                    upload_buffer(
                        &mut ctx.gpu,
                        ctx.target.indices.expect("indexed triangle"),
                        bytemuck::cast_slice(&[0u32, 1, 2]),
                    )
                },
            ))
            .expect("indexed mesh");
        scene.phases[0]
            .objects
            .push(Object::new(triangle, color, full_transform()));
        let counter = snapshot_calls.clone();
        let snapshot = scene
            .resources
            .insert_texture(TextureSource::new(
                TextureDescriptor::new([64, 64], wgpu::TextureFormat::Rgba16Float),
                move |ctx| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    ctx.gpu.snapshot.color.copy_to(
                        ctx.gpu.encoder,
                        &ctx.target.region,
                        [0, 0],
                        [0, 0],
                        ctx.target.desc.size,
                    )?;
                    Ok(())
                },
            ))
            .expect("phase-dependent snapshot source");
        scene.phases.push(Phase {
            objects: vec![Object::new(quad, snapshot, full_transform())],
        });
        let target = output(&device);
        render(&mut backend, &scene, &target);
        let before = pixels(&device, &queue, &target);
        assert!(
            before
                .chunks_exact(4)
                .all(|pixel| pixel == [43, 91, 173, 255])
        );
        let original_stats = backend.stats();
        let bytes = 120 + 72 + 8 * 8 * 4 + 1 + 64 * 64 * 8;
        assert_eq!(original_stats.cache_bytes, bytes);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(snapshot_calls.load(Ordering::SeqCst), 1);
        match mode {
            PlacementMode::Atlas => {
                let error = backend.compact_resources_with_budget(bytes - 1);
                assert!(matches!(error, Err(PlainError::Invalid(_))), "{error:?}");
            }
            PlacementMode::Dedicated => {
                let unchanged = backend
                    .compact_resources_with_budget(0)
                    .expect("dedicated resources contain no shared-placement fragmentation");
                assert_eq!(unchanged.copied_bytes, 0);
                assert_eq!(unchanged.meshes, 0);
                assert_eq!(unchanged.textures, 0);
                assert_eq!(unchanged.placement, original_stats.placement);
                assert_eq!(
                    unchanged.peak_managed_bytes,
                    original_stats.placement.reserved_texture_bytes
                        + original_stats.placement.reserved_mesh_bytes
                );
            }
        }
        let after_failure = backend.stats();
        assert_eq!(after_failure.placement, original_stats.placement);
        assert_eq!(after_failure.cache_bytes, original_stats.cache_bytes);
        assert_eq!(after_failure.prepared, original_stats.prepared);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(snapshot_calls.load(Ordering::SeqCst), 1);
        assert_eq!(before, pixels(&device, &queue, &target));
        render(&mut backend, &scene, &target);
        assert_eq!(
            backend.stats().prepared,
            0,
            "declined or unnecessary relocation retained all old contents"
        );
        assert_eq!(before, pixels(&device, &queue, &target));

        let moved = backend
            .compact_resources_with_budget(bytes)
            .expect("exact logical copy budget");
        let old_capacity = original_stats.placement.reserved_texture_bytes
            + original_stats.placement.reserved_mesh_bytes;
        let new_capacity =
            moved.placement.reserved_texture_bytes + moved.placement.reserved_mesh_bytes;
        match mode {
            PlacementMode::Atlas => {
                assert_eq!(moved.copied_bytes, bytes);
                assert_eq!(moved.meshes, 2);
                assert_eq!(
                    moved.textures, 3,
                    "coverage also participates in texture relocation"
                );
                assert_eq!(moved.peak_managed_bytes, old_capacity + new_capacity);
            }
            PlacementMode::Dedicated => {
                assert_eq!(moved.copied_bytes, 0);
                assert_eq!(moved.meshes, 0);
                assert_eq!(moved.textures, 0);
                assert_eq!(moved.placement, original_stats.placement);
                assert_eq!(moved.peak_managed_bytes, old_capacity);
            }
        }
        render(&mut backend, &scene, &target);
        assert_eq!(backend.stats().prepared, 0);
        assert_eq!(backend.stats().snapshot_copies, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(
            snapshot_calls.load(Ordering::SeqCst),
            1,
            "relocation must not resample an earlier phase"
        );
        assert_eq!(
            before,
            pixels(&device, &queue, &target),
            "{mode:?} relocation preserves all pixels"
        );
    }
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn queue_order_preserves_old_frames_through_relocation_clear_and_new_allocations() {
    let _serial = gpu_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    for mode in [PlacementMode::Dedicated, PlacementMode::Atlas] {
        let mut backend = PlainRenderer::new(&device, &queue);
        backend
            .set_atlas_config(AtlasConfig {
                texture_edge: 16,
                mesh_page_bytes: 128,
            })
            .expect("small pages");
        backend.set_placement_mode(mode);
        let calls = Arc::new(AtomicUsize::new(0));
        let mut outputs = Vec::new();
        // No map, poll, readback or explicit GPU wait occurs in this loop.
        // Every later frame follows the relocation submission on the same Queue.
        for frame in 0..12u8 {
            let expected = [frame * 17, 255 - frame * 13, frame * 7, 255];
            let scene = counted_scene(expected, &calls);
            let before_relocation = output(&device);
            render(&mut backend, &scene, &before_relocation);
            let moved = backend
                .compact_resources()
                .expect("ordered relocation submission");
            match mode {
                PlacementMode::Atlas => {
                    assert_eq!(moved.meshes, 1);
                    assert_eq!(moved.textures, 2);
                }
                PlacementMode::Dedicated => {
                    assert_eq!(moved.meshes, 0);
                    assert_eq!(moved.textures, 0);
                    assert_eq!(moved.copied_bytes, 0);
                }
            }
            let after_relocation = output(&device);
            render(&mut backend, &scene, &after_relocation);
            assert_eq!(backend.stats().prepared, 0);
            outputs.push((before_relocation, expected));
            outputs.push((after_relocation, expected));
            backend.clear_cache();
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(frame + 1) * 3);
        }
        // Observe oldest outputs only after all later resource churn was submitted.
        for (index, (target, expected)) in outputs.iter().enumerate().rev() {
            let actual = pixels(&device, &queue, target);
            assert!(
                actual.chunks_exact(4).all(|pixel| pixel == expected),
                "{mode:?} output {index} was overwritten by later resource reuse"
            );
        }
    }
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}
