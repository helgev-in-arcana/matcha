//! End-to-end check that a frame actually draws, over whichever
//! parameter-passing path is compiled in.
//!
//! The point is the `uniform-params` feature. WebGPU has no push constants, so
//! the web build carries [`FrameParams`] in a uniform buffer instead, via a
//! rewrite of the shared WGSL sources (see `core_renderer/frame_params.rs`).
//! That path's failure mode in a browser is a black canvas plus a console
//! validation error and nothing else, which is a miserable thing to debug — so
//! run the identical path here, on a real desktop GPU:
//!
//! ```text
//! cargo test -p renderer --test uniform_params
//! cargo test -p renderer --test uniform_params --features uniform-params
//! ```
//!
//! Both must pass, and must agree on the pixels. The compute stages are
//! separately covered under the same feature by the unit tests in
//! `core_renderer/stages.rs`; this covers the render pipeline, which is where
//! the bind group index and the uniform layout actually get exercised at draw
//! time.

use std::sync::Arc;

use gpu_utils::gpu::{Gpu, GpuDescriptor};
use gpu_utils::texture_atlas::TextureAtlas;
use renderer::{CoreRenderer, FlatItem, RenderNode};

const W: u32 = 64;
const H: u32 = 64;
/// Format of both the destination and the colour atlas. Non-sRGB so the bytes
/// written into the atlas come back out of the render pass unchanged, and the
/// assertions can be exact.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    core: CoreRenderer,
    texture_atlas: Arc<TextureAtlas>,
    stencil_atlas: Arc<TextureAtlas>,
}

fn harness() -> Option<Harness> {
    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor::default())).ok()?;
    let (device, queue) = gpu.context()?;
    // Small atlases: this test allocates two tiny regions and nothing else.
    let extent = wgpu::Extent3d {
        width: 256,
        height: 256,
        depth_or_array_layers: 1,
    };
    let texture_atlas = TextureAtlas::new(&device, extent, FORMAT, TextureAtlas::DEFAULT_MARGIN_PX);
    let stencil_atlas = TextureAtlas::new(
        &device,
        extent,
        wgpu::TextureFormat::R8Unorm,
        TextureAtlas::DEFAULT_MARGIN_PX,
    );
    let core = CoreRenderer::new(&device);
    Some(Harness {
        device,
        queue,
        core,
        texture_atlas,
        stencil_atlas,
    })
}

/// Render `items` into a fresh `W`x`H` target and read it back as RGBA8.
fn render_to_pixels(h: &Harness, items: &[FlatItem], clear: wgpu::Color) -> Vec<u8> {
    render_to_pixels_via(h, items, clear, FORMAT, None)
}

/// As [`render_to_pixels`], but with an explicit target format and an optional
/// *view* format to draw through — the arrangement the web uses, where the
/// canvas cannot be sRGB but the drawing must still be gamma-encoded.
fn render_to_pixels_via(
    h: &Harness,
    items: &[FlatItem],
    clear: wgpu::Color,
    target_format: wgpu::TextureFormat,
    view_format: Option<wgpu::TextureFormat>,
) -> Vec<u8> {
    let view_formats: &[wgpu::TextureFormat] = match &view_format {
        Some(f) => std::slice::from_ref(f),
        None => &[],
    };
    let target = h.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("uniform_params target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats,
    });
    let draw_format = view_format.unwrap_or(target_format);
    let view = target.create_view(&wgpu::TextureViewDescriptor {
        format: Some(draw_format),
        ..Default::default()
    });

    h.core
        .render_flat(
            &h.device,
            &h.queue,
            draw_format,
            &view,
            [W as f32, H as f32],
            items,
            &[],
            clear,
            &h.texture_atlas.texture(),
            &h.stencil_atlas.texture(),
        )
        .expect("render_flat failed");

    // `bytes_per_row` must be a multiple of 256; W * 4 == 256 here.
    let bytes_per_row = W * 4;
    assert_eq!(bytes_per_row % 256, 0, "test width must keep the copy aligned");
    let readback = h.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniform_params readback"),
        size: (bytes_per_row * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = h
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    h.queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map failed"));
    h.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll failed while mapping readback");
    let data = slice.get_mapped_range().to_vec();
    drop(slice);
    readback.unmap();
    data
}

fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

