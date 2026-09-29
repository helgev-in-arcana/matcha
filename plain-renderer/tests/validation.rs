//! Invalid CPU input must fail before preparation. A failed preparation must
//! preserve submitted pixels and old residency while discarding new residency.
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use plain_renderer::{PlainError, PlainRenderer, PlainTarget};
use render_interface::*;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicUsize, Ordering},
};

#[path = "../examples/support/sources.rs"]
#[allow(dead_code)] // Other binaries use this fixture module's texture helpers.
mod sources;

fn gpu_test_lock() -> MutexGuard<'static, ()> {
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
    .expect("real GPU required for validation and rollback proofs")
}

fn output(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("validation proof"),
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
        label: Some("validation readback"),
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
        result.expect("validation readback mapping")
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completes");
    buffer.slice(..).get_mapped_range().to_vec()
}

fn render(
    renderer: &mut PlainRenderer,
    scene: &Scene,
    texture: &wgpu::Texture,
) -> Result<(), PlainError> {
    renderer.render(
        scene,
        PlainTarget {
            region: render_interface::TextureRegion::whole(
                &texture.create_view(&Default::default()),
                texture.format(),
            )
            .expect("whole output region"),
            viewport: [64., 64.],
            clear: wgpu::Color::BLACK,
            initial: None,
        },
    )
}

struct Fixture {
    scene: Scene,
    mesh: MeshSource,
    texture: TextureSource,
    mask: MaskSource,
    calls: Arc<[AtomicUsize; 3]>,
}

impl Fixture {
    fn new(rgba: [u8; 4]) -> Self {
        let calls = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
        let counts = calls.clone();
        let quad = sources::unit_quad();
        let mesh = MeshSource::new(*quad.descriptor(), move |ctx| {
            counts[0].fetch_add(1, Ordering::SeqCst);
            quad.prepare(ctx)
        });
        let counts = calls.clone();
        let texture = TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            move |mut ctx| {
                counts[1].fetch_add(1, Ordering::SeqCst);
                upload_texture(&mut ctx.gpu, &ctx.target, &rgba)
            },
        );
        let counts = calls.clone();
        let mask = MaskSource::new(
            MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
            move |mut ctx| {
                counts[2].fetch_add(1, Ordering::SeqCst);
                upload_texture(&mut ctx.gpu, &ctx.target, &[255])
            },
        );
        let mut scene = Scene::default();
        scene.resources.share_mesh(&mesh).expect("unique mesh");
        scene
            .resources
            .share_texture(&texture)
            .expect("unique texture");
        scene.resources.share_mask(&mask).expect("unique mask");
        let transform = Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(64., 64., 1.));
        scene.pixel_masks.push(PixelMask {
            mesh: mesh.id(),
            texture: mask.id(),
            transform,
            parent: None,
        });
        scene.phases.push(Phase {
            objects: vec![Object {
                mask: Some(PixelMaskIndex(0)),
                ..Object::new(mesh.id(), texture.id(), transform)
            }],
        });
        Self {
            scene,
            mesh,
            texture,
            mask,
            calls,
        }
    }

    fn counts(&self) -> [usize; 3] {
        std::array::from_fn(|i| self.calls[i].load(Ordering::SeqCst))
    }
}

#[test]
fn invalid_references_and_nonfinite_values_fail_before_any_generator_runs() {
    let _serial = gpu_test_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("initialized GPU");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = PlainRenderer::new(&device, &queue);
    let target = output(&device);
    let baseline = Fixture::new([255, 0, 0, 255]);
    render(&mut renderer, &baseline.scene, &target).expect("baseline");
    let before = pixels(&device, &queue, &target);
    for case in 0..12 {
        let mut fixture = Fixture::new([0, 255, 0, 255]);
        let object = &mut fixture.scene.phases[0].objects[0];
        let name = match case {
            0 => {
                object.mesh = MeshId::new();
                "missing object mesh"
            }
            1 => {
                object.texture = TextureId::new();
                "missing object texture"
            }
            2 => {
                object.mask = Some(PixelMaskIndex(1));
                "mask index outside arena"
            }
            3 => {
                fixture.scene.pixel_masks[0].texture = MaskId::new();
                "missing mask texture"
            }
            4 => {
                fixture.scene.pixel_masks[0].mesh = MeshId::new();
                "missing mask mesh"
            }
            5 => {
                fixture.scene.pixel_masks[0].parent = Some(PixelMaskIndex(0));
                "cyclic mask parent"
            }
            6 => {
                object.transform[(0, 0)] = f32::NAN;
                "NaN object transform"
            }
            7 => {
                object.transform[(1, 3)] = f32::INFINITY;
                "infinite object transform"
            }
            8 => {
                object.opacity = f32::NAN;
                "NaN opacity"
            }
            9 => {
                object.opacity = -0.01;
                "negative opacity"
            }
            10 => {
                object.opacity = 1.01;
                "opacity above one"
            }
            11 => {
                fixture.scene.pixel_masks[0].transform[(0, 3)] = f32::NEG_INFINITY;
                "infinite mask transform"
            }
            _ => unreachable!(),
        };
        let error = render(&mut renderer, &fixture.scene, &target);
        assert!(
            matches!(error, Err(PlainError::Invalid(_))),
            "{name}: {error:?}"
        );
        assert_eq!(
            fixture.counts(),
            [0; 3],
            "{name}: no preparation side effect"
        );
    }
    assert_eq!(
        before,
        pixels(&device, &queue, &target),
        "invalid frames never submit"
    );
    let error = futures::executor::block_on(validation.pop());
    assert!(
        error.is_none(),
        "CPU rejection must not reach GPU validation: {error:?}"
    );
}

