//! M3 demo: `Column > (ColorRect, Row > (ColorRect, ColorRect))` — real
//! layout (gap-stacking, nesting) instead of self-declared absolute
//! positions. No meaningful model or input.

use matcha_ecs::{ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::{Column, ColorRect, Row};
use matcha_window::adapter::Adapter;

fn view(_model: &(), s: &mut Scope) {
    s.node(Column::new().gap(20.0), |s| {
        s.leaf(ColorRect::new(300.0, 100.0).color([0.2, 0.4, 0.8, 1.0]));
        s.node(Row::new().gap(20.0), |s| {
            s.leaf(ColorRect::new(100.0, 100.0).color([0.9, 0.3, 0.3, 1.0]));
            s.leaf(ColorRect::new(100.0, 100.0).color([0.3, 0.8, 0.4, 1.0]));
        });
    });
}

fn main() {
    env_logger::init();
    // No clickable widgets in this demo, so `Msg = ()` and the reducer is a no-op.
    Adapter::new(UiEcs::new((), view, |_model: &mut (), _msg: ()| {}))
        .run()
        .expect("event loop failed");
}