/// A solid opaque quad, drawn through the whole pipeline, lands where it was
/// asked to and leaves the rest of the target at the clear colour.
///
/// This is deliberately the dullest possible frame: if the parameter block does
/// not reach the shaders, `normalize_matrix` is all zeros and the quad
/// collapses, so the centre stays at the clear colour and this fails.
#[test]
fn a_quad_draws_where_it_was_placed() {
    let Some(h) = harness() else {
        eprintln!("no GPU adapter available; skipping");
        return;
    };

    // A 1x1 opaque red texel, stretched over the quad.
    let region = h
        .texture_atlas
        .allocate(&h.device, &h.queue, [1, 1])
        .expect("atlas allocation failed");
    region
        .write_data(&h.queue, &[255, 0, 0, 255])
        .expect("atlas write failed");

    // Cover the middle half of the target: 16,16 .. 48,48.
    let node = Arc::new(RenderNode::new().with_texture(
        region,
        [32.0, 32.0],
        nalgebra::Matrix4::new_translation(&nalgebra::Vector3::new(16.0, 16.0, 0.0)),
    ));
    let items = [FlatItem::new(node, nalgebra::Matrix4::identity())];

    let data = render_to_pixels(&h, &items, wgpu::Color::BLACK);

    assert_eq!(
        pixel(&data, 32, 32),
        [255, 0, 0, 255],
        "the quad's centre should be opaque red — an all-zero parameter block \
         would collapse the quad and leave this at the clear colour"
    );
    assert_eq!(
        pixel(&data, 2, 2),
        [0, 0, 0, 255],
        "outside the quad should still be the clear colour"
    );
    // Corners of the quad, a texel inside, to pin the placement rather than
    // just "something red got drawn".
    assert_eq!(pixel(&data, 17, 17), [255, 0, 0, 255], "top-left inside");
    assert_eq!(pixel(&data, 46, 46), [255, 0, 0, 255], "bottom-right inside");
    assert_eq!(pixel(&data, 14, 14), [0, 0, 0, 255], "just outside the quad");
}

/// An sRGB *view* of a non-sRGB target encodes exactly like a natively sRGB
/// target.
///
/// This is the arrangement the web build is forced into: WebGPU accepts only
/// non-sRGB canvas formats, so the canvas is configured `Rgba8Unorm` and drawn
/// through an `Rgba8UnormSrgb` view (`window_config/web.rs`,
/// `WindowSurface::format`). The whole point is to keep the automatic
/// linear->sRGB encode that the pipeline — and `linear_to_srgb_u8` on the upload
/// side — depend on; configuring non-sRGB and *dropping* the view format would
/// silently change every colour in the app.
///
/// Assert equivalence rather than specific bytes: the claim is "the web path and
/// the native path agree", which is exactly what a future maintainer would break.
#[test]
fn an_srgb_view_of_a_non_srgb_target_matches_a_native_srgb_target() {
    let Some(h) = harness() else {
        eprintln!("no GPU adapter available; skipping");
        return;
    };

    // Mid-grey is the interesting value: 0.5 linear is nowhere near 0.5
    // encoded, so a missing encode is glaring rather than subtle. It has to
    // arrive as a *drawn* quad, not a clear colour — `render_flat` returns
    // early when there are no instances, so an empty frame never touches the
    // target at all.
    let region = h
        .texture_atlas
        .allocate(&h.device, &h.queue, [1, 1])
        .expect("atlas allocation failed");
    // The atlas is non-sRGB, so this samples as 128/255 = 0.502 linear.
    region
        .write_data(&h.queue, &[128, 128, 128, 255])
        .expect("atlas write failed");

    let node = Arc::new(RenderNode::new().with_texture(
        region,
        [W as f32, H as f32],
        nalgebra::Matrix4::identity(),
    ));
    let items = [FlatItem::new(node, nalgebra::Matrix4::identity())];

    let native_srgb = render_to_pixels_via(
        &h,
        &items,
        wgpu::Color::BLACK,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        None,
    );
    let via_view = render_to_pixels_via(
        &h,
        &items,
        wgpu::Color::BLACK,
        wgpu::TextureFormat::Rgba8Unorm,
        Some(wgpu::TextureFormat::Rgba8UnormSrgb),
    );

    assert_eq!(
        pixel(&native_srgb, 32, 32),
        pixel(&via_view, 32, 32),
        "drawing through an sRGB view of a non-sRGB target must encode \
         identically to a natively sRGB target"
    );
    // And the encode really did happen: 0.5 linear encodes to ~188, not ~128.
    let [r, ..] = pixel(&via_view, 32, 32);
    assert!(
        r > 170,
        "0.5 linear should encode to about 188 in sRGB; got {r}, which looks \
         like no encode was applied at all"
    );
}

/// Draw-time alpha reaches the shader, and composites against the clear colour.
///
/// `alpha` rides in `InstanceData`, not in the parameter block, so this mostly
/// guards the render pipeline as a whole — but it fails loudly if the render
/// pass silently draws nothing, which is the exact web failure mode.
#[test]
fn draw_time_alpha_blends_against_the_target() {
    let Some(h) = harness() else {
        eprintln!("no GPU adapter available; skipping");
        return;
    };

    let region = h
        .texture_atlas
        .allocate(&h.device, &h.queue, [1, 1])
        .expect("atlas allocation failed");
    region
        .write_data(&h.queue, &[255, 255, 255, 255])
        .expect("atlas write failed");

    let node = Arc::new(RenderNode::new().with_texture(
        region,
        [32.0, 32.0],
        nalgebra::Matrix4::new_translation(&nalgebra::Vector3::new(16.0, 16.0, 0.0)),
    ));
    let items = [FlatItem::new(node, nalgebra::Matrix4::identity()).with_alpha(0.5)];

    let data = render_to_pixels(&h, &items, wgpu::Color::BLACK);

    let [r, g, b, _] = pixel(&data, 32, 32);
    // Premultiplied blending over black: half-intensity white.
    for (channel, value) in [("r", r), ("g", g), ("b", b)] {
        assert!(
            (100..=155).contains(&value),
            "{channel} should be about half intensity over black, got {value}"
        );
    }
}
