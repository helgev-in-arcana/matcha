//! M5 demo: click "+"/"-" to grow/shrink a rectangle. No text yet (M6) — the
//! button labels aren't drawn — but the rect width visibly tracking clicks
//! proves click -> hit-test -> reducer -> Phase B works end to end.

use matcha_ecs::{ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::{Button, Column, ColorRect, Row};
use matcha_window::adapter::Adapter;

struct Model {
    count: i32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Msg {
    Inc,
    Dec,
}

fn view(model: &Model, s: &mut Scope) {
    s.node(Column::new().gap(20.0), |s| {
        let width = 50.0 + (model.count.max(0) as f32) * 20.0;
        s.leaf(ColorRect::new(width, 80.0).color([0.2, 0.4, 0.8, 1.0]));
        s.node(Row::new().gap(10.0), |s| {
            s.leaf(Button::new("-").on(Msg::Dec).color([0.8, 0.3, 0.3, 1.0]));
            s.leaf(Button::new("+").on(Msg::Inc).color([0.3, 0.8, 0.4, 1.0]));
        });
    });
}

fn reduce(model: &mut Model, msg: Msg) {
    match msg {
        Msg::Inc => model.count += 1,
        Msg::Dec => model.count -= 1,
    }
}

fn main() {
    env_logger::init();
    Adapter::new(UiEcs::new(Model { count: 0 }, view, reduce))
        .run()
        .expect("event loop failed");
}