#[test]
fn warm_cache_rejects_changed_descriptors_for_all_three_resource_kinds() {
    let _serial = gpu_test_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("initialized GPU");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = PlainRenderer::new(&device, &queue);
    let target = output(&device);
    let mut fixture = Fixture::new([255, 0, 0, 255]);
    render(&mut renderer, &fixture.scene, &target).expect("populate cache");
    let before = pixels(&device, &queue, &target);
    assert_eq!(fixture.counts(), [1; 3]);
    for kind in 0..3 {
        match kind {
            0 => {
                let mut descriptor = *fixture.mesh.descriptor();
                descriptor.vertex_count = 3;
                fixture
                    .scene
                    .resources
                    .retain_meshes(|id| id != fixture.mesh.id());
                fixture
                    .scene
                    .resources
                    .insert_mesh(MeshSource::with_id(fixture.mesh.id(), descriptor, |_| {
                        panic!("changed mesh must be rejected")
                    }))
                    .expect("replacement definition");
            }
            1 => {
                let mut descriptor = *fixture.texture.descriptor();
                descriptor.size = [2, 1];
                fixture
                    .scene
                    .resources
                    .retain_textures(|id| id != fixture.texture.id());
                fixture
                    .scene
                    .resources
                    .insert_texture(TextureSource::with_id(
                        fixture.texture.id(),
                        descriptor,
                        |_| panic!("changed texture must be rejected"),
                    ))
                    .expect("replacement definition");
            }
            2 => {
                let mut descriptor = *fixture.mask.descriptor();
                descriptor.size = [2, 1];
                fixture
                    .scene
                    .resources
                    .retain_masks(|id| id != fixture.mask.id());
                fixture
                    .scene
                    .resources
                    .insert_mask(MaskSource::with_id(fixture.mask.id(), descriptor, |_| {
                        panic!("changed mask must be rejected")
                    }))
                    .expect("replacement definition");
            }
            _ => unreachable!(),
        }
        let error = render(&mut renderer, &fixture.scene, &target);
        assert!(
            matches!(error, Err(PlainError::Invalid(_))),
            "kind {kind}: {error:?}"
        );
        assert_eq!(before, pixels(&device, &queue, &target));
        fixture.scene.resources.retain_meshes(|_| false);
        fixture.scene.resources.retain_textures(|_| false);
        fixture.scene.resources.retain_masks(|_| false);
        fixture
            .scene
            .resources
            .share_mesh(&fixture.mesh)
            .expect("restore mesh");
        fixture
            .scene
            .resources
            .share_texture(&fixture.texture)
            .expect("restore texture");
        fixture
            .scene
            .resources
            .share_mask(&fixture.mask)
            .expect("restore mask");
        render(&mut renderer, &fixture.scene, &target)
            .expect("original descriptor remains resident");
        assert_eq!(
            fixture.counts(),
            [1; 3],
            "rejecting a new definition must preserve old residency"
        );
        assert_eq!(before, pixels(&device, &queue, &target));
    }
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn prepare_failure_preserves_old_residency_and_rolls_back_every_new_resource_kind() {
    let _serial = gpu_test_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("initialized GPU");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = PlainRenderer::new(&device, &queue);
    let target = output(&device);
    let old = Fixture::new([255, 0, 0, 255]);
    render(&mut renderer, &old.scene, &target).expect("old submitted frame");
    let before = pixels(&device, &queue, &target);
    let mut new = Fixture::new([0, 0, 255, 255]);
    let attempts = Arc::new(AtomicUsize::new(0));
    let counts = attempts.clone();
    let failing = new
        .scene
        .resources
        .insert_texture(TextureSource::new(
            TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
            move |mut ctx| {
                if counts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("deliberate failure after phase-zero preparation".into());
                }
                upload_texture(&mut ctx.gpu, &ctx.target, &[0, 255, 0, 255])
            },
        ))
        .expect("new fallible definition");
    new.scene.phases.push(Phase {
        objects: vec![Object::new(
            new.mesh.id(),
            failing,
            new.scene.phases[0].objects[0].transform,
        )],
    });
    let error = render(&mut renderer, &new.scene, &target);
    assert!(
        matches!(error, Err(PlainError::Prepare { .. })),
        "{error:?}"
    );
    assert!(
        renderer.stats().draw_batches > 0,
        "phase-zero commands were recorded before the failure"
    );
    assert!(
        renderer.stats().bind_groups > 0,
        "discarded binding work remains observable"
    );
    assert_eq!(
        new.counts(),
        [1; 3],
        "all three new resource kinds were recorded before failure"
    );
    assert_eq!(
        before,
        pixels(&device, &queue, &target),
        "failed recording never touches destination"
    );
    render(&mut renderer, &old.scene, &target).expect("old residency survives abort");
    assert_eq!(
        old.counts(),
        [1; 3],
        "old resources must not be discarded by abort"
    );
    assert_eq!(before, pixels(&device, &queue, &target));
    render(&mut renderer, &new.scene, &target).expect("same IDs recover on second attempt");
    assert_eq!(
        new.counts(),
        [2; 3],
        "all newly recorded content must be regenerated after abort"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let after = pixels(&device, &queue, &target);
    assert!(after.chunks_exact(4).all(|pixel| pixel == [0, 255, 0, 255]));
    render(&mut renderer, &new.scene, &target).expect("successful retry becomes resident");
    assert_eq!(new.counts(), [2; 3]);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(after, pixels(&device, &queue, &target));
    let error = futures::executor::block_on(validation.pop());
    assert!(
        error.is_none(),
        "CPU callback failure must not create invalid GPU commands: {error:?}"
    );
}

#[test]
fn srgb_reinterpretation_uses_declared_view_format_and_rejects_incompatible_base_format() {
    let _serial = gpu_test_lock();
    let gpu = gpu();
    let (device, queue) = gpu.context().expect("initialized GPU");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = PlainRenderer::new(&device, &queue);
    let make_target = |format, view_formats: &[wgpu::TextureFormat]| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sRGB view proof"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats,
        })
    };
    let reinterpreted = make_target(
        wgpu::TextureFormat::Rgba8Unorm,
        &[wgpu::TextureFormat::Rgba8UnormSrgb],
    );
    let reference = make_target(wgpu::TextureFormat::Rgba8UnormSrgb, &[]);
    let srgb_view = reinterpreted.create_view(&wgpu::TextureViewDescriptor {
        format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
        ..Default::default()
    });
    // A nontrivial linear color distinguishes sRGB encoding from an accidental
    // raw UNORM write; pure primaries would hide the pipeline-format mistake.
    let fixture = Fixture::new([64, 128, 192, 255]);
    render(&mut renderer, &fixture.scene, &reference).expect("native sRGB target");
    renderer
        .render(
            &fixture.scene,
            PlainTarget {
                region: render_interface::TextureRegion::whole(
                    &srgb_view,
                    wgpu::TextureFormat::Rgba8UnormSrgb,
                )
                .expect("whole output region"),
                viewport: [64., 64.],
                clear: wgpu::Color::BLACK,
                initial: None,
            },
        )
        .expect("sRGB reinterpretation of linear-format storage");
    let expected = pixels(&device, &queue, &reference);
    let actual = pixels(&device, &queue, &reinterpreted);
    assert_eq!(actual, expected, "attachment encoding follows view format");
    assert!(
        actual[0] > 120 && actual[1] > 180 && actual[2] > 220,
        "readback contains sRGB-encoded linear color, got {:?}",
        &actual[..4]
    );

    let error = TextureRegion::whole(&srgb_view, wgpu::TextureFormat::Bgra8UnormSrgb);
    assert!(
        error.is_err(),
        "incompatible metadata is rejected before a target can be submitted: {error:?}"
    );
    assert_eq!(
        actual,
        pixels(&device, &queue, &reinterpreted),
        "rejected target is unchanged"
    );

    // A valid region can still have an output format this renderer does not support.
    let coverage_target = make_target(wgpu::TextureFormat::R8Unorm, &[]);
    let error = render(&mut renderer, &fixture.scene, &coverage_target);
    assert!(
        matches!(error, Err(PlainError::Invalid(_))),
        "a valid R8 region is rejected by the renderer's colour-output policy: {error:?}"
    );
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}
