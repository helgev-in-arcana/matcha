//! Real GPU proofs through the public tree context, WidgetPod and shared builder.
//! No OS window, legacy RenderNode, atlas bridge or GPU-less fallback is involved.
#![cfg(not(target_arch = "wasm32"))]

use std::{
    io::Cursor,
    path::{Component, PathBuf},
    sync::{Mutex, MutexGuard},
};

use gpu_utils::gpu::{Gpu, GpuDescriptor};
use matcha_tree::{
    color::Color,
    ui_tree::{
        context::{Runtime, UiContext, with_offscreen_context},
        metrics::Constraints,
        widget::{View, WidgetInteractionResult, WidgetPod},
    },
};
use matcha_tree_widgets::{
    layout::{
        grid::Grid,
        position::Position,
        row::Row,
        visibility::{Visibility, VisibilityState},
    },
    style::{
        image::Image as ImageStyle,
        polygon::{Mesh, Polygon, Vertex},
        solid_box::SolidBox,
        viewport_clear::ViewportClear,
    },
    types::{grow_size::GrowSize, size::Size},
    widget::{image::Image, plain::Plain},
};
use render_interface::{Matrix4, TextureId};
use scene_builder::Frame;
use scene_renderer::{SceneRenderer, SceneTarget};

const EDGE: u32 = 128;

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn fractional_outputs_preserve_pixel_scale_and_natural_images_use_their_actual_region() {
    let _serial = serial();
    let h = Harness::new();
    let validation = h.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = SceneRenderer::new(&h.device, &h.queue);
    let mut frame = Frame::default();
    let stripe = Polygon::new(Mesh::TriangleList {
        vertices: [
            [5., 0.],
            [5., 10.5],
            [6., 10.5],
            [5., 0.],
            [6., 10.5],
            [6., 0.],
        ]
        .into_iter()
        .map(|position| Vertex {
            position,
            color: Color::rgb(255, 0, 0),
        })
        .collect(),
    });
    // Clear establishes the full 10.5-wide region. Mapping all 11 allocated
    // texels back into 10.5 pixels would shrink and blur the one-pixel stripe.
    let view = Plain::new()
        .style(ViewportClear::new(Color::TRANSPARENT))
        .style(stripe);
    let mut pod = h.context([128.; 2], |ctx| view.build(ctx));
    h.emit(&mut pod, &mut frame, [10.5; 2]);
    let fractional = h.render(&mut renderer, &frame);
    close(pixel(&fractional, 4, 3), [0; 4]);
    close(pixel(&fractional, 5, 3), [255, 0, 0, 255]);
    close(pixel(&fractional, 6, 3), [0; 4]);
    save_proof("fractional-stripe", &fractional);

    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        3,
        2,
        image::Rgba([255, 0, 0, 255]),
    ))
    .write_to(&mut encoded, image::ImageFormat::Png)
    .expect("natural-size PNG");
    let png = encoded.into_inner();
    let mut natural = h.context([128.; 2], |ctx| Image::new(png.clone()).build(ctx));
    h.emit(&mut natural, &mut frame, [64.; 2]);
    let ids = textures(&frame);
    assert_eq!(ids.len(), 1);
    assert_eq!(
        frame
            .scene
            .resources
            .texture(ids[0])
            .expect("image definition")
            .descriptor()
            .size,
        [3, 2]
    );
    let small = h.render(&mut renderer, &frame);
    close(pixel(&small, 1, 1), [255, 0, 0, 255]);
    close(pixel(&small, 4, 1), [0; 4]);
    h.emit(&mut natural, &mut frame, [128.; 2]);
    assert_eq!(
        textures(&frame),
        ids,
        "unused parent space does not change image content"
    );
    assert_eq!(small, h.render(&mut renderer, &frame));
    assert_eq!(renderer.stats().prepared, 0);
    save_proof("natural-image", &small);

    let view = Plain::new().style(ImageStyle::new(png).size_px(8., 8.).offset_px(6., 0.));
    let mut clipped = h.context([128.; 2], |ctx| view.build(ctx));
    h.emit(&mut clipped, &mut frame, [8.; 2]);
    let ids = textures(&frame);
    assert_eq!(ids.len(), 1);
    assert_eq!(
        frame
            .scene
            .resources
            .texture(ids[0])
            .expect("clipped definition")
            .descriptor()
            .size,
        [2, 8]
    );
    let image = h.render(&mut renderer, &frame);
    close(pixel(&image, 5, 3), [0; 4]);
    close(pixel(&image, 6, 3), [255, 0, 0, 255]);
    close(pixel(&image, 9, 3), [0; 4]);
    save_proof("offset-style-clip", &image);
    assert!(futures::executor::block_on(validation.pop()).is_none());
}
struct Harness {
    gpu: Gpu,
    device: wgpu::Device,
    queue: wgpu::Queue,
    runtime: Runtime,
    output: wgpu::Texture,
}
impl Harness {
    fn new() -> Self {
        let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor {
            backends: match std::env::var("MATCHA_TEST_BACKEND").as_deref() {
                Ok("vulkan") => wgpu::Backends::VULKAN,
                Ok("dx12") => wgpu::Backends::DX12,
                _ => wgpu::Backends::PRIMARY,
            },
            ..GpuDescriptor::standard()
        }))
        .expect("real GPU required for native widget proofs");
        let (device, queue) = gpu.context().expect("GPU ready");
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tree widget pixels"),
            size: wgpu::Extent3d {
                width: EDGE,
                height: EDGE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let runtime = Runtime::from_tokio(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("fixture runtime"),
        );
        Self {
            gpu,
            device,
            queue,
            runtime,
            output,
        }
    }
    fn context<R>(&self, viewport: [f32; 2], run: impl for<'a> FnOnce(&UiContext<'a>) -> R) -> R {
        with_offscreen_context(
            self.runtime.handle().clone(),
            self.gpu.instance(),
            &self.device,
            &self.queue,
            viewport,
            self.output.format(),
            run,
        )
    }
    fn emit(&self, pod: &mut WidgetPod, frame: &mut Frame, bounds: [f32; 2]) {
        frame.begin();
        self.context([EDGE as f32; 2], |ctx| {
            let mut draw = frame.draw(Matrix4::identity(), None, 1.);
            pod.render(bounds, ctx, &mut draw);
        });
        frame.finish().expect("valid tree submission");
    }
    fn render(&self, renderer: &mut SceneRenderer, frame: &Frame) -> Vec<u8> {
        renderer
            .render(
                &frame.scene,
                SceneTarget {
                    view: &self.output.create_view(&Default::default()),
                    format: self.output.format(),
                    viewport: [EDGE as f32; 2],
                    clear: wgpu::Color::TRANSPARENT,
                    initial: None,
                },
            )
            .expect("native tree Scene renders");
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tree readback"),
            size: u64::from(EDGE * EDGE * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(EDGE * 4),
                    rows_per_image: Some(EDGE),
                },
            },
            self.output.size(),
        );
        self.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
            result.expect("map proof pixels")
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("GPU completion");
        buffer.slice(..).get_mapped_range().to_vec()
    }
}
fn pixel(bytes: &[u8], x: usize, y: usize) -> [u8; 4] {
    bytes[(y * EDGE as usize + x) * 4..(y * EDGE as usize + x + 1) * 4]
        .try_into()
        .expect("RGBA pixel")
}
fn close(actual: [u8; 4], expected: [u8; 4]) {
    assert!(
        actual
            .into_iter()
            .zip(expected)
            .all(|(a, b)| a.abs_diff(b) <= 2),
        "actual {actual:?}, expected {expected:?}"
    );
}
fn textures(frame: &Frame) -> Vec<TextureId> {
    let mut ids: Vec<_> = frame.scene.resources.texture_ids().collect();
    ids.sort_unstable_by_key(|id| id.get());
    ids
}
fn solid(color: Color, size: [f32; 2]) -> Plain {
    Plain::new()
        .size(size.map(Size::px))
        .style(SolidBox::new(color))
}

