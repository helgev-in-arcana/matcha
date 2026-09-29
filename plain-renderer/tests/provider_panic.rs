//! A caller may catch a provider panic; unsubmitted cache entries must not then
//! masquerade as generated content. Panic payloads and old submitted pixels survive.
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use plain_renderer::{PlacementMode, PlainRenderer, PlainTarget};
use render_interface::*;
use std::{
    panic::{AssertUnwindSafe, catch_unwind, panic_any},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[path = "../examples/support/sources.rs"]
#[allow(dead_code)]
mod sources;

#[derive(Debug)]
struct ProviderPanic(&'static str);

fn target(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("panic recovery destination"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
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
                logical_size: [4., 4.],
                clear: wgpu::Color::BLACK,
                initial: None,
            },
        )
        .expect("recording after a caught provider panic remains usable");
}
fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::Texture) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4 * 256,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(4),
            },
        },
        target.size(),
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
        result.expect("readback mapping")
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("readback completion");
    buffer
        .slice(..)
        .get_mapped_range()
        .chunks_exact(256)
        .flat_map(|row| row[..16].iter().copied())
        .collect()
}
fn quad(mesh: MeshId, texture: TextureId) -> Object {
    Object::new(
        mesh,
        texture,
        Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(4., 4., 1.)),
    )
}

#[test]
fn provider_unwind_rolls_back_all_modes_and_resource_kinds_then_resumes_original_payload() {
    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor {
        backends: match std::env::var("MATCHA_TEST_BACKEND").as_deref() {
            Ok("dx12") => wgpu::Backends::DX12,
            _ => wgpu::Backends::VULKAN,
        },
        ..GpuDescriptor::standard()
    }))
    .expect("real GPU for panic recovery");
    let (device, queue) = gpu.context().expect("device ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    for mode in [PlacementMode::Dedicated, PlacementMode::Atlas] {
        for layout in [
            PrepareOutputLayout::WholeResource,
            PrepareOutputLayout::AnyRegion,
        ] {
            for kind in ["mesh", "texture", "mask"] {
                let mut renderer = PlainRenderer::new(&device, &queue);
                renderer.set_placement_mode(mode);
                let output = target(&device);
                let mesh = sources::unit_quad().with_output_layout(layout);
                let green_calls = Arc::new(AtomicUsize::new(0));
                let count = green_calls.clone();
                let green = TextureSource::new(
                    TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
                    move |mut context| {
                        count.fetch_add(1, Ordering::SeqCst);
                        upload_texture(&mut context.gpu, &context.target, &[0, 255, 0, 255])
                    },
                )
                .with_output_layout(layout);
                let mut old = Scene::default();
                old.resources.share_mesh(&mesh).expect("mesh definition");
                old.resources.share_texture(&green).expect("old definition");
                old.phases.push(Phase {
                    objects: vec![quad(mesh.id(), green.id())],
                });
                render(&mut renderer, &old, &output);
                let before = pixels(&device, &queue, &output);
                assert!(before.chunks_exact(4).all(|p| p == [0, 255, 0, 255]));
                let old_capacity = renderer.stats().placement;

                let red_calls = Arc::new(AtomicUsize::new(0));
                let count = red_calls.clone();
                let red = TextureSource::new(
                    TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm),
                    move |mut context| {
                        count.fetch_add(1, Ordering::SeqCst);
                        upload_texture(&mut context.gpu, &context.target, &[255, 0, 0, 255])
                    },
                )
                .with_output_layout(layout);
                let mut scene = Scene::default();
                scene.resources.share_mesh(&mesh).expect("mesh definition");
                scene.resources.share_texture(&red).expect("new definition");
                scene.phases.push(Phase {
                    objects: vec![quad(mesh.id(), red.id())],
                });
                let fail = Arc::new(AtomicBool::new(true));
                let trigger = fail.clone();
                let mut object = quad(mesh.id(), red.id());
                match kind {
                    "mesh" => {
                        let source = sources::unit_quad();
                        object.mesh = scene
                            .resources
                            .insert_mesh(
                                MeshSource::new(*source.descriptor(), move |context| {
                                    if trigger.swap(false, Ordering::SeqCst) {
                                        panic_any(ProviderPanic(kind));
                                    }
                                    source.prepare(context)
                                })
                                .with_output_layout(layout),
                            )
                            .expect("panicking mesh");
                    }
                    "texture" => {
                        object.texture = scene
                            .resources
                            .insert_texture(
                                TextureSource::new(*red.descriptor(), move |mut context| {
                                    if trigger.swap(false, Ordering::SeqCst) {
                                        panic_any(ProviderPanic(kind));
                                    }
                                    upload_texture(
                                        &mut context.gpu,
                                        &context.target,
                                        &[255, 0, 0, 255],
                                    )
                                })
                                .with_output_layout(layout),
                            )
                            .expect("panicking texture");
                    }
                    _ => {
                        let mask = scene
                            .resources
                            .insert_mask(
                                MaskSource::new(
                                    TextureDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm),
                                    move |mut context| {
                                        if trigger.swap(false, Ordering::SeqCst) {
                                            panic_any(ProviderPanic(kind));
                                        }
                                        upload_texture(&mut context.gpu, &context.target, &[255])
                                    },
                                )
                                .with_output_layout(layout),
                            )
                            .expect("panicking mask");
                        scene.pixel_masks.push(PixelMask {
                            mesh: mesh.id(),
                            texture: mask,
                            transform: object.transform,
                            parent: None,
                        });
                        object.mask = Some(PixelMaskIndex(0));
                    }
                }
                scene.phases.push(Phase {
                    objects: vec![object],
                });
                let panic =
                    catch_unwind(AssertUnwindSafe(|| render(&mut renderer, &scene, &output)))
                        .expect_err("provider panic propagates");
                assert_eq!(
                    panic
                        .downcast_ref::<ProviderPanic>()
                        .expect("original typed payload")
                        .0,
                    kind
                );
                assert_eq!(
                    red_calls.load(Ordering::SeqCst),
                    1,
                    "phase-zero callback ran but was never submitted"
                );
                assert_eq!(
                    renderer.stats().placement,
                    old_capacity,
                    "all provisional placement was released"
                );
                assert_eq!(
                    pixels(&device, &queue, &output),
                    before,
                    "unsubmitted work never changed the destination"
                );
                render(&mut renderer, &old, &output);
                assert_eq!(
                    green_calls.load(Ordering::SeqCst),
                    1,
                    "old valid residency survived"
                );
                render(&mut renderer, &scene, &output);
                assert_eq!(
                    red_calls.load(Ordering::SeqCst),
                    2,
                    "unsubmitted content is regenerated"
                );
                assert!(
                    pixels(&device, &queue, &output)
                        .chunks_exact(4)
                        .all(|p| p == [255, 0, 0, 255])
                );
                render(&mut renderer, &scene, &output);
                assert_eq!(renderer.stats().prepared, 0, "recovered frame becomes warm");
                assert_eq!(
                    renderer.stats().bind_groups,
                    0,
                    "recovered workspace and bindings remain reusable"
                );
            }
        }
    }
    assert!(
        futures::executor::block_on(validation.pop()).is_none(),
        "panic cleanup introduced no GPU validation error"
    );
}
