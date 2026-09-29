//! Shared placement must preserve drawing semantics and transactional residency.
//! The dedicated path is an independent placement oracle; stripe and triangle
//! pixels additionally have direct expected values, so common compositor errors
//! cannot make the comparison vacuously pass.
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
    .expect("real GPU required for placement proofs")
}

fn output(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("placement proof"),
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
        label: Some("placement readback"),
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
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Matrix4<f32> {
    Matrix4::new_translation(&nalgebra::Vector3::new(x, y, 0.))
        * Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(width, height, 1.))
}

fn rgba(size: [u32; 2], value: [u8; 4], calls: &Arc<AtomicUsize>) -> TextureSource {
    let calls = calls.clone();
    TextureSource::new(
        TextureDescriptor::new(size, wgpu::TextureFormat::Rgba8Unorm),
        move |mut ctx| {
            calls.fetch_add(1, Ordering::SeqCst);
            upload_texture(
                &mut ctx.gpu,
                &ctx.target,
                &value.repeat((size[0] * size[1]) as usize),
            )
        },
    )
}

fn coverage(value: u8, calls: &Arc<AtomicUsize>) -> MaskSource {
    let calls = calls.clone();
    MaskSource::new(
        MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
        move |mut ctx| {
            calls.fetch_add(1, Ordering::SeqCst);
            upload_texture(&mut ctx.gpu, &ctx.target, &[value])
        },
    )
}

fn quad(calls: &Arc<AtomicUsize>) -> MeshSource {
    let original = sources::unit_quad();
    let calls = calls.clone();
    MeshSource::new(*original.descriptor(), move |ctx| {
        calls.fetch_add(1, Ordering::SeqCst);
        original.prepare(ctx)
    })
}

