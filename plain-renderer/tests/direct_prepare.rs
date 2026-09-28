//! Real provider work in shared placements: render attachments, storage meshes,
//! warm reuse and relocation, with exact pixels and preparation-copy accounting.
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use plain_renderer::{AtlasConfig, PlacementMode, PlainRenderer, PlainTarget};
use render_interface::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Observed {
    images: Vec<[u32; 2]>,
    meshes: Vec<[u64; 2]>,
}

fn color(value: wgpu::Color, seen: &Arc<Mutex<Observed>>) -> TextureSource {
    let seen = seen.clone();
    let mut desc = TextureDescriptor::new([4, 4], wgpu::TextureFormat::Rgba8Unorm);
    desc.usages = wgpu::TextureUsages::RENDER_ATTACHMENT;
    TextureSource::new(desc, move |mut c| {
        seen.lock()
            .expect("observation lock")
            .images
            .push(c.target.region.origin());
        let _pass = c.target.region.begin_render_pass(
            &mut c.gpu,
            RegionRenderPassDescriptor {
                load: wgpu::LoadOp::Clear(value),
                ..Default::default()
            },
        )?;
        Ok(())
    })
}

fn mesh(seen: &Arc<Mutex<Observed>>) -> MeshSource {
    let seen = seen.clone();
    let mut desc = MeshDescriptor::triangles(4, 6);
    desc.usages = wgpu::BufferUsages::STORAGE;
    MeshSource::new(desc, move |c| {
        let vertices = c.target.vertices;
        let indices = c.target.indices.expect("indexed quad");
        let alignment = u64::from(c.gpu.device.limits().min_storage_buffer_offset_alignment);
        assert_eq!(vertices.offset() % alignment, 0);
        assert_eq!(indices.offset() % alignment, 0);
        assert_ne!(
            vertices.buffer(),
            indices.buffer(),
            "writable storage bindings must not alias"
        );
        seen.lock()
            .expect("observation lock")
            .meshes
            .push([vertices.offset(), indices.offset()]);
        let shader = c
            .gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("direct indexed mesh"),
                source: wgpu::ShaderSource::Wgsl(
                    r#"
@group(0) @binding(0) var<storage, read_write> vertices: array<f32>;
@group(0) @binding(1) var<storage, read_write> indices: array<u32>;
@compute @workgroup_size(1) fn main() {
    let v = array<f32, 20>(0.,0.,0.,0.,0., 0.,1.,0.,0.,1.,
                          1.,1.,0.,1.,1., 1.,0.,0.,1.,0.);
    let ix = array<u32, 6>(0u,1u,2u,0u,2u,3u);
    for (var i=0u; i<20u; i++) { vertices[i] = v[i]; }
    for (var i=0u; i<6u; i++) { indices[i] = ix[i]; }
}
"#
                    .into(),
                ),
            });
        let pipeline = c
            .gpu
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("direct indexed mesh"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
        fn binding(slice: wgpu::BufferSlice<'_>) -> wgpu::BindingResource<'_> {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: slice.buffer(),
                offset: slice.offset(),
                size: Some(slice.size()),
            })
        }
        let group = c.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: binding(vertices),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: binding(indices),
                },
            ],
        });
        let mut pass = c.gpu.encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        Ok(())
    })
}

fn output(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 16,
            height: 8,
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

fn render(renderer: &mut PlainRenderer, scene: &Scene, texture: &wgpu::Texture) {
    renderer
        .render(
            scene,
            PlainTarget {
                view: &texture.create_view(&Default::default()),
                format: texture.format(),
                viewport: [16., 8.],
                clear: wgpu::Color::BLACK,
                initial: None,
            },
        )
        .expect("direct preparation and composition");
}

fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 8,
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
                rows_per_image: Some(8),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map pixels"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completion");
    buffer
        .slice(..)
        .get_mapped_range()
        .as_chunks::<256>()
        .0
        .iter()
        .flat_map(|row| row[..16 * 4].iter().copied())
        .collect()
}

