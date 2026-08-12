//! A mask must land on the geometry it masks, even when the UI extent and the
//! attachment are not the same size.
//!
//! `render_flat` takes `destination_size` — the extent of the **UI coordinate
//! space** — and draws into an attachment whose size is in **physical pixels**.
//! The two are equal only by coincidence. A display scale (`UiScale` in
//! `matcha-ecs`) makes the UI extent deliberately smaller than the framebuffer,
//! and a surface that has not been reconfigured since the window changed makes
//! them differ by accident.
//!
//! Geometry survives that gap on its own: `make_normalize_matrix` maps the UI
//! extent onto the whole attachment, so a quad simply comes out scaled by
//! `attachment / destination_size` and still covers the right fraction of the
//! screen. **A mask did not.** It is inverse-mapped from the fragment's own
//! position, which is in physical pixels, through a matrix built from a
//! UI-space transform — so the lookup was displaced by that same ratio, and the
//! displacement grows linearly with distance from the origin. The visible
//! result is that unmasked quads render fine while every glyph, rounded corner
//! and clip is eaten into from one side, worse the further it sits from the
//! top-left corner.
//!
//! Both directions are pinned, because which side gets eaten depends on which
//! of the two sizes is larger, and only one of them would be caught by a test
//! that assumed a display scale is always greater than 1.

use std::sync::Arc;

use gpu_utils::gpu::{Gpu, GpuDescriptor};
use gpu_utils::texture_atlas::TextureAtlas;
use renderer::{CoreRenderer, FlatItem, RenderNode};

/// Attachment size, in physical pixels. `W * 4 == 256` keeps the readback copy
/// aligned without padding.
const W: u32 = 64;
const H: u32 = 64;
/// Non-sRGB, so a byte written into the atlas comes back out of the render pass
/// unchanged and the assertions can be exact.
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

/// One opaque red quad covering `[origin, origin + size)` in UI space, masked
/// by a fully-opaque coverage texel stretched over exactly the same rectangle.
///
/// Mask quad == texture quad is the glyph case, and the one where a
/// displacement is unambiguous: correct output is the quad drawn whole, so any
/// missing pixel is the mask landing somewhere it should not.
fn masked_quad(h: &Harness, origin: [f32; 2], size: [f32; 2]) -> FlatItem {
    let colour = h
        .texture_atlas
        .allocate(&h.device, &h.queue, [1, 1])
        .expect("colour atlas allocation failed");
    colour
        .write_data(&h.queue, &[255, 0, 0, 255])
        .expect("colour atlas write failed");

    let coverage = h
        .stencil_atlas
        .allocate(&h.device, &h.queue, [1, 1])
        .expect("coverage atlas allocation failed");
    coverage
        .write_data(&h.queue, &[255])
        .expect("coverage atlas write failed");

    let at = nalgebra::Matrix4::new_translation(&nalgebra::Vector3::new(origin[0], origin[1], 0.0));
    let node = RenderNode::new()
        .with_texture(colour, size, at)
        .with_stencil(coverage, size, at);
    FlatItem::new(Arc::new(node), nalgebra::Matrix4::identity())
}

/// Render into a fresh `W`x`H` attachment, declaring `destination_size` as the
/// UI extent, and read the result back as RGBA8.
fn render_to_pixels(h: &Harness, destination_size: [f32; 2], items: &[FlatItem]) -> Vec<u8> {
    let target = h.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mask_scale target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    h.core
        .render_flat(
            &h.device,
            &h.queue,
            FORMAT,
            &view,
            destination_size,
            items,
            &[],
            wgpu::Color::BLACK,
            &h.texture_atlas.texture(),
            &h.stencil_atlas.texture(),
        )
        .expect("render_flat failed");

    let bytes_per_row = W * 4;
    let readback = h.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mask_scale readback"),
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
    readback.unmap();
    data
}

fn is_red(data: &[u8], x: u32, y: u32) -> bool {
    let i = ((y * W + x) * 4) as usize;
    data[i] > 128 && data[i + 1] < 128
}

