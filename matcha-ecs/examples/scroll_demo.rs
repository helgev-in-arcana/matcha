//! Scrolling.
//!
//! What to check by hand (none of it is reachable from a headless test — the
//! headless side is `matcha-ecs/tests/scroll.rs`, which asserts offsets,
//! transforms and chaining, not pixels):
//!
//! - **The wheel scrolls, and stops at the ends.** The first column's list
//!   scrolls under the pointer and clamps at top and bottom rather than running
//!   past them.
//! - **The bar reads as a position indicator.** The thumb's length is
//!   proportional to how much of the list is visible, it starts flush with the
//!   top of the track and ends flush with the bottom, and its corners are
//!   rounded and cleanly antialiased.
//! - **The bar is drawn over the content**, not under it — scroll until a row
//!   passes behind the thumb.
//! - **Dragging the thumb works**, and the thumb stays under the cursor even
//!   when the cursor drifts a little off it sideways. **Clicking the track**
//!   above or below the thumb pages towards the click.
//! - **Nested views chain.** In the second column, scrolling the inner box
//!   moves it until it reaches its end, and only then does the outer box start
//!   to move — CSS scroll chaining.
//! - **Horizontal scrolling.** The third column scrolls sideways, and both bars
//!   appear at once without overlapping in the corner. Note the text there does
//!   *not* wrap: a scrollable axis is measured unbounded, so `overflow_x` reads
//!   as `white-space: nowrap`.
//! - **`Auto` vs `Hidden`.** The fourth column shows the same oversized content
//!   twice. Both clip; only the `Auto` one has a bar and responds to the wheel.
//! - **A text box inside a scroll view still works** — focus, caret, selection
//!   and the IME candidate window, including after the box has been scrolled
//!   away from where it was first laid out.

