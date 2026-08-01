//! **The** demo. One window exercising every feature `matcha-ecs` and
//! `matcha-ecs-widgets` have, so a change can be eyeballed in one place instead
//! of across ten programs.
//!
//! ```text
//! cargo run --example showcase
//! ```
//!
//! # What to try, and what it proves
//!
//! | Do this | It shows |
//! |---|---|
//! | Hover the buttons | `:hover`, and a colour `transition` |
//! | Hold one down, drag off, drag back | `:active` is the press chain ∩ the hover chain |
//! | Click anything, then press `Tab` / `Shift+Tab` | Focus, focus rings, and document-order tab stops |
//! | Move the pointer between sections | Cursor shapes, resolved leaf-to-root |
//! | Drag the sliders — off the track, even out of the window | Pointer capture |
//! | Type in the text boxes, including via an IME | Keyboard + IME delivery, `Ctrl+C/X/V` |
//! | Scroll the page, and the inner box, to its end | Scrolling, and CSS scroll chaining |
//! | Toggle "overlay" | `Anchor` + `z-index`: painting and picking move together |
//! | Toggle "show the animated row" | `display: none`, and enter/exit fades |
//! | Resize the window | Reflow, `Length::Fill`, wrapping, and text re-wrap |
//!
//! # The one registration that matters
//!
//! `matcha_ecs_widgets::default_systems()` at the bottom. Without it exit fades
//! never despawn, text boxes never re-lay-out, carets never blink, and colour
//! transitions never advance.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use matcha_ecs::{model::ModelHandle, task::spawn_task, ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::{
    box_style::{BoxShadow, Corners, Sides},
    scroll_view::scroll_view,
    text_box, AlignItems, Anchor, Button, Checkbox, ColorRect, Column, Container, CursorIcon,
    Easing, Image, JustifyContent, Length, Link, ObjectFit, Overflow, Panel, RichText, Row,
    ScrollView, Slider, Text, TextBox, Wrap,
};
use matcha_window::adapter::Adapter;

const IMAGE_BYTES: &[u8] = include_bytes!("../../matcha/src/assets/videoframe_21710.png");

/// Held once and `.clone()`d into every `view()` call: `Image` keys its decode
/// cache on this `Arc`'s pointer identity, so re-deriving one per call would
/// defeat both the cache and `patch`'s change detection.
static IMAGE: LazyLock<Arc<[u8]>> = LazyLock::new(|| Arc::from(IMAGE_BYTES));

const INK: [f32; 4] = [0.88, 0.88, 0.92, 1.0];
const MUTED: [f32; 4] = [0.55, 0.55, 0.62, 1.0];
const SURFACE: [f32; 4] = [0.14, 0.14, 0.17, 1.0];
const OUTLINE: [f32; 4] = [0.30, 0.30, 0.36, 1.0];
const ACCENT: [f32; 4] = [0.35, 0.60, 0.95, 1.0];

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

struct Model {
    count: i32,
    agree: bool,
    notify: bool,
    volume: f32,
    quality: f32,
    note: String,
    title: String,
    overlay_open: bool,
    show_animated: bool,
    fetch: Fetch,
}

#[derive(PartialEq)]
enum Fetch {
    Idle,
    Loading,
    Done(String),
}

#[derive(Clone, PartialEq, Debug)]
enum Msg {
    Inc,
    Dec,
    ToggleAgree,
    ToggleNotify,
    Volume(f32),
    Quality(f32),
    NoteEdited(String),
    TitleConfirmed(String),
    ToggleOverlay,
    ToggleAnimated,
    OpenDocs,
    StartFetch,
    FetchDone(String),
}

fn reduce(model: &mut Model, msg: Msg) {
    match msg {
        Msg::Inc => model.count += 1,
        Msg::Dec => model.count -= 1,
        Msg::ToggleAgree => model.agree = !model.agree,
        Msg::ToggleNotify => model.notify = !model.notify,
        Msg::Volume(v) => model.volume = v,
        Msg::Quality(v) => model.quality = v,
        Msg::NoteEdited(text) => model.note = text,
        Msg::TitleConfirmed(text) => model.title = text,
        Msg::ToggleOverlay => model.overlay_open = !model.overlay_open,
        Msg::ToggleAnimated => model.show_animated = !model.show_animated,
        Msg::OpenDocs => log::info!("link clicked"),
        Msg::StartFetch => model.fetch = Fetch::Loading,
        Msg::FetchDone(text) => model.fetch = Fetch::Done(text),
    }
}

