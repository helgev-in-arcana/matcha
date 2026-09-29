//! UI-independent residency stress: cargo run -p plain-renderer --release
//! --example resource_stress -- resource-stress. Outputs stay under target/.
//! MATCHA_TEST_BACKEND selects vulkan or dx12. No optional device features.
//!
//! render_api_ms is wall time inside render (including CPU/driver/submit work);
//! completion_wait_ms is the following wait, not a GPU timestamp measurement.
//! Allocation traffic covers render on this thread only, excluding Scene assembly,
//! other threads, live memory and driver/VRAM allocations. Numbers are observations,
//! not pass/fail performance thresholds. Pixel/identity/budget invariants are checked.
#[path = "support/allocations.rs"]
mod allocations;
#[path = "support/sources.rs"]
mod sources;
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use plain_renderer::{AtlasConfig, PlacementMode, PlainRenderer, PlainTarget, RenderStats};
use render_interface::*;
use std::{
    fmt::Write,
    path::{Component, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

const SIZE: [u32; 2] = [512, 384];
const SCRATCH_LIMIT: u64 = 16 * 1024;
type Calls = Arc<AtomicUsize>;

fn rect(x: f32, y: f32, width: f32, height: f32) -> Matrix4<f32> {
    Matrix4::new_translation(&nalgebra::Vector3::new(x, y, 0.))
        * Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(width, height, 1.))
}
fn texture(size: [u32; 2], seed: usize, calls: &Calls) -> TextureSource {
    let rgba = [
        (seed * 53 % 240 + 10) as u8,
        (seed * 37 % 240 + 10) as u8,
        180,
        255,
    ];
    let source = sources::rgba(size, rgba.repeat(size[0] as usize * size[1] as usize));
    let calls = calls.clone();
    TextureSource::new(*source.descriptor(), move |context| {
        calls.fetch_add(1, Ordering::Relaxed);
        source.prepare(context)
    })
}
fn mesh(calls: &Calls) -> MeshSource {
    let source = sources::unit_quad();
    let calls = calls.clone();
    MeshSource::new(*source.descriptor(), move |context| {
        calls.fetch_add(1, Ordering::Relaxed);
        source.prepare(context)
    })
}
fn fixed_scene(calls: &Calls) -> Scene {
    let mut scene = Scene::default();
    let quad = scene
        .resources
        .insert_mesh(mesh(calls))
        .expect("fresh mesh");
    let sizes = [1, 2, 4, 8, 16, 32];
    let textures: Vec<_> = (0..24)
        .map(|i| {
            scene
                .resources
                .insert_texture(texture([sizes[i % 6], sizes[(i / 6 + i) % 6]], i, calls))
                .expect("fresh texture")
        })
        .collect();
    for i in 0..2 {
        let source = sources::coverage([8, 8], vec![192 + i * 32; 64]);
        let count = calls.clone();
        let id = scene
            .resources
            .insert_mask(MaskSource::new(*source.descriptor(), move |context| {
                count.fetch_add(1, Ordering::Relaxed);
                source.prepare(context)
            }))
            .expect("fresh coverage");
        scene.pixel_masks.push(PixelMask {
            mesh: quad,
            texture: id,
            transform: rect(0., 0., 512., 384.),
            parent: (i == 1).then_some(PixelMaskIndex(0)),
        });
    }
    scene.phases.push(Phase {
        objects: (0..3072)
            .map(|i| Object {
                mask: (i % 3 == 0).then_some(PixelMaskIndex((i % 2) as u32)),
                ..Object::new(
                    quad,
                    textures[i % textures.len()],
                    rect((i % 64) as f32 * 8., (i / 64) as f32 * 8., 8., 8.),
                )
            })
            .collect(),
    });
    scene
}
fn render(
    renderer: &mut PlainRenderer,
    device: &wgpu::Device,
    scene: &Scene,
    view: &wgpu::TextureView,
    label: &str,
    report: &mut String,
) -> RenderStats {
    let start = Instant::now();
    let (result, allocations) = allocations::measure(|| {
        renderer.render(
            scene,
            PlainTarget {
                region: render_interface::TextureRegion::whole(
                    view,
                    wgpu::TextureFormat::Rgba8UnormSrgb,
                )
                .expect("whole output region"),
                viewport: [SIZE[0] as f32, SIZE[1] as f32],
                clear: wgpu::Color::BLACK,
                initial: None,
            },
        )
    });
    let api_ms = start.elapsed().as_secs_f64() * 1000.;
    result.expect("valid stress Scene");
    let wait = Instant::now();
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completion");
    let wait_ms = wait.elapsed().as_secs_f64() * 1000.;
    let s = renderer.stats();
    assert!(
        s.scratch_bytes <= SCRATCH_LIMIT,
        "retained scratch exceeds its budget"
    );
    writeln!(report,
        "{label}\t{api_ms:.4}\t{wait_ms:.4}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        allocations.allocations, allocations.reallocations, allocations.requested_bytes,
        s.prepared, s.cache_hits, s.cache_bytes, s.placement.texture_pages, s.placement.mesh_pages,
        s.placement.reserved_texture_bytes + s.placement.reserved_mesh_bytes,
        s.scratch_bytes, s.scratch_peak_bytes, s.parameter_bytes, s.working_bytes, s.evicted,
        s.over_budget_bytes, s.draw_calls, s.draw_batches, s.mask_passes, s.bind_groups,
    ).expect("format metrics");
    s
}
fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let bytes = u64::from(SIZE[0]) * u64::from(SIZE[1]) * 4;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("stress readback"),
        size: bytes,
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
                bytes_per_row: Some(SIZE[0] * 4),
                rows_per_image: Some(SIZE[1]),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.expect("map readback"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("readback completion");
    buffer.slice(..).get_mapped_range().to_vec()
}
fn main() {
    let folder = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "resource-stress".into()),
    );
    assert!(
        folder
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
        "output must be a relative folder under target/"
    );
    let directory = PathBuf::from("target").join(folder);
    std::fs::create_dir_all(&directory).expect("output directory");
    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor {
        backends: match std::env::var("MATCHA_TEST_BACKEND").as_deref() {
            Ok("vulkan") => wgpu::Backends::VULKAN,
            Ok("dx12") => wgpu::Backends::DX12,
            _ => wgpu::Backends::PRIMARY,
        },
        ..GpuDescriptor::standard()
    }))
    .expect("real GPU adapter with standard features");
    let (device, queue) = gpu.context().expect("GPU ready");
    assert!(device.features().is_empty());
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("resource stress"),
        size: wgpu::Extent3d {
            width: SIZE[0],
            height: SIZE[1],
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
    let calls = Arc::new(AtomicUsize::new(0));
    let fixed = fixed_scene(&calls);
    let mut report = format!(
        "# Adapter: {:?}\n# 3072 fixed objects; 24 image sizes/colours; 2 masks; 64 update frames per mode\n# render_api_ms includes driver/submit, completion_wait_ms is residual wait (not GPU execution time)\n# allocations are calling-thread traffic inside render, not live bytes/VRAM\n",
        gpu.adapter().get_info()
    );
    report.push_str("label\trender_api_ms\tcompletion_wait_ms\tallocations\treallocations\trequested_bytes\tprepared\thits\tlogical_bytes\ttexture_pages\tmesh_pages\tpage_capacity_bytes\tscratch_retained_bytes\tscratch_peak_bytes\tparameter_bytes\tworking_bytes\tevicted\tover_budget_bytes\tdraws\tdraw_batches\tmask_passes\tbind_groups\n");
    let mut baseline = None;
    for mode in [PlacementMode::Dedicated, PlacementMode::Atlas] {
        let mut renderer = PlainRenderer::new(&device, &queue);
        renderer.set_placement_mode(mode);
        renderer
            .set_atlas_config(AtlasConfig {
                texture_edge: 128,
                mesh_page_bytes: 4096,
            })
            .expect("small pages");
        renderer.set_scratch_budget(SCRATCH_LIMIT);
        let cold = render(
            &mut renderer,
            &device,
            &fixed,
            &view,
            &format!("{mode:?}-cold"),
            &mut report,
        );
        assert_eq!(cold.prepared, fixed.resources.len());
        let generated = calls.load(Ordering::Relaxed);
        for i in 0..3 {
            let warm = render(
                &mut renderer,
                &device,
                &fixed,
                &view,
                &format!("{mode:?}-warm-{i}"),
                &mut report,
            );
            assert_eq!(warm.prepared, 0);
            assert_eq!(
                warm.mask_passes, 2,
                "each shared mask is rasterized once per frame"
            );
            assert_eq!(
                warm.bind_groups, 0,
                "warm bindings are reused across frames"
            );
            assert_eq!(calls.load(Ordering::Relaxed), generated);
        }
        let before = pixels(&device, &queue, &target);
        if let Some(reference) = &baseline {
            assert_eq!(&before, reference, "placement modes preserve pixels");
        } else {
            baseline = Some(before.clone());
        }
        image::save_buffer(
            directory.join(format!("{mode:?}.png")),
            &before,
            SIZE[0],
            SIZE[1],
            image::ColorType::Rgba8,
        )
        .expect("save proof image");
        match mode {
            PlacementMode::Atlas => assert!(renderer.compact_resources_with_budget(0).is_err()),
            PlacementMode::Dedicated => {
                let unchanged = renderer
                    .compact_resources_with_budget(0)
                    .expect("dedicated placement needs no relocation");
                assert_eq!(unchanged.copied_bytes, 0);
                assert_eq!(unchanged.meshes, 0);
                assert_eq!(unchanged.textures, 0);
            }
        }
        let moved = renderer
            .compact_resources_with_budget(cold.cache_bytes)
            .expect("bounded relocation");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            generated,
            "relocation must not prepare sources"
        );
        assert_eq!(
            moved.copied_bytes,
            match mode {
                PlacementMode::Atlas => cold.cache_bytes,
                PlacementMode::Dedicated => 0,
            }
        );
        writeln!(report, "# {mode:?} relocation: {moved:?}").expect("relocation metrics");
        assert_eq!(
            render(
                &mut renderer,
                &device,
                &fixed,
                &view,
                &format!("{mode:?}-relocated"),
                &mut report
            )
            .prepared,
            0
        );
        assert_eq!(
            before,
            pixels(&device, &queue, &target),
            "relocation preserves pixels"
        );

        renderer.set_cache_budget(4096);
        let mut churn = Scene::default();
        let quad = churn
            .resources
            .insert_mesh(mesh(&calls))
            .expect("churn mesh");
        for frame in 0..64 {
            churn.resources.retain_textures(|_| false);
            let mut images = Vec::new();
            let mut current_bytes = (6 * std::mem::size_of::<Vertex>()) as u64;
            for i in 0..4 {
                let side = [8, 16, 24, 32, 48, 64][(frame + i) % 6];
                current_bytes += u64::from(side) * u64::from(side) * 4;
                images.push(
                    churn
                        .resources
                        .insert_texture(texture([side, side], frame * 4 + i, &calls))
                        .expect("new content ID"),
                );
            }
            churn.phases = vec![Phase {
                objects: (0..64)
                    .map(|i| {
                        Object::new(
                            quad,
                            images[i % 4],
                            rect((i % 8) as f32 * 64., (i / 8) as f32 * 48., 64., 48.),
                        )
                    })
                    .collect(),
            }];
            let stats = render(
                &mut renderer,
                &device,
                &churn,
                &view,
                &format!("{mode:?}-update-{frame}"),
                &mut report,
            );
            assert_eq!(stats.prepared, if frame == 0 { 5 } else { 4 });
            assert_eq!(
                stats.cache_bytes, current_bytes,
                "obsolete content must be evicted under pressure"
            );
            assert_eq!(
                stats.over_budget_bytes,
                current_bytes - 4096,
                "only the pinned working set may exceed the budget"
            );
            if frame % 16 == 0 {
                assert_eq!(
                    render(
                        &mut renderer,
                        &device,
                        &churn,
                        &view,
                        &format!("{mode:?}-update-{frame}-warm"),
                        &mut report
                    )
                    .prepared,
                    0
                );
            }
        }
        renderer.set_cache_budget(0);
        renderer.set_scratch_budget(0);
        let drained = render(
            &mut renderer,
            &device,
            &Scene::default(),
            &view,
            &format!("{mode:?}-drained"),
            &mut report,
        );
        assert_eq!(drained.cache_bytes, 0);
        assert_eq!(drained.placement, plain_renderer::PlacementStats::default());
        assert_eq!(drained.scratch_bytes, 0);
    }
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "WebGPU validation error: {error:?}");
    std::fs::write(directory.join("metrics.tsv"), &report).expect("write metrics");
    println!(
        "{report}All residency stress checks passed; outputs: {}",
        directory.display()
    );
}
