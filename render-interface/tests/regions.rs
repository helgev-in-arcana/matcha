//! Public preparation helpers exercised independently of a renderer or UI.
use render_interface::*;

fn device() -> (wgpu::Device, wgpu::Queue) {
    futures::executor::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: match std::env::var("MATCHA_TEST_BACKEND").as_deref() {
                Ok("vulkan") => wgpu::Backends::VULKAN,
                Ok("dx12") => wgpu::Backends::DX12,
                _ => wgpu::Backends::PRIMARY,
            },
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&Default::default())
            .await
            .expect("real GPU adapter");
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
                ..Default::default()
            })
            .await
            .expect("standard GPU device")
    })
}

fn texture(device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("region helper proof"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn read_buffer(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Vec<u8> {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("region readback"),
        size: buffer.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &readback, 0, buffer.size());
    queue.submit([encoder.finish()]);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.expect("map buffer"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completes");
    readback.slice(..).get_mapped_range().to_vec()
}

fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let bpp = texture
        .format()
        .block_copy_size(None)
        .expect("colour bytes per texel");
    let row = texture.width() * bpp;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("region texture readback"),
        size: u64::from(padded) * u64::from(texture.height()),
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
                bytes_per_row: Some(padded),
                rows_per_image: Some(texture.height()),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.expect("map texture"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completes");
    buffer
        .slice(..)
        .get_mapped_range()
        .chunks_exact(padded as usize)
        .flat_map(|line| line[..row as usize].iter().copied())
        .collect()
}

fn half_quad_pipeline(device: &wgpu::Device) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("local left half proof"),
        source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(r#"
@vertex fn vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 6>(vec2(-1.,-1.),vec2(0.,-1.),vec2(-1.,1.),vec2(-1.,1.),vec2(0.,-1.),vec2(0.,1.));
    return vec4(p[i], 0., 1.);
}
@fragment fn fragment() -> @location(0) vec4<f32> { return vec4(0.,1.,0.,1.); }
"#)),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("local left half"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[test]
fn partial_clear_and_subsequent_draw_preserve_guard_texels_and_local_viewport() {
    let (device, queue) = device();
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let output = texture(&device, [12, 10], wgpu::TextureFormat::Rgba8Unorm);
    let view = output.create_view(&Default::default());
    let snapshot = texture(&device, [1, 1], wgpu::TextureFormat::Rgba8Unorm);
    let snapshot_view = snapshot.create_view(&Default::default());
    let mut cache = RegionRenderPassCache::new(&device);
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut gpu = GpuPrepareContext {
        device: &device,
        encoder: &mut encoder,
        snapshot: RenderSnapshot {
            color: TextureRegion::whole(&snapshot_view, snapshot.format()).expect("snapshot"),
        },
        render_pass_cache: &mut cache,
    };
    let whole = TextureRegion::whole(&view, output.format()).expect("whole");
    drop(
        whole
            .begin_render_pass(
                &mut gpu,
                RegionRenderPassDescriptor {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLUE),
                    ..Default::default()
                },
            )
            .expect("whole clear"),
    );
    let region = TextureRegion::new(&view, output.format(), [3, 2], [4, 4]).expect("subregion");
    assert!(
        region
            .begin_render_pass(
                &mut gpu,
                RegionRenderPassDescriptor {
                    store: wgpu::StoreOp::Discard,
                    ..Default::default()
                }
            )
            .is_err()
    );
    let pipeline = half_quad_pipeline(&device);
    {
        let mut pass = region
            .begin_render_pass(
                &mut gpu,
                RegionRenderPassDescriptor {
                    load: wgpu::LoadOp::Clear(wgpu::Color::RED),
                    ..Default::default()
                },
            )
            .expect("partial clear");
        pass.set_pipeline(&pipeline);
        pass.draw(0..6, 0..1);
    }
    queue.submit([encoder.finish()]);
    let pixels = pixels(&device, &queue, &output);
    for y in 0..10 {
        for x in 0..12 {
            let expected = if (2..6).contains(&y) && (3..7).contains(&x) {
                if x < 5 {
                    [0, 255, 0, 255]
                } else {
                    [255, 0, 0, 255]
                }
            } else {
                [0, 0, 255, 255]
            };
            assert_eq!(&pixels[(y * 12 + x) * 4..][..4], expected, "pixel {x},{y}");
        }
    }
    assert!(futures::executor::block_on(validation.pop()).is_none());
}

#[test]
fn partial_clear_matches_native_clear_for_linear_srgb_mask_and_float_formats() {
    let (device, queue) = device();
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut cache = RegionRenderPassCache::new(&device);
    let snapshot = texture(&device, [1, 1], wgpu::TextureFormat::Rgba8Unorm);
    let snapshot_view = snapshot.create_view(&Default::default());
    for format in [
        wgpu::TextureFormat::R8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Rgba16Float,
    ] {
        let output = texture(&device, [4, 3], format);
        let view = output.create_view(&Default::default());
        let reference = texture(&device, [1, 1], format);
        let reference_view = reference.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut gpu = GpuPrepareContext {
            device: &device,
            encoder: &mut encoder,
            snapshot: RenderSnapshot {
                color: TextureRegion::whole(&snapshot_view, snapshot.format()).expect("snapshot"),
            },
            render_pass_cache: &mut cache,
        };
        let color = wgpu::Color {
            r: 0.5,
            g: 0.25,
            b: 0.125,
            a: 0.5,
        };
        let whole = TextureRegion::whole(&view, format).expect("whole");
        drop(
            whole
                .begin_render_pass(
                    &mut gpu,
                    RegionRenderPassDescriptor {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        ..Default::default()
                    },
                )
                .expect("guard clear"),
        );
        let region = TextureRegion::new(&view, format, [1, 1], [2, 1]).expect("region");
        drop(
            region
                .begin_render_pass(
                    &mut gpu,
                    RegionRenderPassDescriptor {
                        load: wgpu::LoadOp::Clear(color),
                        ..Default::default()
                    },
                )
                .expect("partial clear"),
        );
        drop(
            TextureRegion::whole(&reference_view, format)
                .expect("reference")
                .begin_render_pass(
                    &mut gpu,
                    RegionRenderPassDescriptor {
                        load: wgpu::LoadOp::Clear(color),
                        ..Default::default()
                    },
                )
                .expect("reference clear"),
        );
        queue.submit([encoder.finish()]);
        let actual = pixels(&device, &queue, &output);
        let expected = pixels(&device, &queue, &reference);
        for (index, pixel) in actual.chunks_exact(expected.len()).enumerate() {
            if index == 5 || index == 6 {
                assert_eq!(pixel, expected, "{format:?}");
            } else {
                assert!(
                    pixel.iter().all(|v| *v == 0),
                    "guard {index} for {format:?}"
                );
            }
        }
    }
    assert!(futures::executor::block_on(validation.pop()).is_none());
}

#[test]
fn texture_and_buffer_uploads_and_cropped_copy_respect_physical_offsets() {
    let (device, queue) = device();
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let source = texture(&device, [10, 8], wgpu::TextureFormat::Rgba8Unorm);
    let source_view = source.create_view(&Default::default());
    let destination = texture(&device, [12, 9], source.format());
    let destination_view = destination.create_view(&Default::default());
    let snapshot = texture(&device, [1, 1], source.format());
    let snapshot_view = snapshot.create_view(&Default::default());
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 32,
        usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut cache = RegionRenderPassCache::new(&device);
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut gpu = GpuPrepareContext {
        device: &device,
        encoder: &mut encoder,
        snapshot: RenderSnapshot {
            color: TextureRegion::whole(&snapshot_view, snapshot.format()).expect("snapshot"),
        },
        render_pass_cache: &mut cache,
    };
    let source_region =
        TextureRegion::new(&source_view, source.format(), [2, 1], [5, 4]).expect("source region");
    let destination_region =
        TextureRegion::new(&destination_view, destination.format(), [3, 2], [6, 5])
            .expect("destination region");
    assert_eq!(source_region.uv_scale_bias(), [0.5, 0.5, 0.2, 0.125]);
    assert_eq!(source_region.uv_clamp(), [0.25, 0.1875, 0.65, 0.5625]);
    let mut expected_source = [101, 37, 59, 255].repeat(10 * 8);
    upload_texture(
        &mut gpu,
        &TextureTarget {
            desc: &TextureDescriptor::new([10, 8], source.format()),
            region: TextureRegion::whole(&source_view, source.format()).expect("whole source"),
        },
        &expected_source,
    )
    .expect("source guard upload");
    let mut expected = [17, 31, 43, 255].repeat(12 * 9);
    upload_texture(
        &mut gpu,
        &TextureTarget {
            desc: &TextureDescriptor::new([12, 9], destination.format()),
            region: TextureRegion::whole(&destination_view, destination.format())
                .expect("whole destination"),
        },
        &expected,
    )
    .expect("guard upload");
    let bytes: Vec<u8> = (0..4)
        .flat_map(|y| (0..5).flat_map(move |x| [x * 31, y * 53, 91, 255]))
        .collect();
    let desc = TextureDescriptor::new([5, 4], source.format());
    upload_texture(
        &mut gpu,
        &TextureTarget {
            desc: &desc,
            region: source_region,
        },
        &bytes,
    )
    .expect("offset upload");
    let wrong_size = TextureDescriptor::new([4, 5], source.format());
    assert!(
        upload_texture(
            &mut gpu,
            &TextureTarget {
                desc: &wrong_size,
                region: source_region
            },
            &bytes
        )
        .is_err()
    );
    let wrong_format = TextureDescriptor::new([5, 4], wgpu::TextureFormat::Rgba8UnormSrgb);
    assert!(
        upload_texture(
            &mut gpu,
            &TextureTarget {
                desc: &wrong_format,
                region: source_region
            },
            &bytes
        )
        .is_err()
    );
    assert_eq!(
        source_region
            .copy_to(gpu.encoder, &destination_region, [-1, 1], [1, -1], [9, 5])
            .expect("cropped copy"),
        [4, 2]
    );
    assert!(
        source_region
            .copy_to(gpu.encoder, &source_region, [0, 0], [0, 0], [1, 1])
            .is_err()
    );
    assert_eq!(
        source_region
            .copy_to(gpu.encoder, &destination_region, [99, 0], [0, 0], [1, 1])
            .expect("empty copy"),
        [0, 0]
    );
    upload_buffer(&mut gpu, buffer.slice(..), &[9; 32]).expect("buffer guard");
    upload_buffer(&mut gpu, buffer.slice(8..16), &[1, 2, 3, 4, 5, 6, 7, 8])
        .expect("offset buffer upload");
    assert!(upload_buffer(&mut gpu, buffer.slice(1..9), &[0; 4]).is_err());
    assert!(upload_buffer(&mut gpu, buffer.slice(8..16), &[0; 12]).is_err());
    queue.submit([encoder.finish()]);
    for y in 0..2 {
        for x in 0..4 {
            expected[((y + 2) * 12 + x + 5) * 4..][..4]
                .copy_from_slice(&bytes[((y + 2) * 5 + x) * 4..][..4]);
        }
    }
    assert_eq!(pixels(&device, &queue, &destination), expected);
    for y in 0..4 {
        expected_source[((y + 1) * 10 + 2) * 4..][..20].copy_from_slice(&bytes[y * 20..][..20]);
    }
    assert_eq!(pixels(&device, &queue, &source), expected_source);
    let mut expected_buffer = [9; 32];
    expected_buffer[8..16].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(read_buffer(&device, &queue, &buffer), expected_buffer);
    assert!(TextureRegion::new(&source_view, source.format(), [u32::MAX, 0], [1, 1]).is_err());
    assert!(
        TextureRegion::new(
            &source_view,
            wgpu::TextureFormat::Bgra8Unorm,
            [0, 0],
            [1, 1]
        )
        .is_err()
    );
    assert!(futures::executor::block_on(validation.pop()).is_none());
}