// ---------------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------------

fn heading(s: &mut Scope, text: &str) {
    s.leaf(Text::new(text).font_size(15.0).color(INK));
}

fn note(s: &mut Scope, text: &str) {
    s.leaf(Text::new(text).font_size(12.0).color(MUTED));
}

/// A titled block. Every section is one of these, so the page reads as a list.
fn section(s: &mut Scope, key: u64, title: &str, body: impl FnOnce(&mut Scope)) {
    s.node(Column::new().key(key).gap(8.0).width(Length::Fill), |s| {
        heading(s, title);
        body(s);
        // A hairline rule: `Sides::bottom` is CSS `border-bottom`.
        s.leaf(
            ColorRect::new(1.0, 1.0)
                .color([0.22, 0.22, 0.27, 1.0])
                .width(Length::Fill)
                .height(Length::Px(1.0)),
        );
    });
}

fn action_button(label: &str, msg: Msg, base: [f32; 4], hover: [f32; 4], active: [f32; 4]) -> Button<Msg> {
    Button::new(label)
        .on(msg)
        .color(base)
        .hover_color(hover)
        .active_color(active)
        .transition(Duration::from_millis(140), Easing::EaseInOut)
        .radius(8.0)
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

/// Hover, active, transition, focus rings, and the widgets that dispatch clicks.
fn controls(s: &mut Scope, model: &Model) {
    section(s, 1, "Controls — hover, press, focus, tab", |s| {
        note(
            s,
            "Hover for a colour transition; hold and drag off to see :active release. Tab cycles focus.",
        );
        s.node(Row::new().gap(10.0).align_items(AlignItems::Center), |s| {
            s.leaf(action_button(
                "−",
                Msg::Dec,
                [0.55, 0.25, 0.28, 1.0],
                [0.72, 0.34, 0.37, 1.0],
                [0.38, 0.16, 0.18, 1.0],
            ));
            s.leaf(
                Text::new(format!("count: {}", model.count))
                    .font_size(18.0)
                    .color(INK),
            );
            s.leaf(action_button(
                "+",
                Msg::Inc,
                [0.24, 0.52, 0.34, 1.0],
                [0.33, 0.68, 0.45, 1.0],
                [0.15, 0.35, 0.22, 1.0],
            ));
            s.leaf(
                Button::<Msg>::new("no message")
                    .color([0.26, 0.26, 0.32, 1.0])
                    .hover_color([0.34, 0.34, 0.42, 1.0])
                    .transition(Duration::from_millis(140), Easing::EaseInOut)
                    .radius(8.0)
                    .cursor(CursorIcon::NotAllowed),
            );
        });

        s.node(Row::new().gap(16.0).align_items(AlignItems::Center), |s| {
            s.leaf(Checkbox::new(model.agree).on(Msg::ToggleAgree).radius(4.0));
            s.leaf(Text::new("checkbox").font_size(13.0).color(INK));
            // Half the size as a radius is a radio button — same widget.
            s.leaf(
                Checkbox::new(model.notify)
                    .on(Msg::ToggleNotify)
                    .size(20.0)
                    .radius(10.0)
                    .fill_color(ACCENT),
            );
            s.leaf(Text::new("radio (same widget, radius = size/2)").font_size(13.0).color(INK));
            s.leaf(Link::new("a link").on(Msg::OpenDocs));
        });
    });
}

/// Sliders: pointer capture, keyboard steps, and `step` quantisation.
fn sliders(s: &mut Scope, model: &Model) {
    section(s, 2, "Slider — drag past the ends, or tab to it and use arrows", |s| {
        note(s, "The drag keeps working outside the track, and outside the window: a consumed press captures the pointer.");
        s.node(Row::new().gap(14.0).align_items(AlignItems::Center), |s| {
            s.leaf(
                Slider::new(model.volume, 0.0, 100.0)
                    .on_change(Msg::Volume)
                    .size(240.0, 26.0),
            );
            s.leaf(
                Text::new(format!("continuous: {:.1}", model.volume))
                    .font_size(13.0)
                    .color(MUTED),
            );
        });
        s.node(Row::new().gap(14.0).align_items(AlignItems::Center), |s| {
            s.leaf(
                Slider::new(model.quality, 0.0, 5.0)
                    .step(1.0)
                    .on_change(Msg::Quality)
                    .size(240.0, 26.0)
                    .colors([0.25, 0.25, 0.3, 1.0], [0.85, 0.6, 0.25, 1.0], [0.95, 0.95, 0.98, 1.0]),
            );
            s.leaf(
                Text::new(format!("step 1: {}", model.quality as i32))
                    .font_size(13.0)
                    .color(MUTED),
            );
        });
    });
}

/// Text input: keyboard, IME, clipboard.
fn text_input(s: &mut Scope, model: &Model) {
    section(s, 3, "Text input — type, use an IME, Ctrl+C / Ctrl+X / Ctrl+V", |s| {
        note(s, "Echoes use RichText: it has font fallback, so non-Latin input renders instead of tofu.");
        s.node(Row::new().gap(16.0).align_items(AlignItems::Start), |s| {
            s.node(Column::new().gap(6.0), |s| {
                note(s, "on_update — every keystroke");
                s.leaf(
                    TextBox::new(300.0, 84.0)
                        .value(&model.note)
                        .on_update(|t| Msg::NoteEdited(t.to_string())),
                );
                s.leaf(RichText::new(format!("→ {}", model.note)).font_size(13.0).color(MUTED));
            });
            s.node(Column::new().gap(6.0), |s| {
                note(s, "on_confirm + Enter — submits, no newline");
                s.leaf(
                    TextBox::new(300.0, 44.0)
                        .value(&model.title)
                        .confirm_key(text_box::confirm_on_enter)
                        .on_confirm(|t| Msg::TitleConfirmed(t.to_string())),
                );
                s.leaf(RichText::new(format!("→ {}", model.title)).font_size(13.0).color(MUTED));
            });
        });
    });
}

/// Everything `box_style` can express.
fn decoration(s: &mut Scope) {
    section(s, 4, "Box decoration — border, radius, per-side border, shadow", |s| {
        note(s, "Square and unbordered costs no rasterisation at all; a curve or a shadow costs one cached bitmap.");
        s.node(
            Row::new().gap(14.0).wrap(Wrap::Wrap).align_items(AlignItems::Start),
            |s| {
                let label = |s: &mut Scope, t: &str| s.leaf(Text::new(t).font_size(12.0).color(INK));
                s.node(Panel::new(130.0, 62.0).background_color(SURFACE), |s| label(s, "plain"));
                s.node(
                    Panel::new(130.0, 62.0)
                        .background_color(SURFACE)
                        .border_width(2.0)
                        .border_color(OUTLINE),
                    |s| label(s, "border"),
                );
                s.node(
                    Panel::new(130.0, 62.0)
                        .background_color(SURFACE)
                        .border_width(2.0)
                        .border_color(ACCENT)
                        .radius(14.0),
                    |s| label(s, "radius"),
                );
                s.node(
                    Panel::new(130.0, 62.0)
                        .background_color(SURFACE)
                        .corners(Corners::top(16.0)),
                    |s| label(s, "per-corner"),
                );
                s.node(
                    Panel::new(130.0, 62.0)
                        .background_color(SURFACE)
                        .borders(Sides::bottom(3.0))
                        .border_color([0.9, 0.35, 0.5, 1.0]),
                    |s| label(s, "border-bottom"),
                );
                s.node(
                    Panel::new(130.0, 62.0)
                        .background_color([0.22, 0.22, 0.27, 1.0])
                        .radius(10.0)
                        .shadow(BoxShadow::drop(6.0, 18.0, [0.0, 0.0, 0.0, 0.75])),
                    |s| label(s, "shadow"),
                );
            },
        );
    });
}

/// Flexbox-lite: justify, align, wrap, grow.
fn layout(s: &mut Scope) {
    section(s, 5, "Layout — justify, align, wrap, grow (resize the window)", |s| {
        note(s, "The first row fills the width, so justify_content has leftover space to distribute.");
        let swatch = |w: f32, h: f32, c: [f32; 4]| ColorRect::new(w, h).color(c).radius(4.0);

        s.node(
            Row::new()
                .width(Length::Fill)
                .gap(8.0)
                .justify_content(JustifyContent::SpaceBetween)
                .align_items(AlignItems::Center),
            |s| {
                s.leaf(swatch(44.0, 30.0, [0.30, 0.50, 0.90, 1.0]));
                s.leaf(swatch(44.0, 18.0, [0.90, 0.60, 0.20, 1.0]));
                s.leaf(swatch(44.0, 42.0, [0.35, 0.75, 0.45, 1.0]));
            },
        );
        note(s, "grow: the middle one takes all the leftover space");
        s.node(Row::new().width(Length::Fill).gap(8.0), |s| {
            s.leaf(swatch(60.0, 22.0, [0.45, 0.45, 0.55, 1.0]));
            s.leaf(swatch(60.0, 22.0, [0.35, 0.60, 0.95, 1.0]).grow(1.0));
            s.leaf(swatch(60.0, 22.0, [0.45, 0.45, 0.55, 1.0]));
        });
        note(s, "wrap: narrow the window until these break onto a second line");
        s.node(
            Row::new().width(Length::Fill).gap(8.0).wrap(Wrap::Wrap),
            |s| {
                for i in 0..9 {
                    let t = i as f32 / 8.0;
                    s.leaf(
                        swatch(96.0, 20.0, [0.25 + t * 0.5, 0.35, 0.75 - t * 0.4, 1.0])
                            .key(i as u64)
                            .shrink(0.0),
                    );
                }
            },
        );
    });
}

/// Text rendering: shaping, fallback, per-span styling, decorations.
fn text(s: &mut Scope) {
    section(s, 6, "Text — shaping, font fallback, per-span styling", |s| {
        s.leaf(
            RichText::new("Mixed scripts and emoji: 日本語のテキスト、English, 🎨🚀 — parley shapes and falls back per run.")
                .font_size(15.0)
                .color(INK),
        );
        s.leaf(
            RichText::new("Per-span: ")
                .font_size(15.0)
                .color(INK)
                .span("bold red", |sp| sp.font_weight(matcha_ecs_widgets::parley::FontWeight::BOLD).color([0.95, 0.4, 0.4, 1.0]))
                .span(", ", |sp| sp)
                .span("italic underlined", |sp| {
                    sp.font_style(matcha_ecs_widgets::parley::FontStyle::Italic)
                        .underline(true)
                })
                .span(", ", |sp| sp)
                .span("struck through", |sp| sp.strikethrough(true)),
        );
        note(s, "Text (suzuri/fontdue) is the fallback engine — no shaping, no font fallback:");
        s.leaf(Text::new("Text widget: Latin only, kerning but no shaping").font_size(13.0).color(MUTED));
    });
}

/// Clipping and scrolling, including chaining out of the inner box.
fn scrolling(s: &mut Scope) {
    section(s, 7, "Clip and scroll — scroll the inner box to its end, then keep going", |s| {
        note(s, "A pinned scroll container returns the event, so the page takes over: CSS scroll chaining.");
        s.node(Row::new().gap(16.0).align_items(AlignItems::Start), |s| {
            s.node(
                Panel::new(240.0, 130.0)
                    .background_color(SURFACE)
                    .border_width(1.0)
                    .border_color(OUTLINE)
                    .radius(8.0),
                |s| {
                    scroll_view(
                        s,
                        ScrollView::new(220.0, 110.0).overflow_y(Overflow::Auto),
                        |s| {
                            s.node(Column::new().gap(4.0), |s| {
                                for i in 0..16 {
                                    s.leaf(
                                        Text::new(format!("scrollable row {i}"))
                                            .font_size(13.0)
                                            .color(if i % 2 == 0 { INK } else { MUTED })
                                            .key(i as u64),
                                    );
                                }
                            });
                        },
                    );
                },
            );
            // `clip(true)` is CSS `overflow: hidden`: the child is laid out at
            // its full size and simply cut off.
            s.node(
                Panel::new(200.0, 130.0)
                    .background_color(SURFACE)
                    .border_width(1.0)
                    .border_color([0.9, 0.35, 0.5, 1.0])
                    .radius(8.0)
                    .clip(true),
                |s| {
                    s.leaf(
                        ColorRect::new(400.0, 300.0)
                            .color([0.9, 0.35, 0.5, 0.5])
                            .radius(20.0),
                    );
                },
            );
        });
    });
}

/// Images and object-fit.
fn images(s: &mut Scope) {
    section(s, 8, "Image — object-fit", |s| {
        s.node(Row::new().gap(12.0).align_items(AlignItems::Start), |s| {
            for (i, (fit, label)) in [
                (ObjectFit::Contain, "contain"),
                (ObjectFit::Fill, "fill"),
                (ObjectFit::Cover, "cover"),
                (ObjectFit::ScaleDown, "scale-down"),
            ]
            .into_iter()
            .enumerate()
            {
                s.node(Column::new().key(i as u64).gap(4.0), |s| {
                    s.node(
                        Panel::new(150.0, 90.0)
                            .background_color([0.08, 0.08, 0.10, 1.0])
                            .radius(6.0)
                            .clip(true),
                        |s| {
                            s.leaf(Image::from_bytes(IMAGE.clone(), 140.0, 80.0).fit(fit));
                        },
                    );
                    note(s, label);
                });
            }
        });
    });
}

/// Overlays, `display: none`, fades, and an async task.
fn dynamics(s: &mut Scope, model: &Model) {
    section(s, 9, "Overlay, display:none, fades, async", |s| {
        s.node(Row::new().gap(10.0).align_items(AlignItems::Center), |s| {
            s.leaf(action_button(
                if model.overlay_open { "close overlay" } else { "open overlay" },
                Msg::ToggleOverlay,
                [0.30, 0.30, 0.38, 1.0],
                [0.40, 0.40, 0.50, 1.0],
                [0.20, 0.20, 0.26, 1.0],
            ));
            s.leaf(action_button(
                if model.show_animated { "hide row" } else { "show row" },
                Msg::ToggleAnimated,
                [0.30, 0.30, 0.38, 1.0],
                [0.40, 0.40, 0.50, 1.0],
                [0.20, 0.20, 0.26, 1.0],
            ));
            s.leaf(action_button(
                "fetch",
                Msg::StartFetch,
                [0.28, 0.40, 0.60, 1.0],
                [0.36, 0.52, 0.78, 1.0],
                [0.18, 0.26, 0.40, 1.0],
            ));
            s.leaf(
                Text::new(match &model.fetch {
                    Fetch::Idle => "idle".to_string(),
                    Fetch::Loading => "loading…".to_string(),
                    Fetch::Done(text) => text.clone(),
                })
                .font_size(13.0)
                .color(MUTED),
            );
            if model.fetch == Fetch::Loading {
                s.leaf(FetchTrigger);
            }

            // The overlay: zero-sized in the flow, drawn over its siblings.
            // Declared *inside* the row so its offset is relative to here.
            s.node(Anchor::at(0.0, 28.0).z_index(10).key(99u64), |s| {
                s.node(
                    Container::new().visible(model.overlay_open),
                    |s| {
                        s.node(
                            Panel::new(220.0, 96.0)
                                .background_color([0.20, 0.20, 0.26, 1.0])
                                .border_width(1.0)
                                .border_color(ACCENT)
                                .radius(10.0)
                                .shadow(BoxShadow::drop(8.0, 24.0, [0.0, 0.0, 0.0, 0.8])),
                            |s| {
                                s.node(Column::new().gap(6.0).align_items(AlignItems::Center), |s| {
                                    s.leaf(Text::new("I am over the buttons").font_size(13.0).color(INK));
                                    s.leaf(Text::new("and I intercept their clicks").font_size(12.0).color(MUTED));
                                });
                            },
                        );
                    },
                );
            });
        });

        // `Container::visible(false)` is `display: none`: the row claims no
        // space at all, so the gap closes rather than leaving a hole. The rects
        // fade in when it comes back.
        s.node(Container::new().key(50u64).visible(model.show_animated), |s| {
            s.node(Row::new().gap(8.0), |s| {
                for i in 0..5 {
                    s.leaf(
                        ColorRect::new(52.0, 30.0)
                            .color([0.30 + i as f32 * 0.1, 0.45, 0.85, 1.0])
                            .radius(6.0)
                            .key(i as u64)
                            .enter_fade(Duration::from_millis(320), Easing::EaseInOut)
                            .exit_fade(Duration::from_millis(320), Easing::EaseInOut),
                    );
                }
            });
        });
    });
}

/// Cursor shapes, and a place to see the leaf-to-root resolution.
fn cursors(s: &mut Scope) {
    section(s, 10, "Cursor — move the pointer across these", |s| {
        s.node(Row::new().gap(8.0).wrap(Wrap::Wrap), |s| {
            for (i, (icon, label)) in [
                (CursorIcon::Default, "default"),
                (CursorIcon::Pointer, "pointer"),
                (CursorIcon::Text, "text"),
                (CursorIcon::Crosshair, "crosshair"),
                (CursorIcon::Move, "move"),
                (CursorIcon::Grab, "grab"),
                (CursorIcon::NotAllowed, "not-allowed"),
                (CursorIcon::ResizeHorizontal, "resize-h"),
                (CursorIcon::Wait, "wait"),
            ]
            .into_iter()
            .enumerate()
            {
                // `Button` is the handy pickable box; it needs no message to
                // carry a cursor.
                s.leaf(
                    Button::<Msg>::new(label)
                        .key(i as u64)
                        .size(104.0, 30.0)
                        .font_size(12.0)
                        .color([0.22, 0.22, 0.27, 1.0])
                        .hover_color([0.30, 0.30, 0.38, 1.0])
                        .radius(6.0)
                        .cursor(icon),
                );
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Async: a zero-sized widget whose only job is to start a task
// ---------------------------------------------------------------------------

/// Invisible leaf: spawns a simulated fetch on spawn and reports back through
/// `ModelHandle`. Declared only while the model says a fetch is in flight, so
/// the reconciler starting and stopping it *is* the task's lifetime.
struct FetchTrigger;

impl matcha_ecs::view::Widget for FetchTrigger {
    fn bundle(&self) -> impl bevy_ecs::bundle::Bundle {
        (
            matcha_ecs_widgets::color_rect::RectGeometry { w: 0.0, h: 0.0 },
            matcha_ecs::layout::LayoutDispatch::of::<matcha_ecs_widgets::color_rect::RectGeometry>(),
        )
    }

    fn after_spawn(&self, entity: &mut bevy_ecs::world::EntityWorldMut) {
        let handle = entity.resource::<ModelHandle<Model>>().clone();
        spawn_task(entity, matcha_ecs::task::TaskKey(1), async move {
            // Blocking is fine here: this runs on an `AsyncComputeTaskPool`
            // worker, not the UI thread.
            std::thread::sleep(Duration::from_millis(900));
            handle.update(|model| {
                model.fetch = Fetch::Done("fetched ✓".to_string());
            });
        });
    }

    fn patch(&self, _entity: &mut bevy_ecs::world::EntityWorldMut) {}
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

fn view(model: &Model, s: &mut Scope) {
    // The whole page scrolls. `overflow_x: Hidden` is the default and is what
    // keeps text wrapping to the window's width rather than running off it.
    scroll_view(
        s,
        // `ScrollView` is the one widget that does not route through `Sizing`
        // (its viewport size is load-bearing in three places), so "fill the
        // window" is expressed by declaring something larger than any window
        // and letting `measure`'s clamp bring it down to what the root offers.
        ScrollView::new(20_000.0, 20_000.0).overflow_y(Overflow::Auto),
        |s| {
            s.node(
                Column::new().gap(18.0).width(Length::Fill),
                |s| {
                    s.leaf(
                        Text::new("matcha — showcase")
                            .font_size(22.0)
                            .color(INK),
                    );
                    note(s, "Every widget and behaviour in one window. See the module docs for what to try.");

                    controls(s, model);
                    sliders(s, model);
                    text_input(s, model);
                    decoration(s);
                    layout(s);
                    text(s);
                    scrolling(s);
                    images(s);
                    dynamics(s, model);
                    cursors(s);
                },
            );
        },
    );
}

fn main() {
    env_logger::init();
    Adapter::new(
        UiEcs::new(
            Model {
                count: 0,
                agree: false,
                notify: true,
                volume: 42.0,
                quality: 3.0,
                note: String::new(),
                title: "press Enter to submit".to_string(),
                overlay_open: false,
                show_animated: true,
                fetch: Fetch::Idle,
            },
            view,
            reduce,
        )
        // Without this: no exit fades, no caret blink, no colour transitions.
        .with_pre_layout_systems(matcha_ecs_widgets::default_systems()),
    )
    .run()
    .expect("event loop failed");
}
