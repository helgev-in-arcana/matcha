use matcha_window::event::window_event::WindowEvent;
use matcha_window::headless_interface::HeadlessEventLoop;

use super::*;
use crate::ui_tree::window::{Apply, Window, WindowConfig, WindowRegistry};

struct DpiFromViewport(ProbeView);
struct DpiFromViewportWidget;

impl View for DpiFromViewport {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        ctx.child(&self.0);
        DpiFromViewportWidget
    }
}

impl Widget for DpiFromViewportWidget {
    type V = DpiFromViewport;

    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        ctx.child(&view.0);
        UpdateResult::empty()
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }

    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds::default()
    }

    fn layout(&self, _: Bounds, mut children: Children<'_>, ctx: &mut Context) {
        let dpi = f64::from(ctx.viewport_size()[0]) / 100.0;
        children.for_each(|_, mut child| {
            // The same native viewport, constraints and assigned bounds throughout this pass.
            let mut check_scales = |ctx: &mut Context, scales: [f64; 3]| {
                for scale in scales {
                    let mut child_ctx = ctx.reborrow().with_dpi(scale);
                    let measured = child.measure(Constraints::default(), &mut child_ctx);
                    assert_eq!(measured.x, bounds().x * scale as f32);
                    child.layout(position(Matrix4::identity()), &mut child_ctx);
                }
            };
            check_scales(ctx, [dpi, dpi, dpi * 2.0]);
            let mut independent = Context::new(ctx.window_registry());
            check_scales(&mut independent, [dpi * 3.0, dpi * 3.0, dpi * 4.0]);
        });
    }

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        Draw::default()
    }
}

#[test]
fn local_dpi_keys_child_geometry_inside_a_resized_window() {
    let observed = Arc::new(Mutex::new(Observations::default()));
    let view =
        Window::new(DpiFromViewport(ProbeView(observed.clone()))).with_config(WindowConfig {
            inner_size: Some(Apply::Init([100, 80].into())),
            ..WindowConfig::default()
        });
    let event_loop = HeadlessEventLoop::new();
    let mut open = windows::headless_factory(&event_loop);
    let mut registry = WindowRegistry::default();
    let mut root = Context::new(&mut registry);
    let mut ctx = root.with_window_factory(&mut open);
    let mut tree = UiTree::new();
    tree.try_update(&view, &mut ctx).unwrap();
    let id = ctx.window_registry().ids()[0];
    assert!(tree.layout_window(id, &mut ctx));
    let next_time = ctx.frame_time() + std::time::Duration::from_secs(1);
    assert!(tree.layout_window(id, &mut ctx.reborrow().with_frame_time(next_time)));
    ctx.window_registry()
        .window(id)
        .unwrap()
        .request_inner_size(300, 80);
    let event = WindowEvent::Resized {
        inner_size: [300.0, 80.0],
        outer_size: [300.0, 80.0],
    };
    assert!(tree.window_event(id, &event, &mut ctx).is_some());
    assert!(tree.layout_window(id, &mut ctx));
    tree.render_window(id, &mut ctx).unwrap();
    let observed = observed.lock().unwrap();
    for phase in ["measure", "layout"] {
        let samples: Vec<_> = observed
            .scopes
            .iter()
            .filter_map(|(stage, dpi, _)| (*stage == phase).then_some(*dpi))
            .collect();
        assert_eq!(
            samples,
            [1.0, 2.0, 3.0, 4.0, 3.0, 6.0, 9.0, 12.0],
            "{phase}"
        );
    }
}
