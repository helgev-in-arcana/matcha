//! Run with `cargo run -p matcha-better-tree --example poll_animation`.
//! The square moves for four seconds without input, then stays at the right edge.
//! Resize the window while it moves to trigger extra polls between deadlines.
use std::time::Duration;

use matcha_better_tree::metrics::{Bounds, Constraints};
use matcha_better_tree::ui_tree::context::Context;
use matcha_better_tree::ui_tree::widget::{
    Color, ControlFlow, Draw, InputResult, PollInfo, Lifecycle, UpdateResult, View, Widget,
};
use matcha_better_tree::ui_tree::window::{Apply, Window, WindowConfig};
use matcha_better_tree::ui_tree::{Children, DeclContext};
use matcha_better_tree::{Model, ModelContext};
use matcha_window::event::device_event::DeviceEvent;
use nalgebra::{Matrix4, Vector3};
use render_interface::{TextureDescriptor, TextureSource, unit_quad, upload_texture};
use web_time::Instant;

const FRAME: Duration = Duration::from_millis(40);
const LAST_FRAME: u32 = 100;

struct Demo;

impl Model for Demo {
    fn view(&mut self, _: &ModelContext<'_>) -> impl View {
        Window::new(MovingSquare)
            .with_config(WindowConfig {
                title: "Matcha · poll animation".into(),
                inner_size: Some(Apply::Init([640, 200].into())),
                ..Default::default()
            })
            .on_close_requested(|| std::process::exit(0))
    }
}

struct MovingSquare;

impl View for MovingSquare {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        Some("moving square")
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        Square {
            start: ctx.context().frame_time(),
            frame: 0,
        }
    }
}

struct Square {
    start: Instant,
    frame: u32,
}

impl Widget for Square {
    type V = MovingSquare;

    fn update<'a>(&mut self, _: &'a MovingSquare, _: &mut DeclContext<'a, '_>) -> UpdateResult {
        UpdateResult::empty()
    }

    fn poll(&mut self, _: PollInfo, ctx: &mut Context) -> Lifecycle {
        let elapsed = ctx.frame_time().duration_since(self.start);
        let frame = (elapsed.as_millis() / FRAME.as_millis()).min(LAST_FRAME.into()) as u32;
        let update = if frame != self.frame {
            self.frame = frame;
            UpdateResult::RENDER
        } else {
            UpdateResult::empty()
        };
        // The start time anchors every deadline, even if a resize polls us early
        // or a delayed wake skips several frames.
        let next_poll = if frame < LAST_FRAME {
            ControlFlow::WaitUntil(self.start + FRAME * (frame + 1))
        } else {
            ControlFlow::Wait
        };
        Lifecycle::Keep { update, next_poll }
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }

    fn measure(&self, constraints: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds {
            x: constraints.x[0],
            y: constraints.y[0],
            z: 0.0,
        }
    }

    fn layout(&self, _: Bounds, _: Children<'_>, _: &mut Context) {}

    fn render(&self, bounds: Bounds, _: &mut Context) -> Draw {
        let side = 48.0_f32.min(bounds.x).min(bounds.y);
        let progress = self.frame as f32 / LAST_FRAME as f32;
        let x = (bounds.x - side) * progress;
        let y = (bounds.y - side) * 0.5;
        let square = Color {
            mesh: unit_quad(),
            texture: TextureSource::new(
                TextureDescriptor::new([1, 1], wgpu::TextureFormat::Rgba8UnormSrgb),
                |mut ctx| upload_texture(&mut ctx.gpu, &ctx.target, &[95, 215, 130, 255]),
            ),
            transform: Matrix4::new_translation(&Vector3::new(x, y, 0.0))
                * Matrix4::new_nonuniform_scaling(&Vector3::new(side, side, 1.0)),
            opacity: 1.0,
        };
        Draw {
            color: smallvec::smallvec![square],
            mask: None,
        }
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use matcha_better_tree::App;
    use matcha_better_tree::app::{Gpu, GpuDescriptor};
    use matcha_window::adapter::Adapter;

    let gpu = futures::executor::block_on(Gpu::new(GpuDescriptor::standard()))?;
    Adapter::new(App::new(Demo, gpu)).run()?;
    Ok(())
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn main() {
    eprintln!("Run this native winit example on a desktop.");
}
