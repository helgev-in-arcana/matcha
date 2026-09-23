//! Native widget/renderer-interface stress proof. No matcha-paint adapter.
//! cargo run -p matcha-ecs --example interface_stress -- target
//! Generates and verifies GPU box masks against the CPU oracle, then exercises
//! retained/shared sources and per-redraw background-dependent widget Scenes.
use bevy_ecs::{bundle::Bundle, world::World};
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use matcha_ecs::{
    components::{
        render::{RenderCtx, RenderItem},
        view::ViewChildren,
    },
    layout::{Constraints, LayoutDispatch, layout_root},
    render::{GuiRenderer, extract_items},
    scene::{push_quad, unit_quad},
    view::{Widget, run_view},
};
use matcha_ecs_widgets::{
    Column, RectGeometry, Row, Text,
    shape::{CoverageKey, ShapeCtx, rasterize_box},
};
use parking_lot::Mutex;
use render_interface::*;
use renderer::{SceneRenderer, SceneTarget};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wgpu::util::DeviceExt;

fn target(device: &wgpu::Device, size: [u32; 2], srgb: bool) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("native interface proof"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        },
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
fn read(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let [w, h] = [texture.width(), texture.height()];
    let row = (w * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row) * u64::from(h),
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
                rows_per_image: Some(h),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("readback map"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completion");
    let bytes = buffer.slice(..).get_mapped_range();
    let mut out = Vec::new();
    for line in bytes.chunks_exact(row as usize) {
        out.extend_from_slice(&line[..w as usize * 4]);
    }
    out
}
fn rect(w: f32, h: f32) -> Matrix4<f32> {
    Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(w, h, 1.))
}
fn white() -> TextureSource {
    let mut desc = TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8Unorm);
    desc.usages = wgpu::TextureUsages::RENDER_ATTACHMENT;
    TextureSource::new(desc, |c| {
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: c.target.view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
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
    })
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EffectParams {
    transform: Matrix4<f32>,
    viewport_size: [f32; 4],
    mode: [u32; 4],
}
struct Program {
    device: wgpu::Device,
    pipeline: wgpu::ComputePipeline,
}
type Programs = Arc<Mutex<Option<Program>>>;
fn effect(
    programs: &Programs,
    params: EffectParams,
    gpu: GpuPrepareContext<'_>,
    target: TextureTarget<'_>,
) -> PrepareResult {
    let mut cache = programs.lock();
    if cache.as_ref().is_none_or(|p| p.device != *gpu.device) {
        let module = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native widget effect"),
                source: wgpu::ShaderSource::Wgsl(include_str!("support/native_effect.wgsl").into()),
            });
        *cache = Some(Program {
            device: gpu.device.clone(),
            pipeline: gpu
                .device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: None,
                    layout: None,
                    module: &module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                }),
        });
    }
    let program = cache.as_ref().expect("device program initialized");
    let uniform = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("widget placement"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &program.pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(gpu.snapshot.color_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(target.view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform.as_entire_binding(),
            },
        ],
    });
    let mut pass = gpu.encoder.begin_compute_pass(&Default::default());
    pass.set_pipeline(&program.pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.dispatch_workgroups(
        target.desc.size[0].div_ceil(8),
        target.desc.size[1].div_ceil(8),
        1,
    );
    Ok(())
}
fn descriptor(size: [u32; 2]) -> TextureDescriptor {
    let mut desc = TextureDescriptor::new(size, wgpu::TextureFormat::Rgba8Unorm);
    desc.usages = wgpu::TextureUsages::STORAGE_BINDING;
    desc
}

struct ShaderWidget {
    background: TextureSource,
    programs: Programs,
    mode: u32,
    background_alpha: f32,
    generated: Arc<AtomicUsize>,
}
impl Widget for ShaderWidget {
    fn bundle(&self) -> impl Bundle {
        let background = self.background.clone();
        let programs = self.programs.clone();
        let mode = self.mode;
        let alpha = self.background_alpha;
        let generated = self.generated.clone();
        (
            RectGeometry { w: 160., h: 120. },
            LayoutDispatch::of::<RectGeometry>(),
            RenderItem::dynamic(move |ctx, scene| {
                scene.resources.retain_textures(|id| id == background.id());
                for p in &mut scene.phases {
                    p.objects.clear();
                }
                scene.pixel_masks.clear();
                let mesh = scene
                    .resources
                    .share_mesh(&unit_quad())
                    .expect("same shared mesh");
                let texture = scene
                    .resources
                    .share_texture(&background)
                    .expect("same shared background");
                scene.phases.resize_with(2, Phase::default);
                scene.phases[0].objects.push(Object {
                    opacity: alpha,
                    ..Object::new(mesh, texture, rect(ctx.size[0], ctx.size[1]))
                });
                let params = EffectParams {
                    transform: ctx.transform,
                    viewport_size: [
                        ctx.viewport_size[0],
                        ctx.viewport_size[1],
                        ctx.size[0],
                        ctx.size[1],
                    ],
                    mode: [mode, 0, 0, 0],
                };
                let programs = programs.clone();
                let generated = generated.clone();
                let texture = scene
                    .resources
                    .insert_texture(TextureSource::new(
                        descriptor(ctx.size.map(|v| v as u32)),
                        move |c| {
                            generated.fetch_add(1, Ordering::SeqCst);
                            effect(&programs, params, c.gpu, c.target)
                        },
                    ))
                    .expect("fresh background-dependent ID");
                scene.phases[1].objects.push(Object::new(
                    mesh,
                    texture,
                    rect(ctx.size[0], ctx.size[1]),
                ));
            }),
        )
    }
    fn patch(&self, _entity: &mut bevy_ecs::world::EntityWorldMut) {}
}

