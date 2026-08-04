//! The web build of matcha: the whole page is one `<canvas>`, drawn by the
//! framework, the way egui does it.
//!
//! ```text
//! trunk serve                 # http://localhost:8080, rebuilds on save
//! trunk build --release       # -> dist/
//! cargo run -p matcha-web     # the same UI in a desktop window
//! ```
//!
//! Without Trunk:
//!
//! ```text
//! cargo build -p matcha-web --target wasm32-unknown-unknown --release
//! wasm-bindgen --target web --out-dir dist \
//!     target/wasm32-unknown-unknown/release/matcha-web.wasm
//! ```
//!
//! Always build the wasm target with `-p matcha-web`, never `--workspace`:
//! `matcha` and `matcha-tree` pull in `tokio/rt-multi-thread`, which does not
//! exist there.
//!
//! # How the page and the canvas fit together
//!
//! `index.html` sizes `#matcha-canvas` to the viewport in CSS and this program
//! never sets an inner size, because winit's `request_inner_size` on the web
//! sets the canvas's *CSS* size and would fight the stylesheet. winit installs a
//! `ResizeObserver` of its own, so resizing the browser window arrives as an
//! ordinary `Resized` event in physical pixels, and nothing here needs any JS.
//!
//! # Known gaps on the web
//!
//! - **IME does not work.** winit 0.30's web backend makes `set_ime_allowed` a
//!   no-op and never emits `WindowEvent::Ime`, so a `TextBox` cannot accept
//!   Japanese input. Direct keyboard input is fine.
//! - **The clipboard is inert** (`navigator.clipboard` is async and
//!   gesture-gated; see `matcha-window/src/clipboard/web.rs`).
//! - **`Image::from_path` does nothing** — there is no filesystem. Use
//!   `from_bytes` with `include_bytes!`.

use matcha_ecs::{ui_ecs::UiEcs, view::Scope};
use matcha_ecs_widgets::{
    AlignItems, Button, Checkbox, ColorRect, Column, JustifyContent, Length, Panel, RichText, Row,
    Text,
};

const INK: [f32; 4] = [0.90, 0.90, 0.94, 1.0];
const SURFACE: [f32; 4] = [0.14, 0.14, 0.17, 1.0];
const OUTLINE: [f32; 4] = [0.30, 0.30, 0.36, 1.0];
const ACCENT: [f32; 4] = [0.35, 0.60, 0.95, 1.0];

/// The id of the `<canvas>` in `index.html`.
const CANVAS_ID: &str = "matcha-canvas";

struct Model {
    count: i32,
    agree: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Increment,
    Decrement,
    ToggleAgree,
}

fn reduce(model: &mut Model, msg: Msg) {
    match msg {
        Msg::Increment => model.count += 1,
        Msg::Decrement => model.count -= 1,
        Msg::ToggleAgree => model.agree = !model.agree,
    }
}

fn view(model: &Model, s: &mut Scope) {
    s.node(Column::new().gap(18.0).width(Length::Fill), |s| {
        // A plain rect first, deliberately: geometry and text fail
        // independently, so if fonts are missing this still proves the
        // pipeline reached the canvas.
        s.leaf(ColorRect::new(360.0, 6.0).color(ACCENT));

        s.leaf(
            Text::new("matcha — a Rust GUI framework, in the browser")
                .font_size(26.0)
                .color(INK),
        );

        // RichText rather than Text: it has font fallback and real shaping, so
        // it is what can render Japanese.
        s.leaf(
            RichText::new(
                "このページ全体が 1 枚の <canvas> です。ボタンも文字も、\
                 すべて wgpu (WebGPU) で描画しています。",
            )
            .font_size(16.0)
            .color(INK),
        );

        s.node(
            Panel::new(380.0, 170.0)
                .background_color(SURFACE)
                .border_color(OUTLINE)
                .border_width(1.0)
                .radius(10.0),
            |s| {
                s.node(
                    Column::new()
                        .gap(12.0)
                        .align_items(AlignItems::Center)
                        .justify_content(JustifyContent::Center),
                    |s| {
                        s.leaf(
                            Text::new(format!("count: {}", model.count))
                                .font_size(20.0)
                                .color(INK),
                        );

                        s.node(Row::new().gap(8.0), |s| {
                            s.leaf(Button::new("-").on(Msg::Decrement).color(ACCENT).radius(8.0));
                            s.leaf(Button::new("+").on(Msg::Increment).color(ACCENT).radius(8.0));
                        });

                        s.node(Row::new().gap(8.0).align_items(AlignItems::Center), |s| {
                            s.leaf(Checkbox::new(model.agree).on(Msg::ToggleAgree).radius(4.0));
                            s.leaf(
                                Text::new("clicks route through the reducer")
                                    .font_size(13.0)
                                    .color(INK),
                            );
                        });
                    },
                );
            },
        );
    });
}

fn model() -> Model {
    Model {
        count: 0,
        agree: false,
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

#[cfg(not(web))]
fn main() {
    env_logger::init();
    let app = UiEcs::new(model(), view, reduce)
        .with_pre_layout_systems(matcha_ecs_widgets::default_systems());
    matcha_window::adapter::Adapter::new(app)
        .run()
        .expect("event loop failed");
}

/// On the web the browser owns the event loop, and GPU initialisation is
/// genuinely asynchronous, so startup is a spawned future rather than a call
/// that returns when the app exits.
#[cfg(web)]
fn main() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);

    wasm_bindgen_futures::spawn_local(async {
        // Physical pixels per CSS pixel. Without this the whole UI renders at
        // 1/dpr scale on any HiDPI display or zoomed browser.
        let dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio() as f32)
            .unwrap_or(1.0);

        log::info!("matcha-web: starting (devicePixelRatio = {dpr})");

        let config = matcha_window::window::WindowConfig::default()
            .with_title("matcha")
            .with_canvas_id(CANVAS_ID);

        // GPU init is the one genuinely async step and the most likely thing to
        // fail on an unfamiliar machine, so bracket it: seeing "starting"
        // without "GPU ready" in the console localises the problem instantly.
        let app = UiEcs::new_async(model(), view, reduce).await;
        log::info!("matcha-web: GPU ready, handing off to the browser event loop");

        let app = app
            .with_window_config(config)
            .with_ui_scale(dpr)
            .with_pre_layout_systems(matcha_ecs_widgets::default_systems());

        // Returns immediately: winit hands the app to the browser's event loop.
        matcha_window::adapter::Adapter::new(app).run();
    });
}