/// Optional visual proof output. Values are linear-premultiplied in readback;
/// PNG presentation uses straight sRGB RGB with unchanged alpha. Assertions keep
/// using the raw GPU bytes. Set MATCHA_TREE_PROOF_OUTPUT to a folder under target/.
fn save_proof(name: &str, bytes: &[u8]) {
    let Ok(folder) = std::env::var("MATCHA_TREE_PROOF_OUTPUT") else {
        return;
    };
    let folder = PathBuf::from(folder);
    assert!(
        folder
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
        "proof output must remain in target/"
    );
    // Cargo starts integration tests in the package directory, not the workspace.
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("widget crate is a workspace member")
        .join("target")
        .join(folder);
    std::fs::create_dir_all(&directory).expect("proof directory");
    let mut display = bytes.to_vec();
    for pixel in display.chunks_exact_mut(4) {
        let alpha = pixel[3] as f32 / 255.;
        for channel in &mut pixel[..3] {
            let linear = if alpha > 0. {
                (*channel as f32 / 255. / alpha).clamp(0., 1.)
            } else {
                0.
            };
            let encoded = if linear <= 0.0031308 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1. / 2.4) - 0.055
            };
            *channel = (encoded * 255.).round() as u8;
        }
    }
    image::save_buffer(
        directory.join(format!("{name}.png")),
        &display,
        EDGE,
        EDGE,
        image::ColorType::Rgba8,
    )
    .expect("save proof image");
}