fn main() {
    let directory = std::env::args().nth(1).unwrap_or_else(|| "target".into());
    std::fs::create_dir_all(&directory).expect("output directory");
    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor {
        required_features: wgpu::Features::empty(),
        backends: if std::env::var("MATCHA_TEST_BACKEND").as_deref() == Ok("dx12") {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::VULKAN
        },
        ..Default::default()
    }))
    .expect("real GPU");
    let (device, queue) = gpu.context().expect("GPU ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut backend = SceneRenderer::new(&device, &queue);
    let mut report = format!("Adapter: {:?}\n", gpu.adapter().get_info());
    let mut world = World::new();
    let entity = world.spawn_empty().id();
    let shape = ShapeCtx::get(&mut world.entity_mut(entity));
    let cases = [
        CoverageKey::filled(1, 1, [0.; 4]),
        CoverageKey::filled(37, 23, [8., 2., 12., 0.]),
        CoverageKey::ring(37, 23, [8.; 4], [1., 4., 7., 2.]),
        CoverageKey::filled(64, 48, [9.; 4]).inset(10.).blurred(3.),
        CoverageKey::ring(64, 48, [9.; 4], [2., 3., 2., 3.])
            .inset(8.)
            .blurred(2.),
    ];
    for (i, key) in cases.into_iter().enumerate() {
        let ctx = RenderCtx {
            transform: Matrix4::identity(),
            viewport_size: [key.w as f32, key.h as f32],
            size: [key.w as f32, key.h as f32],
            focused: false,
            focus_within: false,
            hovered: false,
            active: false,
        };
        let mask = shape.coverage_source(key, &ctx).expect("GPU shape source");
        let mut scene = Scene::default();
        push_quad(
            &mut scene,
            &white(),
            ctx.size,
            Matrix4::identity(),
            Some(&mask),
        );
        let output = target(&device, [key.w, key.h], false);
        backend
            .render(
                &scene,
                SceneTarget {
                    view: &output.create_view(&Default::default()),
                    viewport: ctx.size,
                    clear: wgpu::Color::BLACK,
                    initial: None,
                },
            )
            .expect("GPU shape");
        let bytes = read(&device, &queue, &output);
        let oracle = rasterize_box(key);
        let delta = bytes
            .chunks_exact(4)
            .zip(&oracle)
            .map(|(a, b)| a[0].abs_diff(*b))
            .max()
            .expect("pixels");
        assert!(
            delta <= 1,
            "GPU box/shadow vs CPU oracle, case {i}, delta {delta}"
        );
        report.push_str(&format!("GPU shape {i}: max byte delta {delta}\n"));
        image::save_buffer(
            std::path::Path::new(&directory).join(format!("native-shape-{i}.png")),
            &bytes,
            key.w,
            key.h,
            image::ColorType::Rgba8,
        )
        .expect("shape artifact");
    }
    let programs: Programs = Arc::new(Mutex::new(None));
    let bg_calls = Arc::new(AtomicUsize::new(0));
    let calls = bg_calls.clone();
    let background_programs = programs.clone();
    let background = TextureSource::new(descriptor([160, 120]), move |c| {
        calls.fetch_add(1, Ordering::SeqCst);
        effect(
            &background_programs,
            EffectParams {
                transform: Matrix4::identity(),
                viewport_size: [160., 120., 160., 120.],
                mode: [0; 4],
            },
            c.gpu,
            c.target,
        )
    });
    let effect_calls = Arc::new(AtomicUsize::new(0));
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.node(Column::new().gap(11.), |s| {
            s.leaf(
                Text::new("Native Scene generators: blur / refraction / invert")
                    .font_size(16.)
                    .color([0.9, 0.9, 0.9, 1.]),
            );
            s.node(Row::new().gap(13.), |s| {
                for (mode, alpha) in [(1, 1.), (2, 0.7), (3, 0.4)] {
                    s.leaf(ShaderWidget {
                        background: background.clone(),
                        programs: programs.clone(),
                        mode,
                        background_alpha: alpha,
                        generated: effect_calls.clone(),
                    });
                }
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([512., 176.]));
    let mut frame = extract_items(&world, root);
    let output = target(&device, [512, 176], true);
    let output_view = output.create_view(&Default::default());
    let mut renderer = GuiRenderer::new(&device, &queue);
    let clear = wgpu::Color {
        r: 0.02,
        g: 0.02,
        b: 0.02,
        a: 1.,
    };
    for i in 0..3 {
        let start = std::time::Instant::now();
        renderer
            .render_extracted(
                &frame.items,
                &frame.clips,
                SceneTarget {
                    view: &output_view,
                    viewport: [512., 176.],
                    clear,
                    initial: None,
                },
            )
            .expect("native widget scene");
        let bytes = read(&device, &queue, &output);
        report.push_str(&format!(
            "Widget frame {i}: {:?}, {:?}\n",
            start.elapsed(),
            renderer.backend.stats()
        ));
        assert_eq!(
            bg_calls.load(Ordering::SeqCst),
            1,
            "one GPU generation shared by three independent widget Scenes"
        );
        assert_eq!(
            effect_calls.load(Ordering::SeqCst),
            (i + 1) * 3,
            "fresh effect IDs per redraw"
        );
        if i > 0 {
            assert_eq!(
                renderer.backend.stats().prepared,
                3,
                "static resources retained while effects change"
            );
        }
        image::save_buffer(
            std::path::Path::new(&directory).join("native-widgets.png"),
            &bytes,
            512,
            176,
            image::ColorType::Rgba8,
        )
        .expect("GUI artifact");
    }
    // Render phase zero as a reference, then compare the translated third
    // widget's invert output to its own backdrop in linear colour space.
    let full = read(&device, &queue, &output);
    let effects = renderer.scene.phases.pop().expect("effect phase");
    renderer
        .backend
        .render(
            &renderer.scene,
            SceneTarget {
                view: &output_view,
                viewport: [512., 176.],
                clear,
                initial: None,
            },
        )
        .expect("phase-zero reference");
    let base = read(&device, &queue, &output);
    renderer.scene.phases.push(effects);
    let item = frame
        .items
        .iter()
        .filter(|i| i.rebuild_each_frame)
        .nth(2)
        .expect("third effect widget");
    let x = item.transform[(0, 3)] as usize + 67;
    let y = item.transform[(1, 3)] as usize + 43;
    let at = (y * 512 + x) * 4;
    for channel in 0..3 {
        let encoded = base[at + channel] as f32 / 255.;
        let linear = if encoded <= 0.04045 {
            encoded / 12.92
        } else {
            ((encoded + 0.055) / 1.055).powf(2.4)
        };
        let v = 1. - linear;
        let expected = if v <= 0.0031308 {
            v * 12.92
        } else {
            1.055 * v.powf(1. / 2.4) - 0.055
        };
        assert!(
            full[at + channel].abs_diff((expected * 255.).round() as u8) <= 3,
            "effect samples its translated backdrop, channel {channel}"
        );
    }
    // Already prepared background-dependent output survives pure GPU relocation.
    let moved = renderer
        .backend
        .compact_resources(renderer::scene_renderer::AtlasConfig {
            texture_edge: 256,
            mesh_page_bytes: 4096,
        })
        .expect("repack");
    renderer
        .backend
        .render(
            &renderer.scene,
            SceneTarget {
                view: &output_view,
                viewport: [512., 176.],
                clear,
                initial: None,
            },
        )
        .expect("repacked scene");
    assert_eq!(full, read(&device, &queue, &output));
    assert_eq!(effect_calls.load(Ordering::SeqCst), 9);
    report.push_str(&format!(
        "Translated backdrop oracle passed; relocation without generator calls: {moved:?}\n"
    ));
    let limit = renderer.scene.resources.len();
    let bad = RenderItem::new(|_| {
        let mut scene = Scene::default();
        scene.phases.push(Phase {
            objects: vec![Object {
                mask: Some(PixelMaskIndex(u32::MAX)),
                ..Object::new(MeshId::new(), TextureId::new(), Matrix4::identity())
            }],
        });
        scene
    });
    frame.items.push(matcha_ecs::render::RenderItemSnapshot {
        entity: root,
        rebuild_each_frame: false,
        cache: bad.cache,
        builder: bad.builder,
        transform: Matrix4::identity(),
        size: [1., 1.],
        opacity: 1.,
        focused: false,
        focus_within: false,
        hovered: false,
        active: false,
        clip: None,
    });
    for _ in 0..20 {
        assert!(
            renderer
                .assemble(&frame.items, &frame.clips, [512., 176.])
                .is_err()
        );
        assert!(
            renderer.scene.resources.len() <= limit,
            "failed assemblies must not retain past dynamic definitions"
        );
    }
    assert_eq!(
        effect_calls.load(Ordering::SeqCst),
        9,
        "failed CPU composition never invokes GPU generators"
    );
    frame.items.pop();
    renderer
        .render_extracted(
            &frame.items,
            &frame.clips,
            SceneTarget {
                view: &output_view,
                viewport: [512., 176.],
                clear,
                initial: None,
            },
        )
        .expect("retry after malformed fragment");
    assert_eq!(full, read(&device, &queue, &output));
    report.push_str("20 failed CPU compositions stayed bounded; repaired frame reproduced the original pixels.\n");
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "GPU validation: {error:?}");
    std::fs::write(
        std::path::Path::new(&directory).join("native-interface-stress.txt"),
        &report,
    )
    .expect("report");
    println!("{report}");
}
