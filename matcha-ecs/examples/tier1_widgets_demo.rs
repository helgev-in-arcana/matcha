//! Tier-1 HTML/CSS-inspired widgets demo: exercises every widget/feature
//! added in this batch — `Column`/`Row` with `align_items` (all four
//! variants, side by side) and `justify_content` (exercised but currently
//! inert, see below), a `Checkbox` in both default and fully-customised
//! form, a `Button` with a real shaped label (default and customised
//! font_size/label_color), a `Link` (default and customised), an `Image`
//! (`object-fit: contain` shown against two differently-shaped boxes, same
//! source), and a `Panel` (background-only and bordered).
//!
//! `justify_content` (exercised on the Button row below via
//! `.justify_content(JustifyContent::SpaceBetween)`) currently has **no
//! visible effect** anywhere in this demo, or anywhere in this codebase's
//! current widget set — see `matcha-ecs-widgets`'s `JustifyContent` doc
//! comment: every layout here hands a `Column`/`Row` exactly its own
//! measured (fit-content) size, so there is never leftover main-axis space
//! to distribute. It's exercised anyway to confirm the API runs without
//! panicking; do not expect the Button row to visibly redistribute.
//! `align_items` does not have this limitation — the dedicated comparison
//! row near the bottom shows all four variants with a visible difference.

use std::sync::{Arc, LazyLock};