#[test]
fn region_providers_write_shared_outputs_without_copies_and_survive_relocation() {
    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor {
        backends: match std::env::var("MATCHA_TEST_BACKEND").as_deref() {
            Ok("dx12") => wgpu::Backends::DX12,
            _ => wgpu::Backends::VULKAN,
        },
        ..GpuDescriptor::standard()
    }))
    .expect("real GPU required for direct preparation");
    let (device, queue) = gpu.context().expect("device ready");
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    for mode in [PlacementMode::Atlas, PlacementMode::Dedicated] {
        for layout in [
            PrepareOutputLayout::WholeResource,
            PrepareOutputLayout::AnyRegion,
        ] {
            let seen = Arc::new(Mutex::new(Observed::default()));
            let mut scene = Scene::default();
            scene.phases.push(Phase::default());
            for (index, value) in [wgpu::Color::RED, wgpu::Color::GREEN]
                .into_iter()
                .enumerate()
            {
                let mesh = scene
                    .resources
                    .insert_mesh(mesh(&seen).with_output_layout(layout))
                    .expect("unique mesh definition");
                let image = scene
                    .resources
                    .insert_texture(color(value, &seen).with_output_layout(layout))
                    .expect("unique image definition");
                let transform =
                    Matrix4::new_translation(&nalgebra::Vector3::new(index as f32 * 8., 0., 0.))
                        * Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(8., 8., 1.));
                scene.phases[0]
                    .objects
                    .push(Object::new(mesh, image, transform));
            }
            let mut renderer = PlainRenderer::new(&device, &queue);
            renderer
                .set_atlas_config(AtlasConfig {
                    texture_edge: 16,
                    mesh_page_bytes: 4096,
                })
                .expect("valid page dimensions");
            renderer.set_placement_mode(mode);
            let output = output(&device);
            render(&mut renderer, &scene, &output);
            let copy_fallback =
                mode == PlacementMode::Atlas && layout == PrepareOutputLayout::WholeResource;
            assert_eq!(
                renderer.stats().preparation_texture_copies,
                if copy_fallback { 2 } else { 0 }
            );
            assert_eq!(
                renderer.stats().preparation_buffer_copies,
                if copy_fallback { 4 } else { 0 }
            );
            let before = pixels(&device, &queue, &output);
            for (i, pixel) in before.as_chunks::<4>().0.iter().enumerate() {
                let expected = if i % 16 < 8 {
                    [255, 0, 0, 255]
                } else {
                    [0, 255, 0, 255]
                };
                assert_eq!(
                    *pixel, expected,
                    "neighboring generated output: {mode:?}/{layout:?}/{i}"
                );
            }
            {
                let seen = seen.lock().expect("observation lock");
                assert_eq!(seen.images.len(), 2);
                assert_eq!(seen.meshes.len(), 2);
                let direct_atlas =
                    mode == PlacementMode::Atlas && layout == PrepareOutputLayout::AnyRegion;
                assert_eq!(seen.images.iter().any(|p| *p != [0, 0]), direct_atlas);
                assert_eq!(
                    seen.meshes.iter().any(|p| p[0] > 0 && p[1] > 0),
                    direct_atlas
                );
            }
            render(&mut renderer, &scene, &output);
            assert_eq!(renderer.stats().prepared, 0);
            assert_eq!(renderer.stats().preparation_texture_copies, 0);
            assert_eq!(renderer.stats().preparation_buffer_copies, 0);
            let relocated = renderer
                .compact_resources()
                .expect("relocate direct residents");
            if mode == PlacementMode::Atlas {
                assert!(relocated.copied_bytes > 0);
            }
            render(&mut renderer, &scene, &output);
            assert_eq!(renderer.stats().prepared, 0);
            assert_eq!(pixels(&device, &queue, &output), before);
            let seen = seen.lock().expect("observation lock");
            assert_eq!(
                seen.images.len(),
                2,
                "relocation does not regenerate content"
            );
            assert_eq!(seen.meshes.len(), 2);
        }
    }
    let error = futures::executor::block_on(validation.pop());
    assert!(error.is_none(), "{error:?}");
}