#[test]
fn stock_sources_reuse_change_prune_and_relocate_through_the_real_tree() {
    let _serial = serial();
    let h = Harness::new();
    let validation = h.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = SceneRenderer::new(&h.device, &h.queue);
    let mut frame = Frame::default();
    let size = [Size::px(64.), Size::px(64.)];
    let mut view = Row::new().push(
        Plain::new()
            .size(size.clone())
            .style(SolidBox::new(Color::rgb(255, 0, 0))),
    );
    let mut pod = h.context([128.; 2], |ctx| view.build(ctx));
    h.emit(&mut pod, &mut frame, [128.; 2]);
    let original = h.render(&mut renderer, &frame);
    close(pixel(&original, 16, 16), [255, 0, 0, 255]);
    close(pixel(&original, 80, 16), [0; 4]);
    let old_ids = textures(&frame);
    h.emit(&mut pod, &mut frame, [128.; 2]);
    assert_eq!(old_ids, textures(&frame));
    assert_eq!(original, h.render(&mut renderer, &frame));
    assert_eq!(renderer.stats().prepared, 0);
    let relocation = renderer.compact_resources().expect("GPU-only relocation");
    assert_eq!(relocation.textures, 1);
    assert_eq!(original, h.render(&mut renderer, &frame));
    assert_eq!(renderer.stats().prepared, 0);

    // Keep layout functions/settings identical; only the child's painter changes.
    // The redraw must propagate through Row without reporting a layout change.
    view.items[0] = Box::new(
        Plain::new()
            .size(size.clone())
            .style(SolidBox::new(Color::rgb(0, 255, 0))),
    );
    h.context([128.; 2], |ctx| {
        assert!(matches!(
            pod.try_update(&view, ctx),
            Ok(WidgetInteractionResult::RedrawNeeded)
        ))
    });
    h.emit(&mut pod, &mut frame, [128.; 2]);
    assert_ne!(old_ids, textures(&frame));
    assert!(
        old_ids
            .iter()
            .all(|id| frame.scene.resources.texture(*id).is_none())
    );
    close(
        pixel(&h.render(&mut renderer, &frame), 16, 16),
        [0, 255, 0, 255],
    );

    // Removing every style must remove its old logical image and request redraw.
    view.items[0] = Box::new(Plain::new().size(size));
    h.context([128.; 2], |ctx| {
        assert!(matches!(
            pod.try_update(&view, ctx),
            Ok(WidgetInteractionResult::RedrawNeeded)
        ))
    });
    h.emit(&mut pod, &mut frame, [128.; 2]);
    assert!(textures(&frame).is_empty());
    assert!(
        h.render(&mut renderer, &frame)
            .iter()
            .all(|byte| *byte == 0)
    );
    assert!(futures::executor::block_on(validation.pop()).is_none());
}

