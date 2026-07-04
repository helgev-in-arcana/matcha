//! M8 demo: click "+ card" to add a card that starts a simulated ~1.2s fetch
//! (a widget-internal `after_spawn` hook calling `spawn_task`, not the
//! reducer — the reducer only ever touches the Model, never entities).
//! Click "remove last" to despawn the most recently added card; if its fetch
//! hasn't finished yet, this demonstrates cancellation-on-despawn (though see
//! `matcha_ecs::task`'s module docs for the caveat on how promptly a
//! cancelled task's resources are actually reclaimed — it isn't tested for
//! timing here, only that adding/removing cards works and completed fetches
//! turn the card green).

use std::time::Duration;

use bevy_ecs::{bundle::Bundle, world::EntityWorldMut};
use matcha_ecs::{
    components::view::Key,
    layout::LayoutDispatch,
    model::ModelHandle,
    task::spawn_task,
    ui_ecs::UiEcs,
    view::{Scope, Widget},
};
use matcha_ecs_widgets::{color_rect::RectGeometry, Button, ColorRect, Column, Row};
use matcha_window::adapter::Adapter;

struct Card {
    id: u64,
    loaded: bool,
}

struct Model {
    cards: Vec<Card>,
    next_id: u64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Msg {
    Add,
    RemoveLast,
}

fn reduce(model: &mut Model, msg: Msg) {
    match msg {
        Msg::Add => {
            let id = model.next_id;
            model.next_id += 1;
            model.cards.push(Card { id, loaded: false });
        }
        Msg::RemoveLast => {
            model.cards.pop();
        }
    }
}

/// Invisible (zero-size) leaf: its only job is to `spawn_task` a simulated
/// fetch for `id` when its entity is first created. Reuses `ColorRect`'s
/// `RectGeometry`/`LayoutDispatch` so it participates in layout without
/// needing its own rendering machinery (it carries no `RenderItem`, so
/// `extract_items` skips it — nothing is drawn for it).
struct FetchTrigger {
    id: u64,
}

impl Widget for FetchTrigger {
    fn key(&self) -> Key {
        Key::Auto
    }

    fn bundle(&self) -> impl Bundle {
        (
            RectGeometry { w: 0.0, h: 0.0 },
            LayoutDispatch::of::<RectGeometry>(),
        )
    }

    fn patch(&self, _entity: &mut EntityWorldMut) {}

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        let handle = entity.resource::<ModelHandle<Model>>().clone();
        let id = self.id;
        spawn_task(entity, id, async move {
            // Stand-in for a real async fetch (image decode, network, ...).
            // Blocking the AsyncComputeTaskPool worker thread for a fixed
            // delay is fine here — that pool exists precisely for
            // longer-running background work, unlike the main UI thread.
            std::thread::sleep(Duration::from_millis(1200));
            handle.update(move |model: &mut Model| {
                if let Some(card) = model.cards.iter_mut().find(|c| c.id == id) {
                    card.loaded = true;
                }
            });
        });
    }
}

fn view(model: &Model, s: &mut Scope) {
    s.node(Column::new().gap(10.0), |s| {
        s.node(Row::new().gap(10.0), |s| {
            s.leaf(Button::new("+ card").on(Msg::Add).color([0.3, 0.8, 0.4, 1.0]));
            s.leaf(
                Button::new("remove last")
                    .on(Msg::RemoveLast)
                    .color([0.8, 0.3, 0.3, 1.0]),
            );
        });
        for card in &model.cards {
            s.node(Row::new().key(card.id), |s| {
                s.leaf(FetchTrigger { id: card.id });
                let color = if card.loaded {
                    [0.2, 0.7, 0.3, 1.0]
                } else {
                    [0.5, 0.5, 0.5, 1.0]
                };
                s.leaf(ColorRect::new(220.0, 50.0).color(color));
            });
        }
    });
}

fn main() {
    env_logger::init();

    let model = Model {
        cards: Vec::new(),
        next_id: 0,
    };
    Adapter::new(UiEcs::new(model, view, reduce))
        .run()
        .expect("event loop failed");
}