fn triangle(copies: u32, calls: &Arc<AtomicUsize>) -> MeshSource {
    let calls = calls.clone();
    MeshSource::new(
        MeshDescriptor::triangles(3 * copies, 3 * copies),
        move |mut ctx| {
            calls.fetch_add(1, Ordering::SeqCst);
            let triangle = [
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
            let vertices = triangle.repeat(copies as usize);
            let indices: Vec<u32> = (0..3 * copies).collect();
            upload_buffer(
                &mut ctx.gpu,
                ctx.target.vertices,
                bytemuck::cast_slice(&vertices),
            )?;
            upload_buffer(
                &mut ctx.gpu,
                ctx.target.indices.expect("indexed triangle"),
                bytemuck::cast_slice(&indices),
            )
        },
    )
}

fn pixel(bytes: &[u8], x: usize, y: usize) -> [u8; 4] {
    bytes[(y * 64 + x) * 4..(y * 64 + x + 1) * 4]
        .try_into()
        .expect("one RGBA pixel")
}

#[test]
fn shared_and_dedicated_placements_have_identical_pixels_and_lazy_regeneration() {
    let _serial = gpu_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut scene = Scene::default();
    let mesh = scene.resources.insert_mesh(quad(&calls)).expect("quad");
    scene.phases.push(Phase::default());
    // Every source occupies one texel. Adjacent regions of opposite colors
    // expose accidental sampling across packed region boundaries.
    for x in 0..64 {
        let color = if x % 2 == 0 {
            [255, 0, 0, 255]
        } else {
            [0, 255, 0, 255]
        };
        let texture = scene
            .resources
            .insert_texture(rgba([1, 1], color, &calls))
            .expect("stripe source");
        scene.phases[0]
            .objects
            .push(Object::new(mesh, texture, rect(x as f32, 0., 1., 8.)));
    }
    let oversized = scene
        .resources
        .insert_texture(rgba([65, 33], [35, 91, 173, 255], &calls))
        .expect("oversized image");
    scene.phases[0]
        .objects
        .push(Object::new(mesh, oversized, rect(0., 8., 64., 16.)));
    let red = scene
        .resources
        .insert_texture(rgba([1, 1], [128, 0, 0, 128], &calls))
        .expect("premultiplied red");
    scene.phases[0]
        .objects
        .push(Object::new(mesh, red, rect(0., 24., 64., 40.)));
    let triangle = scene
        .resources
        .insert_mesh(triangle(1, &calls))
        .expect("indexed geometry");
    let cyan = scene
        .resources
        .insert_texture(rgba([1, 1], [0, 255, 255, 255], &calls))
        .expect("cyan");
    let mask = scene
        .resources
        .insert_mask(coverage(128, &calls))
        .expect("half mask");
    scene.pixel_masks = vec![
        PixelMask {
            mesh,
            texture: mask,
            transform: rect(0., 24., 64., 40.),
            parent: None,
        },
        PixelMask {
            mesh: triangle,
            texture: mask,
            transform: rect(0., 24., 64., 40.),
            parent: Some(PixelMaskIndex(0)),
        },
    ];
    scene.phases[0].objects.push(Object {
        mask: Some(PixelMaskIndex(1)),
        opacity: 0.5,
        ..Object::new(triangle, cyan, rect(0., 24., 64., 40.))
    });
    scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            |_| panic!("registered but unreferenced source stays lazy"),
        ))
        .expect("unused definition");

    let target = output(&device);
    let mut dedicated = PlainRenderer::new(&device, &queue);
    dedicated.set_placement_mode(PlacementMode::Dedicated);
    render(&mut dedicated, &scene, &target).expect("dedicated oracle");
    let expected = pixels(&device, &queue, &target);
    let count = calls.load(Ordering::SeqCst);
    assert_eq!(dedicated.stats().prepared, count);
    let mut atlas = PlainRenderer::new(&device, &queue);
    atlas
        .set_atlas_config(AtlasConfig {
            texture_edge: 16,
            mesh_page_bytes: 128,
        })
        .expect("small pages");
    atlas.set_placement_mode(PlacementMode::Atlas);
    render(&mut atlas, &scene, &target).expect("atlas renderer");
    let first = pixels(&device, &queue, &target);
    assert_eq!(first, expected, "packing must not change final pixels");
    assert_eq!(calls.load(Ordering::SeqCst), count * 2);
    let placement = atlas.stats().placement;
    assert!(
        placement.texture_pages >= 3,
        "RGBA page, mask page, oversized page: {placement:?}"
    );
    assert!(
        placement.mesh_pages >= 2,
        "small vertex/index pages: {placement:?}"
    );
    assert!(placement.reserved_texture_bytes >= 65 * 33 * 4);
    for y in 0..8 {
        for x in 0..64 {
            assert_eq!(
                pixel(&first, x, y),
                if x % 2 == 0 {
                    [255, 0, 0, 255]
                } else {
                    [0, 255, 0, 255]
                },
                "stripe ({x}, {y})"
            );
        }
    }
    assert_eq!(pixel(&first, 32, 16), [35, 91, 173, 255]);
    assert_eq!(
        pixel(&first, 60, 60),
        [128, 0, 0, 255],
        "outside triangle retains translucent background"
    );
    let inside = pixel(&first, 4, 28);
    for (actual, expected) in inside.into_iter().zip([112u8, 32, 32, 255]) {
        assert!(
            actual.abs_diff(expected) <= 1,
            "nested coverage and opacity: {inside:?}"
        );
    }
    render(&mut atlas, &scene, &target).expect("warm atlas");
    assert_eq!(atlas.stats().prepared, 0);
    assert_eq!(calls.load(Ordering::SeqCst), count * 2);
    assert_eq!(first, pixels(&device, &queue, &target));
    atlas.clear_cache();
    render(&mut atlas, &Scene::default(), &target).expect("cleared placement");
    assert_eq!(atlas.stats().placement, Default::default());
    assert_eq!(atlas.stats().cache_bytes, 0);
    render(&mut atlas, &scene, &target).expect("regenerated atlas");
    assert_eq!(calls.load(Ordering::SeqCst), count * 3);
    assert_eq!(first, pixels(&device, &queue, &target));
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn aborted_shared_page_allocations_release_space_without_damaging_live_content() {
    for layout in [
        PrepareOutputLayout::WholeResource,
        PrepareOutputLayout::AnyRegion,
    ] {
        rollback_proof(false, layout);
    }
}

#[test]
fn aborted_new_pages_are_reclaimed_before_retrying_the_same_content_ids() {
    for layout in [
        PrepareOutputLayout::WholeResource,
        PrepareOutputLayout::AnyRegion,
    ] {
        rollback_proof(true, layout);
    }
}