#[test]
fn gradients_private_clears_and_png_alpha_keep_their_visual_meaning() {
    let _serial = serial();
    let h = Harness::new();
    let validation = h.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = SceneRenderer::new(&h.device, &h.queue);
    let mut frame = Frame::default();
    let triangle = Polygon::new(Mesh::TriangleList {
        vertices: vec![
            Vertex {
                position: [0., 0.],
                color: Color::rgb(255, 0, 0),
            },
            Vertex {
                position: [64., 0.],
                color: Color::rgb(0, 255, 0),
            },
            Vertex {
                position: [0., 64.],
                color: Color::rgb(0, 0, 255),
            },
        ],
    });
    let mut gradient = h.context([128.; 2], |ctx| Plain::new().style(triangle).build(ctx));
    h.emit(&mut gradient, &mut frame, [64.; 2]);
    let image = h.render(&mut renderer, &frame);
    close(pixel(&image, 8, 8), [187, 34, 34, 255]);
    close(pixel(&image, 40, 8), [60, 161, 34, 255]);
    close(pixel(&image, 48, 48), [0; 4]);
    save_proof("gradient", &image);
    h.emit(&mut gradient, &mut frame, [64.; 2]);
    assert_eq!(image, h.render(&mut renderer, &frame));
    assert_eq!(renderer.stats().prepared, 0);

    let view = Plain::new()
        .style(SolidBox::new(Color::rgb(255, 0, 0)))
        .content(
            Plain::new()
                .style(SolidBox::new(Color::rgb(0, 255, 0)))
                .style(ViewportClear::new(Color::TRANSPARENT)),
        );
    let mut cleared = h.context([128.; 2], |ctx| view.build(ctx));
    h.emit(&mut cleared, &mut frame, [64.; 2]);
    let private_clear = h.render(&mut renderer, &frame);
    close(pixel(&private_clear, 32, 32), [255, 0, 0, 255]);
    save_proof("private-clear", &private_clear);

    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        2,
        image::Rgba([255, 0, 0, 128]),
    ))
    .write_to(&mut png, image::ImageFormat::Png)
    .expect("in-memory PNG");
    let view = Plain::new()
        .style(SolidBox::new(Color::rgb(0, 0, 255)))
        .content(
            Position::new()
                .left(12.)
                .top(9.)
                .content(Image::new(png.into_inner()).size([Size::px(8.), Size::px(8.)])),
        );
    let mut alpha = h.context([128.; 2], |ctx| view.build(ctx));
    h.emit(&mut alpha, &mut frame, [64.; 2]);
    let image = h.render(&mut renderer, &frame);
    close(pixel(&image, 10, 10), [0, 0, 255, 255]);
    close(pixel(&image, 14, 11), [128, 0, 127, 255]);
    close(pixel(&image, 21, 11), [0, 0, 255, 255]);
    save_proof("png-alpha", &image);
    h.emit(&mut alpha, &mut frame, [64.; 2]);
    assert_eq!(image, h.render(&mut renderer, &frame));
    assert_eq!(renderer.stats().prepared, 0);
    assert!(futures::executor::block_on(validation.pop()).is_none());
}