use matcha_ecs::{ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::{
    AlignItems, Button, Checkbox, Column, ColorRect, Image, JustifyContent, Link, Panel, Row, Text,
};
use matcha_window::adapter::Adapter;

const IMAGE_BYTES: &[u8] = include_bytes!("../../matcha/src/assets/videoframe_21710.png");

/// Held once and `.clone()`d (a cheap refcount bump) into every `view()` call
/// — `Image::from_bytes` keys its decode cache on this `Arc`'s pointer
/// identity, so re-deriving a fresh `Arc` from `IMAGE_BYTES` at each call
/// site (e.g. `Arc::from(IMAGE_BYTES)` inline in `view()`) would defeat the
/// cache and force a full re-decode of this multi-megabyte PNG on every
/// click. See `Image::from_bytes`'s doc comment.
static IMAGE_ARC: LazyLock<Arc<[u8]>> = LazyLock::new(|| Arc::from(IMAGE_BYTES));

struct Model {
    count: i32,
    checked_a: bool,
    checked_b: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Msg {
    Inc,
    Dec,
    ToggleA,
    ToggleB,
    OpenDocs,
}

fn section_label(s: &mut Scope, text: &str) {
    s.leaf(Text::new(text).font_size(13.0).color([0.6, 0.6, 0.65, 1.0]));
}

/// One `AlignItems` variant, shown as a label over a two-child `Row` (a tall
/// blue reference rect + a shorter orange rect whose position/size visibly
/// changes with `align`).
fn align_demo(s: &mut Scope, label: &str, align: AlignItems) {
    s.node(Column::new().gap(4.0), |s| {
        section_label(s, label);
        s.node(Row::new().gap(6.0).align_items(align), |s| {
            s.leaf(ColorRect::new(36.0, 70.0).color([0.3, 0.5, 0.9, 1.0]));
            s.leaf(ColorRect::new(36.0, 26.0).color([0.9, 0.6, 0.2, 1.0]));
        });
    });
}

fn view(model: &Model, s: &mut Scope) {
    s.node(Column::new().gap(20.0), |s| {
        // --- Checkbox: default + fully-customised (size/border_width/colors) ---
        section_label(s, "Checkbox — default, and custom size/border_width/colors");
        s.node(Row::new().gap(16.0).align_items(AlignItems::Center), |s| {
            s.leaf(Checkbox::new(model.checked_a).on(Msg::ToggleA));
            s.leaf(Text::new(if model.checked_a { "checked" } else { "unchecked" }));
            s.leaf(
                Checkbox::new(model.checked_b)
                    .on(Msg::ToggleB)
                    .size(28.0)
                    .border_width(3.0)
                    .border_color([0.9, 0.3, 0.5, 1.0])
                    .fill_color([0.95, 0.75, 0.1, 1.0]),
            );
            s.leaf(Text::new(if model.checked_b { "checked" } else { "unchecked" }));
        });

        // --- Button: real shaped labels, custom font_size/label_color, RectColor patch fix ---
        section_label(s, "Button — real label; justify_content(SpaceBetween) exercised but inert (see doc comment)");
        s.node(Row::new().gap(10.0).justify_content(JustifyContent::SpaceBetween), |s| {
            s.leaf(Button::new("-").on(Msg::Dec).color([0.8, 0.3, 0.3, 1.0]));
            s.leaf(
                Button::<Msg>::new(format!("count: {}", model.count))
                    .color([0.3, 0.3, 0.4, 1.0])
                    .size(170.0, 44.0)
                    .font_size(20.0)
                    .label_color([1.0, 0.9, 0.4, 1.0]),
            );
            s.leaf(Button::new("+").on(Msg::Inc).color([0.3, 0.8, 0.4, 1.0]));
        });

        // --- Link: default underline+accent, and fully customised ---
        section_label(s, "Link — default underline+accent, and custom color/font_size/no-underline");
        s.node(Row::new().gap(24.0).align_items(AlignItems::Center), |s| {
            s.leaf(Link::new("Open documentation").on(Msg::OpenDocs));
            s.leaf(
                Link::new("Custom styled link")
                    .on(Msg::OpenDocs)
                    .color([0.9, 0.4, 0.8, 1.0])
                    .font_size(20.0)
                    .underline(false),
            );
        });

        // --- Panel: background-only (no border) vs bordered ---
        section_label(s, "Panel — background-only (border_width 0) vs bordered");
        s.node(Row::new().gap(16.0), |s| {
            s.node(Panel::new(150.0, 60.0).background_color([0.18, 0.22, 0.3, 1.0]), |s| {
                s.leaf(Text::new("no border").font_size(13.0));
            });
            s.node(
                Panel::new(150.0, 60.0)
                    .border_width(3.0)
                    .border_color([0.9, 0.6, 0.2, 1.0])
                    .background_color([0.18, 0.18, 0.2, 1.0]),
                |s| {
                    s.leaf(Text::new("bordered").font_size(13.0));
                },
            );
        });

        // --- Image: object-fit: contain against a wide box and a tall box, same source ---
        section_label(s, "Image — object-fit: contain, same source in a wide box and a tall box");
        s.node(Row::new().gap(16.0), |s| {
            s.node(
                Panel::new(220.0, 100.0).border_width(2.0).border_color([0.5, 0.5, 0.55, 1.0]).background_color([0.12, 0.12, 0.14, 1.0]),
                |s| {
                    s.leaf(Image::from_bytes(IMAGE_ARC.clone(), 200.0, 80.0));
                },
            );
            s.node(
                Panel::new(110.0, 180.0).border_width(2.0).border_color([0.5, 0.5, 0.55, 1.0]).background_color([0.12, 0.12, 0.14, 1.0]),
                |s| {
                    s.leaf(Image::from_bytes(IMAGE_ARC.clone(), 90.0, 160.0));
                },
            );
        });

        // --- AlignItems: all four variants side by side, visibly different ---
        section_label(s, "Row align_items — all four variants (default is Stretch)");
        s.node(Row::new().gap(24.0), |s| {
            align_demo(s, "Stretch", AlignItems::Stretch);
            align_demo(s, "Start", AlignItems::Start);
            align_demo(s, "Center", AlignItems::Center);
            align_demo(s, "End", AlignItems::End);
        });
    });
}

fn reduce(model: &mut Model, msg: Msg) {
    match msg {
        Msg::Inc => model.count += 1,
        Msg::Dec => model.count -= 1,
        Msg::ToggleA => model.checked_a = !model.checked_a,
        Msg::ToggleB => model.checked_b = !model.checked_b,
        Msg::OpenDocs => log::info!("Link clicked (demo has no real navigation)"),
    }
}

fn main() {
    env_logger::init();
    Adapter::new(UiEcs::new(
        Model { count: 0, checked_a: false, checked_b: true },
        view,
        reduce,
    ))
    .run()
    .expect("event loop failed");
}
