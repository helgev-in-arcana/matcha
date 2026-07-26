//! Editable text with IME.
//!
//! What to check by hand (none of it is reachable from a headless test):
//!
//! - **Japanese input.** Click a box, switch to a Japanese IME, and type. The
//!   unconfirmed reading should appear inline as you type, and the conversion
//!   candidate list should pop up **next to the caret**, not in a screen corner
//!   — that is `set_ime_cursor_area` working. Press Enter to commit.
//! - **Focus.** Clicking a box gives it a highlighted border and a blinking
//!   caret; clicking the background clears both. Only one box is ever focused.
//! - **Editing.** Arrows, Home/End, Ctrl+arrows by word, Shift to select,
//!   Ctrl+A, Backspace/Delete (Ctrl for whole words). Enter inserts a newline;
//!   **Ctrl+Enter confirms**.
//! - **Live sync.** The first box declares `on_update`, so the echo line under
//!   it follows every keystroke through the model. The second declares only
//!   `on_confirm`, so its echo updates on Ctrl+Enter or when focus leaves.
//! - **App-driven overwrite.** "Reset first box" writes a new value into the
//!   model. The box takes it because the *declared* value changed; typing in it
//!   afterwards is not clobbered by the same value being re-declared each frame.

use matcha_ecs::{ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::{text_box, AlignItems, Button, Column, Panel, Row, Text, TextBox};
use matcha_window::adapter::Adapter;

struct Model {
    note: String,
    committed_note: String,
    title: String,
    resets: u32,
}

#[derive(Clone, PartialEq, Debug)]
enum Msg {
    /// Carries the text — the reason `Message` is `Clone` rather than `Copy`.
    NoteEdited(String),
    NoteConfirmed(String),
    TitleConfirmed(String),
    ResetNote,
}

fn label(s: &mut Scope, text: &str) {
    s.leaf(Text::new(text).font_size(13.0).color([0.6, 0.6, 0.65, 1.0]));
}

fn view(model: &Model, s: &mut Scope) {
    s.node(Column::new().gap(18.0), |s| {
        label(
            s,
            "Text box — click to focus, type (IME welcome). Enter = newline, Ctrl+Enter = confirm.",
        );

        // --- Box 1: syncs to the model on every keystroke ---
        s.node(Column::new().gap(6.0), |s| {
            label(s, "on_update — echoes every keystroke through the model");
            s.node(Panel::new(360.0, 110.0).border_width(1.0), |s| {
                s.leaf(
                    TextBox::new(340.0, 90.0)
                        .value(&model.note)
                        .on_update(|text| Msg::NoteEdited(text.to_string()))
                        .on_confirm(|text| Msg::NoteConfirmed(text.to_string())),
                );
            });
            s.leaf(
                Text::new(format!("model.note = {:?}", model.note))
                    .font_size(13.0)
                    .color([0.55, 0.8, 0.6, 1.0]),
            );
            s.leaf(
                Text::new(format!("last confirmed = {:?}", model.committed_note))
                    .font_size(13.0)
                    .color([0.5, 0.6, 0.8, 1.0]),
            );
        });

        // --- Box 2: only reports on confirm ---
        s.node(Column::new().gap(6.0), |s| {
            label(s, "on_confirm only — updates on Ctrl+Enter or focus loss");
            s.leaf(
                TextBox::new(340.0, 44.0)
                    .font_size(18.0)
                    .on_confirm(|text| Msg::TitleConfirmed(text.to_string())),
            );
            s.leaf(
                Text::new(format!("model.title = {:?}", model.title))
                    .font_size(13.0)
                    .color([0.55, 0.8, 0.6, 1.0]),
            );
        });

        // --- App-driven value change ---
        s.node(Row::new().gap(12.0).align_items(AlignItems::Center), |s| {
            s.leaf(
                Button::new("Reset first box")
                    .on(Msg::ResetNote)
                    .size(160.0, 36.0),
            );
            label(
                s,
                "writes a new value into the model; the box adopts it because the declared value changed",
            );
        });
    });
}

fn reduce(model: &mut Model, msg: Msg) {
    match msg {
        Msg::NoteEdited(text) => model.note = text,
        Msg::NoteConfirmed(text) => model.committed_note = text,
        Msg::TitleConfirmed(text) => model.title = text,
        Msg::ResetNote => {
            model.resets += 1;
            model.note = format!("reset #{} — 日本語もどうぞ", model.resets);
        }
    }
}

fn main() {
    env_logger::init();

    let model = Model {
        note: "ここに入力してください / type here".to_string(),
        committed_note: String::new(),
        title: String::new(),
        resets: 0,
    };

    // Without `text_box::default_systems()` the text never re-lays-out and the
    // caret never blinks — the same registration footgun `animation` has.
    let app = UiEcs::new(model, view, reduce).with_pre_layout_systems(text_box::default_systems());

    Adapter::new(app).run().expect("event loop failed");
}
