//! Minimal M1 demo: two absolutely-positioned solid rectangles rendered from a
//! view function, with no model, layout, or input.

use matcha_ecs::{ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::ColorRect;
use matcha_window::adapter::Adapter;

fn view(s: &mut Scope) {
    s.leaf(
        ColorRect::new(300.0, 200.0)
            .color([0.2, 0.4, 0.8, 1.0])
            .pos(250.0, 200.0),
    );
    s.leaf(
        ColorRect::new(100.0, 100.0)
            .color([0.9, 0.3, 0.3, 1.0])
            .pos(50.0, 50.0),
    );
}

fn main() {
    Adapter::new(UiEcs::new(view))
        .run()
        .expect("event loop failed");
}
