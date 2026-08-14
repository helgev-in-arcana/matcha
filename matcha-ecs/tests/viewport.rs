//! The frame's extent must agree with the framebuffer it is drawn into.
//!
//! Three things have to describe the same rectangle every frame: the
//! constraints `run_layout` gives the view tree, the `destination_size` the
//! renderer normalises against, and the surface actually being drawn into. If
//! they drift apart the frame is laid out for one size and presented at
//! another, which shows up as content that is cropped or letterboxed rather
//! than merely mis-scaled.
//!
//! The gap these pin is a **scale change with no resize behind it**. A browser
//! zoom moves `devicePixelRatio` while the CSS size stays put, and a display
//! scale change can move the window's physical size without a `Resized`
//! following; either way `ScaleFactorChanged` is the only event that arrives.
//! Before the fix that arm swapped `UiScale` and redrew without touching the
//! surface, and no other path would ever re-sync it — `WindowSurface::
//! reconfigure` is otherwise reachable only from `get_surface_texture`'s
//! `Outdated`/`Lost` arms, which WebGPU never reports.
//!
//! Runs on the real `Adapter` + `UiEcs` with headless windows and wgpu's noop
//! backend, like `tests/headless_app.rs`. No GPU, no OS window: the surface
//! stays absent, but `WindowSurface` tracks its configuration regardless
//! (`resize`/`reconfigure` write `current_config` whether or not a surface is
//! attached to hand it to), which is exactly the state under test.

use gpu_utils::gpu::GpuDescriptor;
use matcha_ecs::{
    components::layout::LayoutOutput,
    components::view::ViewChildren,
    ui_ecs::UiEcs,
    view::Scope,
};
use matcha_ecs_widgets::ColorRect;
use matcha_window::{
    adapter::Adapter,
    event::window_event::WindowEvent,
    headless_interface::{HeadlessEventLoop, HeadlessEventLoopProxy},
    window::WindowId,
};

/// Big enough that the constraints always clamp it, so its `LayoutOutput` is
/// the viewport rather than its own declared size.
fn view(_model: &(), s: &mut Scope) {
    s.leaf(ColorRect::new(100_000.0, 100_000.0));
}

fn reduce(_model: &mut (), _msg: ()) {}

type TestApp = UiEcs<(), (), fn(&(), &mut Scope), fn(&mut (), ())>;

struct Harness {
    adapter: Adapter<TestApp>,
    event_loop: HeadlessEventLoop,
    window_id: WindowId,
}

fn boot() -> Harness {
    let app: TestApp = UiEcs::new_with_gpu((), view, reduce, GpuDescriptor::noop());
    let mut adapter = Adapter::new(app);
    let event_loop = HeadlessEventLoop::new();

    adapter.init(Box::new(HeadlessEventLoopProxy::new()), &event_loop);
    adapter.resumed(&event_loop);
    adapter.create_surface(&event_loop);

    let window_id = *event_loop
        .created_window_ids()
        .first()
        .expect("UiEcs::resumed creates exactly one window");
    adapter.render(window_id);

    Harness {
        adapter,
        event_loop,
        window_id,
    }
}

impl Harness {
    fn viewport(&self) -> [f32; 2] {
        self.adapter
            .app()
            .viewport_size(self.window_id)
            .expect("the window exists")
    }

    /// What the surface is configured to, in physical pixels — the framebuffer
    /// a frame would actually land in.
    fn framebuffer(&self) -> [u32; 2] {
        let config = self
            .adapter
            .app()
            .window(self.window_id)
            .expect("the window exists")
            .surface()
            .surface_config();
        [config.width, config.height]
    }

    /// The size layout resolved the root's child to. Requires a render, since
    /// `run_layout` only runs inside the render schedule.
    fn laid_out(&mut self) -> [f32; 2] {
        self.adapter.render(self.window_id);
        let world = self.adapter.app().world();
        let root = matcha_ecs::resources::ui_root(world).expect("the window root exists");
        let child = world
            .get::<ViewChildren>(root)
            .expect("root has ViewChildren")
            .slots[0]
            .1;
        world
            .get::<LayoutOutput>(child)
            .expect("the child was laid out")
            .size
    }

    fn scale_factor_changed(&mut self, scale_factor: f64) {
        self.adapter.window_event(
            &self.event_loop,
            self.window_id,
            WindowEvent::ScaleFactorChanged { scale_factor },
        );
    }
}

#[test]
fn the_viewport_is_the_framebuffer_at_scale_one() {
    let h = boot();
    let [fb_w, fb_h] = h.framebuffer();
    assert_eq!(h.viewport(), [fb_w as f32, fb_h as f32]);
}

#[test]
fn a_scale_change_shrinks_the_viewport_by_exactly_that_factor() {
    // The framebuffer does not move — this is the browser-zoom shape, where
    // devicePixelRatio changes and the device-pixel content box does not.
    let mut h = boot();
    let before = h.framebuffer();

    h.scale_factor_changed(2.0);

    assert_eq!(h.framebuffer(), before, "the framebuffer must not have moved");
    assert_eq!(
        h.viewport(),
        [before[0] as f32 / 2.0, before[1] as f32 / 2.0],
        "the UI extent must halve so the same frame still fills the framebuffer"
    );
}

#[test]
fn a_scale_change_reconfigures_a_window_that_resized_without_a_resized_event() {
    // The gap this whole file exists for. A display-scale change can move the
    // window's physical size with `ScaleFactorChanged` as the only event; the
    // surface has to pick that up here or nothing ever will.
    let mut h = boot();
    let before = h.framebuffer();

    h.adapter
        .app()
        .window(h.window_id)
        .expect("the window exists")
        .request_inner_size(before[0] * 2, before[1] * 2);
    // Deliberately no `WindowEvent::Resized`.
    h.scale_factor_changed(2.0);

    assert_eq!(
        h.framebuffer(),
        [before[0] * 2, before[1] * 2],
        "the surface must be reconfigured to the window's new size"
    );
    assert_eq!(
        h.viewport(),
        [before[0] as f32, before[1] as f32],
        "twice the pixels at twice the density is the same UI extent"
    );
}

#[test]
fn layout_is_given_exactly_the_viewport() {
    // The invariant that makes the other three matter: whatever the viewport
    // resolves to, that is the box the view tree is laid out into. A widget
    // larger than the window is clamped to it, so its resolved size *is* the
    // constraints.
    let mut h = boot();
    assert_fills(h.laid_out(), h.viewport());

    h.scale_factor_changed(1.5);
    let (laid_out, viewport) = (h.laid_out(), h.viewport());
    assert_fills(laid_out, viewport);
    assert!(
        viewport[0] < 700.0,
        "the scale change should have shrunk the viewport at all; got {viewport:?}"
    );
}

/// `Constraints` stores its bounds quantised to `u32` (see `layout.rs`), so a
/// viewport with a repeating fraction — 1600/1.5 — comes back a fraction of a
/// pixel off. That rounding is the layout system's, not a lag: a whole pixel
/// of disagreement would be.
#[track_caller]
fn assert_fills(laid_out: [f32; 2], viewport: [f32; 2]) {
    for axis in 0..2 {
        assert!(
            (laid_out[axis] - viewport[axis]).abs() < 1.0,
            "layout resolved {laid_out:?} against a viewport of {viewport:?}"
        );
    }
}