#[test]
fn row_grid_position_resize_and_visibility_emit_resolved_geometry() {
    let _serial = serial();
    let h = Harness::new();
    let validation = h.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut renderer = SceneRenderer::new(&h.device, &h.queue);
    let mut frame = Frame::default();
    let relative = Plain::new().size([Size::vw(0.5), Size::vh(0.25)]);
    for viewport in [[64.; 2], [128.; 2]] {
        h.context(viewport, |ctx| {
            assert_eq!(ctx.dpi(), Some(1.));
            assert_eq!(ctx.surface_format(), Some(wgpu::TextureFormat::Rgba8Unorm));
            assert_eq!(ctx.viewport_size(), Some(viewport));
            let config = ctx.window_config().expect("offscreen window metadata");
            assert_eq!(config.surface_config.width, viewport[0] as u32);
            assert_eq!(config.surface_config.height, viewport[1] as u32);
            assert_eq!(
                relative
                    .build(ctx)
                    .measure(&Constraints::from_boundary(viewport), ctx),
                [viewport[0] * 0.5, viewport[1] * 0.25]
            );
        });
    }
    let mut moved = Position::new().left(4.).top(6.).content(
        Row::new()
            .push(solid(Color::rgb(255, 0, 0), [16., 16.]))
            .push(solid(Color::rgb(0, 255, 0), [24., 16.])),
    );
    let mut pod = h.context([128.; 2], |ctx| moved.build(ctx));
    h.emit(&mut pod, &mut frame, [128.; 2]);
    let first = h.render(&mut renderer, &frame);
    close(pixel(&first, 8, 10), [255, 0, 0, 255]);
    close(pixel(&first, 26, 10), [0, 255, 0, 255]);
    let ids = textures(&frame);
    moved.left = Some(20.);
    h.context([128.; 2], |ctx| {
        assert!(pod.try_update(&moved, ctx).is_ok());
    });
    h.emit(&mut pod, &mut frame, [128.; 2]);
    let shifted = h.render(&mut renderer, &frame);
    close(pixel(&shifted, 8, 10), [0; 4]);
    close(pixel(&shifted, 24, 10), [255, 0, 0, 255]);
    close(pixel(&shifted, 42, 10), [0, 255, 0, 255]);
    save_proof("row-position-moved", &shifted);
    assert_eq!(ids, textures(&frame));
    assert_eq!(renderer.stats().prepared, 0);

    let grid = Grid::new()
        .template_columns(vec![GrowSize::Grow(Size::px(1.)); 2])
        .template_rows(vec![GrowSize::Grow(Size::px(1.))])
        .item(solid(Color::rgb(255, 0, 0), [1.; 2]), [0, 1], [0, 1])
        .item(solid(Color::rgb(0, 0, 255), [1.; 2]), [1, 2], [0, 1]);
    let mut grid = h.context([128.; 2], |ctx| grid.build(ctx));
    h.emit(&mut grid, &mut frame, [64.; 2]);
    let small = h.render(&mut renderer, &frame);
    close(pixel(&small, 16, 16), [255, 0, 0, 255]);
    close(pixel(&small, 48, 16), [0, 0, 255, 255]);
    close(pixel(&small, 80, 16), [0; 4]);
    h.emit(&mut grid, &mut frame, [128.; 2]);
    let large = h.render(&mut renderer, &frame);
    close(pixel(&large, 48, 16), [255, 0, 0, 255]);
    close(pixel(&large, 96, 16), [0, 0, 255, 255]);
    save_proof("grid-resized", &large);

    let mut visible = Visibility::new().content(solid(Color::rgb(255, 0, 0), [16.; 2]));
    let mut pod = h.context([128.; 2], |ctx| visible.build(ctx));
    h.emit(&mut pod, &mut frame, [16.; 2]);
    let before = h.render(&mut renderer, &frame);
    for state in [VisibilityState::Hidden, VisibilityState::Gone] {
        visible.visibility = state;
        h.context([128.; 2], |ctx| {
            assert!(pod.try_update(&visible, ctx).is_ok());
        });
        h.emit(&mut pod, &mut frame, [16.; 2]);
        assert!(textures(&frame).is_empty());
        assert!(
            h.render(&mut renderer, &frame)
                .iter()
                .all(|byte| *byte == 0)
        );
    }
    visible.visibility = VisibilityState::Visible;
    h.context([128.; 2], |ctx| {
        assert!(pod.try_update(&visible, ctx).is_ok());
    });
    h.emit(&mut pod, &mut frame, [16.; 2]);
    assert_eq!(before, h.render(&mut renderer, &frame));
    assert_eq!(renderer.stats().prepared, 0);
    assert!(futures::executor::block_on(validation.pop()).is_none());
}