/// Assert the quad is drawn whole: red everywhere strictly inside its physical
/// rectangle, background everywhere strictly outside.
///
/// Sampled a pixel in from each edge, so nothing here depends on how a fragment
/// exactly on a boundary rounds.
#[track_caller]
fn assert_covers(data: &[u8], min: [u32; 2], max: [u32; 2], case: &str) {
    let corners = [
        (min[0] + 1, min[1] + 1, "top-left"),
        (max[0] - 2, min[1] + 1, "top-right"),
        (min[0] + 1, max[1] - 2, "bottom-left"),
        (max[0] - 2, max[1] - 2, "bottom-right"),
        ((min[0] + max[0]) / 2, (min[1] + max[1]) / 2, "centre"),
    ];
    for (x, y, corner) in corners {
        assert!(
            is_red(data, x, y),
            "{case}: the {corner} of the quad ({x}, {y}) was masked away. The mask \
             is displaced by the ratio between the attachment and the UI extent, \
             and the displacement grows with distance from the origin — so the \
             corners furthest from (0, 0) fail first.\n{}",
            ascii(data)
        );
    }
    assert!(
        !is_red(data, min[0].saturating_sub(2), min[1].saturating_sub(2)),
        "{case}: painted outside the quad, before it starts"
    );
    assert!(
        !is_red(data, max[0] + 2, max[1] + 2),
        "{case}: painted outside the quad, after it ends"
    );
}

/// The whole target as `#`/`.`, so a failure shows the shape of what was drawn
/// rather than one unhelpful coordinate.
fn ascii(data: &[u8]) -> String {
    let mut s = String::from("drawn:\n");
    for y in 0..H {
        for x in 0..W {
            s.push(if is_red(data, x, y) { '#' } else { '.' });
        }
        s.push('\n');
    }
    s
}

/// UI extent **smaller** than the attachment: the display-scale case, where
/// `UiScale` is above 1 and the framebuffer is deliberately denser than the UI.
#[test]
fn a_mask_lands_on_its_quad_when_the_ui_extent_is_smaller_than_the_attachment() {
    let Some(h) = harness() else {
        eprintln!("no GPU adapter available; skipping");
        return;
    };

    // UI space is 32x32 inside a 64x64 attachment, so one UI unit is two
    // pixels. The quad covers UI 8..24, i.e. physical 16..48.
    let items = [masked_quad(&h, [8.0, 8.0], [16.0, 16.0])];
    let data = render_to_pixels(&h, [32.0, 32.0], &items);

    assert_covers(&data, [16, 16], [48, 48], "ui extent 32 into a 64 attachment");
}

/// UI extent **larger** than the attachment — the opposite sign, which eats the
/// other side of every masked quad. Reachable whenever the surface is
/// configured smaller than the extent the frame was laid out for.
#[test]
fn a_mask_lands_on_its_quad_when_the_ui_extent_is_larger_than_the_attachment() {
    let Some(h) = harness() else {
        eprintln!("no GPU adapter available; skipping");
        return;
    };

    // UI space is 128x128 inside a 64x64 attachment: one UI unit is half a
    // pixel. The quad covers UI 32..96, i.e. physical 16..48 again.
    let items = [masked_quad(&h, [32.0, 32.0], [64.0, 64.0])];
    let data = render_to_pixels(&h, [128.0, 128.0], &items);

    assert_covers(&data, [16, 16], [48, 48], "ui extent 128 into a 64 attachment");
}

/// The case that always worked, kept so a fix cannot be "make everything pass
/// by ignoring the mask": at 1:1 the mask is exact, and it still has to reject
/// what falls outside it.
#[test]
fn a_mask_still_clips_when_the_ui_extent_matches_the_attachment() {
    let Some(h) = harness() else {
        eprintln!("no GPU adapter available; skipping");
        return;
    };

    let items = [masked_quad(&h, [16.0, 16.0], [32.0, 32.0])];
    let data = render_to_pixels(&h, [W as f32, H as f32], &items);

    assert_covers(&data, [16, 16], [48, 48], "1:1");
}
