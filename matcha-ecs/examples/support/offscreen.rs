//! Reproducible native GPU visual verification using production extraction and
//! GuiRenderer. Run `showcase --offscreen target/showcase.png` without a window.
#[path = "allocations.rs"]
mod allocations;
use bevy_ecs::world::World;
use gpu_utils::gpu::{Gpu, GpuDescriptor};
use matcha_ecs::{
    components::view::ViewChildren,
    layout::{Constraints, layout_root},
    render::{GuiRenderer, extract_items},
    view::{Scope, run_view},
};

pub fn capture(view: impl FnOnce(&mut Scope), path: &str, size: [u32; 2]) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, view);
    layout_root(
        &mut world,
        root,
        Constraints::from_max_size(size.map(|v| v as f32)),
    );
    let frame = extract_items(&world, root);
    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor {
        required_features: wgpu::Features::empty(),
        ..Default::default()
    }))
    .expect("real GPU for visual proof");
    let (device, queue) = gpu.context().expect("initialized GPU");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("GUI proof"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let mut renderer = GuiRenderer::new(&device, &queue);
    for i in 0..4 {
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let allocation = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let start = std::time::Instant::now();
        let (assembled, allocations) = allocations::measure(|| {
            renderer.assemble(&frame.items, &frame.clips, size.map(|v| v as f32))
        });
        assembled.expect("native Scene assembly");
        let assembly_elapsed = start.elapsed();
        let encode_start = std::time::Instant::now();
        renderer
            .backend
            .render(
                &renderer.frame.scene,
                renderer::SceneTarget {
                    view: &target_view,
                    viewport: size.map(|v| v as f32),
                    clear: wgpu::Color {
                        r: 0.025,
                        g: 0.03,
                        b: 0.045,
                        a: 1.,
                    },
                    initial: None,
                },
            )
            .expect("production GUI renders");
        let encode_elapsed = encode_start.elapsed();
        let wait_start = std::time::Instant::now();
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("GPU completion");
        let error = futures::executor::block_on(allocation.pop());
        assert!(error.is_none(), "GPU allocation failed: {error:?}");
        let error = futures::executor::block_on(validation.pop());
        assert!(error.is_none(), "GPU validation failed: {error:?}");
        println!(
            "GUI frame {i}: {:?}; assembly={assembly_elapsed:?}, allocations={allocations:?}, encode/submit={encode_elapsed:?}, wait={:?}; {:?}",
            start.elapsed(),
            wait_start.elapsed(),
            renderer.backend.stats()
        );
        if i == 1 {
            assert_eq!(
                renderer.backend.stats().prepared,
                0,
                "static GUI reuses every source"
            );
        }
    }
    let row = (size[0] * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("GUI proof readback"),
        size: u64::from(row) * u64::from(size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(size[1]),
            },
        },
        target.size(),
    );
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("readback"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completion");
    let mapped = buffer.slice(..).get_mapped_range();
    let mut data = Vec::new();
    for line in mapped.chunks_exact(row as usize) {
        data.extend_from_slice(&line[..size[0] as usize * 4]);
    }
    image::save_buffer(path, &data, size[0], size[1], image::ColorType::Rgba8)
        .expect("save visual proof");
    println!(
        "Saved {path}; adapter {:?}; {} UI items",
        gpu.adapter().get_info(),
        frame.items.len()
    );
}
