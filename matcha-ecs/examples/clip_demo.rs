//! Clipping (`overflow: hidden`).
//!
//! What to check by hand (none of it is reachable from a headless test — the
//! headless side is `matcha-ecs/tests/clip.rs`, which asserts the shape of the
//! clip arena, not the pixels):
//!
//! - **Basic clip.** The left panel holds a rectangle far larger than itself.
//!   Only the part inside the panel is drawn, and the cut is a clean straight
//!   edge flush with the border — no fringe, no half-transparent row.
//! - **Nesting.** The middle panel holds a smaller clipped panel, which in turn
//!   holds an oversized rectangle. The rectangle is confined to the *inner*
//!   panel, and the inner panel is itself confined by the outer one, so the
//!   visible region is the intersection of the two.
//! - **Text across a boundary.** The right panel holds a line of text wider and
//!   taller than it is. Glyphs are cut mid-stroke at the edge rather than
//!   disappearing whole — clipping is per pixel, not per glyph.
//! - **Clipping affects clicking.** The button in the left panel sticks out past
//!   the panel's right edge. Clicking the visible part increments the counter;
//!   clicking where the button *would* be, outside the panel, does nothing.
//! - **A clipped text box still works.** The bottom box sits inside a clip that
//!   cuts its right-hand side. Focus, the caret, selection and the IME candidate
//!   window must all still behave — in particular the candidate window should
//!   follow the caret, since `ImeCursorArea` is reported in window space and is
//!   unaffected by clipping.

use matcha_ecs::{ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::{
    text_box, AlignItems, Button, ColorRect, Column, Panel, RichText, Row, TextBox,
};
use matcha_window::adapter::Adapter;

struct Model {
    clicks: u32,
    note: String,
}

#[derive(Clone, PartialEq, Debug)]
enum Msg {
    Clicked,
    NoteEdited(String),
}

fn label(s: &mut Scope, text: impl Into<String>) {
    s.leaf(
        RichText::new(text)
            .font_size(13.0)
            .color([0.6, 0.6, 0.65, 1.0]),
    );
}

/// A labelled panel, clipped or not, so the two can be compared side by side.
fn demo_panel(s: &mut Scope, caption: &str, clip: bool, content: impl Fn(&mut Scope)) {
    s.node(Column::new().gap(6.0), |s| {
        label(s, caption);
        s.node(
            Panel::new(160.0, 120.0)
                .clip(clip)
                .border_width(2.0)
                .border_color([0.45, 0.45, 0.55, 1.0])
                .background_color([0.16, 0.16, 0.19, 1.0]),
            |s| content(s),
        );
    });
}

fn view(model: &Model, s: &mut Scope) {
    s.node(Column::new().gap(20.0).align_items(AlignItems::Start), |s| {
        s.node(Row::new().gap(20.0).align_items(AlignItems::Start), |s| {
            // Oversized child, cut to the panel.
            demo_panel(s, "clipped", true, |s| {
                s.leaf(ColorRect::new(400.0, 400.0).color([0.85, 0.45, 0.2, 1.0]));
            });

            // The same composition without the clip, for comparison: this one
            // spills over everything to its right.
            demo_panel(s, "not clipped", false, |s| {
                s.leaf(ColorRect::new(60.0, 60.0).color([0.3, 0.55, 0.85, 1.0]));
            });

            // Nested: the inner panel is clipped by the outer one, and the
            // rectangle by both.
            demo_panel(s, "nested clips", true, |s| {
                s.node(
                    Panel::new(90.0, 200.0)
                        .clip(true)
                        .border_width(2.0)
                        .border_color([0.9, 0.8, 0.3, 1.0]),
                    |s| {
                        s.leaf(ColorRect::new(400.0, 400.0).color([0.3, 0.7, 0.4, 1.0]));
                    },
                );
            });

            // Glyphs must be cut mid-stroke, not dropped whole.
            demo_panel(s, "text across the edge", true, |s| {
                s.leaf(
                    RichText::new("clipping cuts glyphs mid-stroke — 日本語も同じく")
                        .font_size(20.0)
                        .color([0.9, 0.9, 0.95, 1.0]),
                );
            });
        });

        s.node(Row::new().gap(20.0).align_items(AlignItems::Start), |s| {
            // The button overhangs the panel: only the visible part is clickable.
            s.node(Column::new().gap(6.0), |s| {
                label(s, "clipped button — only the visible part is clickable");
                s.node(
                    Panel::new(120.0, 60.0)
                        .clip(true)
                        .border_width(2.0)
                        .border_color([0.45, 0.45, 0.55, 1.0]),
                    |s| {
                        s.leaf(
                            Button::new("click me")
                                .size(260.0, 40.0)
                                .color([0.25, 0.45, 0.3, 1.0])
                                .on(Msg::Clicked),
                        );
                    },
                );
            });

            s.node(Column::new().gap(6.0), |s| {
                label(s, &format!("clicks: {}", model.clicks));
                label(s, "(clicking right of the panel must do nothing)");
            });
        });

        s.node(Column::new().gap(6.0), |s| {
            label(s, "a text box inside a clip — caret, selection and IME still work");
            s.node(
                Panel::new(240.0, 90.0)
                    .clip(true)
                    .border_width(2.0)
                    .border_color([0.45, 0.45, 0.55, 1.0]),
                |s| {
                    s.leaf(
                        // Wider than the panel, so its right-hand side is cut.
                        TextBox::new(360.0, 80.0)
                            .value(model.note.clone())
                            .on_update(|text| Msg::NoteEdited(text.to_string())),
                    );
                },
            );
        });
    });
}

fn reduce(model: &mut Model, msg: Msg) {
    match msg {
        Msg::Clicked => model.clicks += 1,
        Msg::NoteEdited(text) => model.note = text,
    }
}

fn main() {
    env_logger::init();

    let model = Model {
        clicks: 0,
        note: "ここに入力 / type here".to_string(),
    };

    let app = UiEcs::new(model, view, reduce).with_pre_layout_systems(text_box::default_systems());

    Adapter::new(app).run().expect("event loop failed");
}