fn rollback_proof(force_new_pages: bool, layout: PrepareOutputLayout) {
    let _serial = gpu_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut backend = PlainRenderer::new(&device, &queue);
    backend
        .set_atlas_config(AtlasConfig {
            texture_edge: 16,
            mesh_page_bytes: 256,
        })
        .expect("small pages");
    backend.set_placement_mode(PlacementMode::Atlas);
    let target = output(&device);
    let old_calls = Arc::new(AtomicUsize::new(0));
    let new_calls = Arc::new(AtomicUsize::new(0));
    let mut old = Scene::default();
    let mesh = old
        .resources
        .insert_mesh(quad(&old_calls).with_output_layout(layout))
        .expect("old mesh");
    let texture = old
        .resources
        .insert_texture(rgba([1, 1], [255, 0, 0, 255], &old_calls).with_output_layout(layout))
        .expect("old texture");
    let mask = old
        .resources
        .insert_mask(coverage(255, &old_calls).with_output_layout(layout))
        .expect("old mask");
    old.pixel_masks.push(PixelMask {
        mesh,
        texture: mask,
        transform: rect(0., 0., 64., 64.),
        parent: None,
    });
    old.phases.push(Phase {
        objects: vec![Object {
            mask: Some(PixelMaskIndex(0)),
            ..Object::new(mesh, texture, rect(0., 0., 64., 64.))
        }],
    });
    render(&mut backend, &old, &target).expect("old residency");
    let before = pixels(&device, &queue, &target);
    let old_placement = backend.stats().placement;
    assert_eq!(old_calls.load(Ordering::SeqCst), 3);

    let mut next = Scene::default();
    let mesh = next
        .resources
        .insert_mesh(
            triangle(if force_new_pages { 32 } else { 1 }, &new_calls).with_output_layout(layout),
        )
        .expect("new geometry");
    let texture = next
        .resources
        .insert_texture(
            rgba(
                if force_new_pages { [65, 33] } else { [1, 1] },
                [0, 255, 0, 255],
                &new_calls,
            )
            .with_output_layout(layout),
        )
        .expect("new image");
    let mask = next
        .resources
        .insert_mask(coverage(128, &new_calls).with_output_layout(layout))
        .expect("new mask");
    next.pixel_masks.push(PixelMask {
        mesh,
        texture: mask,
        transform: rect(0., 0., 64., 64.),
        parent: None,
    });
    next.phases.push(Phase {
        objects: vec![Object {
            mask: Some(PixelMaskIndex(0)),
            ..Object::new(mesh, texture, rect(0., 0., 64., 64.))
        }],
    });
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempt_counter = attempts.clone();
    let fallible = next
        .resources
        .insert_texture(
            TextureSource::new(
                TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
                move |mut ctx| {
                    upload_texture(&mut ctx.gpu, &ctx.target, &[0, 0, 255, 255])?;
                    if attempt_counter.fetch_add(1, Ordering::SeqCst) == 0 {
                        Err("discard all preceding atlas writes and allocations".into())
                    } else {
                        Ok(())
                    }
                },
            )
            .with_output_layout(layout),
        )
        .expect("fallible source");
    let final_quad = old.phases[0].objects[0].mesh;
    next.resources
        .share_mesh(
            old.resources
                .mesh(final_quad)
                .expect("resident old quad definition"),
        )
        .expect("reuse resident quad for final pass");
    next.phases.push(Phase {
        objects: vec![Object::new(final_quad, fallible, rect(0., 0., 64., 64.))],
    });
    let error = render(&mut backend, &next, &target);
    assert!(
        matches!(error, Err(PlainError::Prepare { .. })),
        "{error:?}"
    );
    assert_eq!(
        new_calls.load(Ordering::SeqCst),
        3,
        "all new resource kinds were prepared in phase zero"
    );
    assert_eq!(
        backend.stats().placement,
        old_placement,
        "abort restores page counts and allocated bytes"
    );
    assert_eq!(
        before,
        pixels(&device, &queue, &target),
        "aborted writes were never submitted"
    );
    render(&mut backend, &old, &target).expect("prior live contents survive region release");
    assert_eq!(old_calls.load(Ordering::SeqCst), 3);
    assert_eq!(before, pixels(&device, &queue, &target));
    render(&mut backend, &next, &target).expect("same IDs regenerate into reclaimed space");
    assert_eq!(new_calls.load(Ordering::SeqCst), 6);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(
        pixels(&device, &queue, &target)
            .chunks_exact(4)
            .all(|p| p == [0, 0, 255, 255])
    );
    render(&mut backend, &old, &target).expect("reuse did not overwrite old live allocations");
    assert_eq!(old_calls.load(Ordering::SeqCst), 3);
    assert_eq!(before, pixels(&device, &queue, &target));
    backend.clear_cache();
    render(&mut backend, &Scene::default(), &target).expect("release all page ownership");
    assert_eq!(backend.stats().placement, Default::default());
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}
