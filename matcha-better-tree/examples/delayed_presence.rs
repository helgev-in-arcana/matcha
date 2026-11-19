//! Run with `cargo run -p matcha-better-tree --example delayed_presence`.
//! Drive a notice through entry, a cancelled exit, and a completed exit using
//! explicit frame times, so the example needs neither a window nor a timer.
use std::time::Duration;

use matcha_better_tree::UiTree;
use matcha_better_tree::metrics::{Bounds, Constraints, Position};
use matcha_better_tree::ui_tree::context::Context;
use matcha_better_tree::ui_tree::widget::{
    ControlFlow, Draw, InputResult, Lifecycle, PollInfo, UpdateResult, View, Widget, WidgetPhase,
};
use matcha_better_tree::ui_tree::window::WindowRegistry;
use matcha_better_tree::ui_tree::{Children, DeclContext};
use matcha_window::event::device_event::DeviceEvent;
use web_time::Instant;

const DELAY: Duration = Duration::from_millis(300);

struct Notice;
struct NoticeWidget {
    enter_at: Instant,
    exit_at: Option<Instant>,
}

impl View for Notice {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        Some("notice")
    }

    fn key(&self) -> Option<u64> {
        Some(1)
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        ctx.start_inactive();
        NoticeWidget {
            enter_at: ctx.context().frame_time() + DELAY,
            exit_at: None,
        }
    }
}

impl Widget for NoticeWidget {
    type V = Notice;

    fn update<'a>(&mut self, _: &'a Notice, _: &mut DeclContext<'a, '_>) -> UpdateResult {
        // Re-declaration cancels any previous exit, even if no poll runs before the next omission.
        self.exit_at = None;
        UpdateResult::empty()
    }

    fn poll(&mut self, info: PollInfo, ctx: &mut Context) -> Lifecycle {
        let now = ctx.frame_time();
        let deadline = match info.phase {
            WidgetPhase::PendingEnter => self.enter_at,
            WidgetPhase::Active => return info.phase.default_lifecycle(),
            WidgetPhase::Exiting => *self.exit_at.get_or_insert_with(|| now + DELAY),
        };
        if now < deadline {
            Lifecycle::Keep {
                next_poll: ControlFlow::WaitUntil(deadline),
                update: UpdateResult::empty(),
            }
        } else {
            info.phase.default_lifecycle()
        }
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }
    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds::default()
    }
    fn layout(&self, _: Bounds, _: Children<'_>, _: &mut Context) {}
    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        println!("  paint notice");
        Draw::default()
    }
}

struct Page(Option<Notice>);
struct PageWidget;

impl Page {
    fn declare<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) {
        if let Some(notice) = &self.0 {
            ctx.child(notice);
        }
    }
}

impl View for Page {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        Some("page")
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        self.declare(ctx);
        PageWidget
    }
}

impl Widget for PageWidget {
    type V = Page;

    fn update<'a>(&mut self, page: &'a Page, ctx: &mut DeclContext<'a, '_>) -> UpdateResult {
        page.declare(ctx);
        UpdateResult::empty()
    }
    fn poll(&mut self, info: PollInfo, _: &mut Context) -> Lifecycle {
        if info.active_children_changed {
            println!("  page: active children changed");
        }
        info.phase.default_lifecycle()
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }
    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        Bounds::default()
    }
    fn layout(&self, bounds: Bounds, mut children: Children<'_>, ctx: &mut Context) {
        println!("  layout: {} active children", children.len());
        children.for_each(|_, mut child| {
            child.layout(
                Position {
                    bounds,
                    ..Position::default()
                },
                ctx,
            )
        });
    }
    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        Draw::default()
    }
}

fn main() {
    let start = Instant::now();
    let mut windows = WindowRegistry::default();
    let mut tree = UiTree::new();
    let mut frame = |millis, declaration: Option<Page>| {
        println!("{millis} ms");
        let mut ctx =
            Context::new(&mut windows).with_frame_time(start + Duration::from_millis(millis));
        if let Some(page) = declaration {
            tree.update(&page, &mut ctx);
        }
        let next = tree.poll(&mut ctx).next_poll;
        match next {
            ControlFlow::WaitUntil(t) => println!("  next wake: {} ms", (t - start).as_millis()),
            other => println!("  next wake: {other:?}"),
        }
        tree.layout(Bounds::default(), &mut ctx);
        tree.render(&mut ctx).unwrap();
    };

    frame(0, Some(Page(Some(Notice))));
    frame(100, None); // Early poll keeps the 300 ms deadline.
    frame(300, None); // The notice joins layout.
    frame(400, Some(Page(None))); // Exit requested. Still present until 700 ms.
    frame(500, Some(Page(Some(Notice)))); // Same key and type: cancel exit without rebuilding.
    frame(700, None); // The cancelled deadline cannot remove the notice.
    frame(800, Some(Page(None))); // A new exit sets a new deadline.
    frame(1100, None); // Finally release the notice.
}