use matcha_ecs::{ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::{
    scroll_view::{scroll_view, Overflow, ScrollView},
    text_box, AlignItems, ColorRect, Column, Panel, RichText, Row, TextBox,
};
use matcha_window::adapter::Adapter;

struct Model {
    note: String,
}

#[derive(Clone, PartialEq, Debug)]
enum Msg {
    NoteEdited(String),
}

fn label(s: &mut Scope, text: impl Into<String>) {
    s.leaf(
        RichText::new(text)
            .font_size(13.0)
            .color([0.62, 0.62, 0.68, 1.0]),
    );
}

fn heading(s: &mut Scope, text: impl Into<String>) {
    s.leaf(
        RichText::new(text)
            .font_size(13.0)
            .color([0.85, 0.85, 0.9, 1.0]),
    );
}

/// A scroll view in a bordered panel, so its edges are visible.
fn framed(s: &mut Scope, w: f32, h: f32, content: impl FnOnce(&mut Scope)) {
    s.node(
        Panel::new(w + 4.0, h + 4.0)
            .border_width(2.0)
            .border_color([0.4, 0.4, 0.5, 1.0])
            .background_color([0.14, 0.14, 0.17, 1.0]),
        content,
    );
}

/// A stack of numbered rows, tall enough to need scrolling.
fn rows(s: &mut Scope, count: usize, width: f32) {
    s.node(Column::new().gap(4.0).align_items(AlignItems::Start), |s| {
        for i in 0..count {
            // Alternating shades make the motion easy to read.
            let shade = if i % 2 == 0 { 0.30 } else { 0.24 };
            s.node(
                Row::new().gap(8.0).align_items(AlignItems::Center),
                move |s| {
                    s.leaf(ColorRect::new(width, 28.0).color([shade, shade * 1.3, 0.5, 1.0]));
                },
            );
            label(s, format!("row {i}"));
        }
    });
}

fn view(model: &Model, s: &mut Scope) {
    s.node(Column::new().gap(16.0).align_items(AlignItems::Start), |s| {
        s.node(Row::new().gap(20.0).align_items(AlignItems::Start), |s| {
            // 1. The basics: wheel, thumb drag, track paging, clamping.
            s.node(Column::new().gap(6.0).align_items(AlignItems::Start), |s| {
                heading(s, "vertical — wheel, drag, page");
                framed(s, 200.0, 220.0, |s| {
                    scroll_view(s, ScrollView::new(200.0, 220.0), |s| {
                        rows(s, 20, 150.0);
                    });
                });
            });

            // 2. Chaining: the inner view hands over once it is pinned.
            s.node(Column::new().gap(6.0).align_items(AlignItems::Start), |s| {
                heading(s, "nested — inner first, then outer");
                framed(s, 200.0, 220.0, |s| {
                    scroll_view(s, ScrollView::new(200.0, 220.0), |s| {
                        s.node(Column::new().gap(8.0).align_items(AlignItems::Start), |s| {
                            label(s, "above the inner box");
                            s.node(
                                Panel::new(184.0, 124.0)
                                    .border_width(2.0)
                                    .border_color([0.9, 0.75, 0.3, 1.0]),
                                |s| {
                                    scroll_view(s, ScrollView::new(180.0, 120.0), |s| {
                                        rows(s, 8, 120.0);
                                    });
                                },
                            );
                            rows(s, 6, 150.0);
                        });
                    });
                });
            });

            // 3. Both axes at once. Also shows that a scrollable axis does not
            //    wrap text.
            s.node(Column::new().gap(6.0).align_items(AlignItems::Start), |s| {
                heading(s, "both axes — no wrap, no corner overlap");
                framed(s, 220.0, 220.0, |s| {
                    scroll_view(
                        s,
                        ScrollView::new(220.0, 220.0).overflow_x(Overflow::Auto),
                        |s| {
                            s.node(Column::new().gap(6.0).align_items(AlignItems::Start), |s| {
                                s.leaf(
                                    RichText::new(
                                        "a single unwrapped line that runs well past the right edge \
                                         — 折り返さない一行",
                                    )
                                    .font_size(15.0)
                                    .color([0.9, 0.9, 0.95, 1.0]),
                                );
                                for i in 0..10 {
                                    s.leaf(
                                        ColorRect::new(420.0, 24.0)
                                            .color([0.2 + (i as f32) * 0.05, 0.35, 0.55, 1.0]),
                                    );
                                }
                            });
                        },
                    );
                });
            });
        });

        s.node(Row::new().gap(20.0).align_items(AlignItems::Start), |s| {
            // 4. Auto vs Hidden: both clip, only one scrolls.
            s.node(Column::new().gap(6.0).align_items(AlignItems::Start), |s| {
                heading(s, "Auto — clips, has a bar, scrolls");
                framed(s, 180.0, 120.0, |s| {
                    scroll_view(s, ScrollView::new(180.0, 120.0), |s| {
                        rows(s, 10, 130.0);
                    });
                });
            });
            s.node(Column::new().gap(6.0).align_items(AlignItems::Start), |s| {
                heading(s, "Hidden — clips, no bar, no wheel");
                framed(s, 180.0, 120.0, |s| {
                    scroll_view(
                        s,
                        ScrollView::new(180.0, 120.0).overflow_y(Overflow::Hidden),
                        |s| {
                            rows(s, 10, 130.0);
                        },
                    );
                });
            });

            // 5. A text box that has been scrolled away from where it started.
            s.node(Column::new().gap(6.0).align_items(AlignItems::Start), |s| {
                heading(s, "a text box inside a scroll view");
                framed(s, 260.0, 120.0, |s| {
                    scroll_view(s, ScrollView::new(260.0, 120.0), |s| {
                        s.node(Column::new().gap(8.0).align_items(AlignItems::Start), |s| {
                            label(s, "scroll down to reach the box");
                            s.leaf(ColorRect::new(200.0, 40.0).color([0.25, 0.25, 0.3, 1.0]));
                            s.leaf(
                                TextBox::new(230.0, 70.0)
                                    .value(model.note.clone())
                                    .on_update(|text| Msg::NoteEdited(text.to_string())),
                            );
                            label(s, "typed value:");
                            s.leaf(
                                RichText::new(model.note.clone())
                                    .font_size(14.0)
                                    .color([0.9, 0.9, 0.95, 1.0]),
                            );
                        });
                    });
                });
            });
        });
    });
}

fn reduce(model: &mut Model, msg: Msg) {
    match msg {
        Msg::NoteEdited(text) => model.note = text,
    }
}

fn main() {
    env_logger::init();

    let model = Model {
        note: "ここに入力 / type here".to_string(),
    };

    let app = UiEcs::new(model, view, reduce).with_pre_layout_systems(text_box::default_systems());

    Adapter::new(app).run().expect("event loop failed");
}
