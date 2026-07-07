//! Visual check for `RichText` (parley-backed): mixed Japanese/English/emoji
//! text (font fallback + real shaping) and a counter label to confirm
//! content updates reshape correctly. Resize the window to see word-wrap
//! reflow (there's no fixed-width container widget yet, so `RichText` here
//! wraps at the window's width, same as `counter_click.rs`'s `Text`).
//! Compare against `counter_click.rs`'s suzuri-backed `Text` for the
//! difference fallback/shaping makes on mixed-script text.

use matcha_ecs::{ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::{Button, Column, Row, RichText};
use matcha_window::adapter::Adapter;

struct Model {
    count: i32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Msg {
    Inc,
    Dec,
}

const SAMPLE: &str = "Hello, 世界! parley renders 日本語 and English in one line, with emoji fallback: 😀🎉 and ligatures: fi fl.";

fn view(model: &Model, s: &mut Scope) {
    s.node(Column::new().gap(20.0), |s| {
        s.leaf(RichText::new(format!("count: {}", model.count)).font_size(24.0));
        s.leaf(
            RichText::new(SAMPLE)
                .font_size(36.0)
                .color([0.0, 0.0, 0.0, 1.0]),
        );
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
