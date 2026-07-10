//! M7 demo: toasts fade in when added and fade out when removed, with a
//! revival reversal if one gets re-added while still fading out. No text yet
//! (M6) -- each toast is a plain coloured rectangle; a background thread adds
//! one every ~2s and expires it ~2.5s after it appears.

use std::time::{Duration, Instant};

use matcha_ecs::{animation::Easing, ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::{Column, ColorRect};
use matcha_window::adapter::Adapter;

const TOAST_LIFETIME: Duration = Duration::from_millis(2500);
const FADE_DURATION: Duration = Duration::from_millis(300);
const SPAWN_INTERVAL: Duration = Duration::from_millis(2000);

struct Toast {
    id: u64,
    spawned_at: Instant,
}

struct Model {
    toasts: Vec<Toast>,
    next_id: u64,
    last_spawn: Instant,
}

fn view(model: &Model, s: &mut Scope) {
    s.node(Column::new().gap(10.0), |s| {
        for toast in &model.toasts {
            s.leaf(
                ColorRect::new(220.0, 50.0)
                    .key(toast.id)
                    .color([0.2, 0.5, 0.9, 1.0])
                    .enter_fade(FADE_DURATION, Easing::EaseInOut)
                    .exit_fade(FADE_DURATION, Easing::EaseInOut),
            );
        }
    });
}

fn main() {
    env_logger::init();

    let now = Instant::now();
    let model = Model {
        toasts: Vec::new(),
        next_id: 0,
        // Far enough in the past that the first tick spawns immediately.
        last_spawn: now - SPAWN_INTERVAL,
    };
    let app = UiEcs::new(model, view, |_model: &mut Model, _msg: ()| {});
    let handle = app.model_handle();

    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(100));
        handle.update(|model: &mut Model| {
            let now = Instant::now();
            if now.duration_since(model.last_spawn) >= SPAWN_INTERVAL {
                model.last_spawn = now;
                let id = model.next_id;
                model.next_id += 1;
                model.toasts.push(Toast {
                    id,
                    spawned_at: now,
                });
            }
            model
                .toasts
                .retain(|t| now.duration_since(t.spawned_at) < TOAST_LIFETIME);
        });
    });

    Adapter::new(app).run().expect("event loop failed");
}
