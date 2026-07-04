//! M2 demo: a background thread increments the model once a second and the
//! view function grows a rectangle in response. No layout (M3) and no text
//! label (M6) yet — just a widening rect proves the model loop is wired end
//! to end (`ModelHandle::update` -> drain -> re-`run_view` -> redraw).

use std::time::Duration;

use matcha_ecs::{ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::ColorRect;
use matcha_window::adapter::Adapter;

struct Model {
    count: u32,
}

fn view(model: &Model, s: &mut Scope) {
    let width = 50.0 + (model.count as f32) * 20.0;
    s.leaf(ColorRect::new(width, 80.0).color([0.2, 0.4, 0.8, 1.0]));
}

fn main() {
    env_logger::init();
    // No clickable widgets in this demo, so `Msg = ()` and the reducer is a no-op.
    let app = UiEcs::new(Model { count: 0 }, view, |_model: &mut Model, _msg: ()| {});
    let handle = app.model_handle();

    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        handle.update(|model| model.count += 1);
    });

    Adapter::new(app).run().expect("event loop failed");
}
