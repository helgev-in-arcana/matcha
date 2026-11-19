use std::time::Duration;

use super::*;
use crate::ui_tree::widget::{ControlFlow, Lifecycle, PollInfo, WidgetPhase};
use crate::ui_tree::window::WindowRegistry;
use web_time::Instant;

const DELAY: Duration = Duration::from_millis(30);

// A row uses its child's slot for placement. The slot deliberately isn't Clone.
#[derive(PartialEq)]
struct Slot(u16);

#[derive(Default)]
struct Trace {
    builds: usize,
    drops: usize,
    polls: Vec<(WidgetPhase, bool)>,
    layouts: Vec<Vec<u16>>,
    measures: usize,
    renders: usize,
    inputs: usize,
}

struct Item {
    key: u64,
    enter_at: Option<Instant>,
    retain: bool,
    wake: ControlFlow,
    trace: Arc<Mutex<Trace>>,
    children: Vec<(u16, Item)>,
}

impl Item {
    fn new(key: u64) -> Self {
        Self {
            key,
            enter_at: None,
            retain: false,
            wake: ControlFlow::Wait,
            trace: Arc::default(),
            children: vec![],
        }
    }

    fn declare<'a>(&'a self, ctx: &mut DeclContext<'a, '_, Slot>) {
        for (offset, child) in &self.children {
            ctx.child_with(child, Slot(*offset));
        }
    }
}

struct ItemWidget {
    trace: Arc<Mutex<Trace>>,
    enter_at: Option<Instant>,
    exit_at: Option<Instant>,
    retain: bool,
    wake: ControlFlow,
}

impl View for Item {
    type ChildData = Slot;

    fn key(&self) -> Option<u64> {
        Some(self.key)
    }
    fn label(&self) -> Option<&str> {
        None
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_, Slot>) -> impl Widget<V = Self> {
        self.trace.lock().unwrap().builds += 1;
        if self.enter_at.is_some() {
            ctx.start_inactive();
        }
        self.declare(ctx);
        ItemWidget {
            trace: self.trace.clone(),
            enter_at: self.enter_at,
            exit_at: None,
            retain: self.retain,
            wake: self.wake,
        }
    }
}

impl Drop for ItemWidget {
    fn drop(&mut self) {
        self.trace.lock().unwrap().drops += 1;
    }
}

impl Widget for ItemWidget {
    type V = Item;

    fn update<'a>(&mut self, view: &'a Item, ctx: &mut DeclContext<'a, '_, Slot>) -> UpdateResult {
        // A declaration revives an exiting item even if no Active poll runs before its next exit.
        self.exit_at = None;
        view.declare(ctx);
        UpdateResult::empty()
    }

    fn poll(&mut self, info: PollInfo, ctx: &mut Context) -> Lifecycle {
        self.trace
            .lock()
            .unwrap()
            .polls
            .push((info.phase, info.active_children_changed));
        let wake = match info.phase {
            WidgetPhase::PendingEnter => {
                let deadline = self.enter_at.unwrap();
                if deadline <= ctx.frame_time() {
                    return Lifecycle::Enter {
                        update: UpdateResult::empty(),
                        next_poll: ControlFlow::Wait,
                    };
                }
                ControlFlow::WaitUntil(deadline)
            }
            WidgetPhase::Active => self.wake,
            WidgetPhase::Exiting if self.retain => {
                let now = ctx.frame_time();
                let deadline = *self.exit_at.get_or_insert(now + DELAY);
                if deadline <= now {
                    return Lifecycle::FinishExit;
                }
                ControlFlow::WaitUntil(deadline)
            }
            phase => return phase.default_lifecycle(),
        };
        Lifecycle::Keep {
            next_poll: wake,
            update: UpdateResult::empty(),
        }
    }

    fn measure(&self, _: Constraints, _: Children<'_, Slot>, _: &mut Context) -> Bounds {
        self.trace.lock().unwrap().measures += 1;
        bounds()
    }

    fn layout(&self, _: Bounds, mut children: Children<'_, Slot>, ctx: &mut Context) {
        let len = children.len();
        assert_eq!(children.is_empty(), len == 0);
        let mut slots = Vec::new();
        children.for_each(|slot, mut child| {
            slots.push(slot.0);
            child.measure(Constraints::default(), ctx);
            child.layout(position(translation(f32::from(slot.0), 0., 0.)), ctx);
        });
        assert_eq!(slots.len(), len);
        self.trace.lock().unwrap().layouts.push(slots);
    }

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        self.trace.lock().unwrap().renders += 1;
        Draw::default()
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        self.trace.lock().unwrap().inputs += 1;
        InputResult::default()
    }
}

#[test]
fn pending_subtree_enters_before_parent_poll_and_only_then_participates() {
    let now = Instant::now();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry).with_frame_time(now);
    let mut tree = UiTree::new();
    let mut root = Item::new(0);
    let mut child = Item::new(1);
    child.enter_at = Some(now + DELAY);
    let grandchild = Item::new(2);
    let leaf = grandchild.trace.clone();
    child.children.push((0, grandchild));
    root.children.push((20, child));
    tree.update(&root, &mut ctx);
    for elapsed in [Duration::ZERO, DELAY / 2] {
        let mut early = ctx.reborrow().with_frame_time(now + elapsed);
        assert_eq!(
            tree.poll(&mut early).next_poll,
            ControlFlow::WaitUntil(now + DELAY)
        );
        tree.layout(bounds(), &mut early);
        tree.render(&mut early).unwrap();
        let _ = tree.input(&mouse_event(25., 5.), &mut early);
    }
    let leaf_trace = leaf.lock().unwrap();
    assert_eq!(
        leaf_trace.polls.len(),
        2,
        "active descendant still polls below pending ancestor"
    );
    assert_eq!(
        (leaf_trace.measures, leaf_trace.renders, leaf_trace.inputs),
        (0, 0, 0)
    );
    drop(leaf_trace);
    assert_eq!(root.trace.lock().unwrap().layouts, [vec![]]);

    let mut due = ctx.reborrow().with_frame_time(now + DELAY);
    let result = tree.poll(&mut due);
    assert!(result.update.contains(UpdateResult::LAYOUT));
    assert_eq!(result.next_poll, ControlFlow::Wait);
    assert_eq!(
        root.trace.lock().unwrap().polls.last(),
        Some(&(WidgetPhase::Active, true))
    );
    tree.layout(bounds(), &mut due);
    tree.render(&mut due).unwrap();
    let _ = tree.input(&mouse_event(25., 5.), &mut due);
    let leaf_trace = leaf.lock().unwrap();
    assert_eq!(
        (leaf_trace.measures, leaf_trace.renders, leaf_trace.inputs),
        (1, 1, 1)
    );
    drop(leaf_trace);
    assert_eq!(root.trace.lock().unwrap().layouts, [vec![], vec![20]]);
    assert_eq!(tree.poll(&mut due), PollSummary::default());
    assert_eq!(
        root.trace.lock().unwrap().polls.last(),
        Some(&(WidgetPhase::Active, false))
    );
}

#[test]
fn retirement_keeps_slot_and_order_blocks_subtree_input_and_discards_subtree_wake() {
    let now = Instant::now();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry).with_frame_time(now);
    let mut root = Item::new(0);
    let mut b = Item::new(2);
    b.retain = true;
    let departing = b.trace.clone();
    let mut grandchild = Item::new(4);
    grandchild.wake = ControlFlow::WaitUntil(now + DELAY * 3);
    let leaf = grandchild.trace.clone();
    b.children.push((0, grandchild));
    root.children
        .extend([(0, Item::new(1)), (20, b), (40, Item::new(3))]);
    let mut tree = UiTree::new();
    tree.update(&root, &mut ctx);
    let _ = tree.poll(&mut ctx);
    tree.layout(bounds(), &mut ctx);
    let _ = tree.input(&mouse_event(25., 5.), &mut ctx);
    assert_eq!(leaf.lock().unwrap().inputs, 1);

    root.children.remove(1);
    tree.update(&root, &mut ctx);
    assert_eq!(
        tree.poll(&mut ctx).next_poll,
        ControlFlow::WaitUntil(now + DELAY)
    );
    let mut early = ctx.reborrow().with_frame_time(now + DELAY / 2);
    tree.update(&root, &mut early);
    assert_eq!(
        tree.poll(&mut early).next_poll,
        ControlFlow::WaitUntil(now + DELAY)
    );
    assert_eq!(
        root.trace.lock().unwrap().polls.last(),
        Some(&(WidgetPhase::Active, false))
    );
    let resized = Bounds { x: 80., ..bounds() };
    tree.layout(resized, &mut early);
    tree.render(&mut early).unwrap();
    let _ = tree.input(&mouse_event(25., 5.), &mut early);
    assert_eq!(
        root.trace.lock().unwrap().layouts.last(),
        Some(&vec![0, 20, 40])
    );
    assert_eq!(leaf.lock().unwrap().renders, 1);
    assert_eq!(leaf.lock().unwrap().inputs, 1);

    let mut due = ctx.reborrow().with_frame_time(now + DELAY);
    let result = tree.poll(&mut due);
    assert_eq!(
        result.next_poll,
        ControlFlow::Wait,
        "the removed grandchild's deadline must not survive"
    );
    assert!(result.update.contains(UpdateResult::LAYOUT));
    assert_eq!(departing.lock().unwrap().drops, 1);
    assert_eq!(leaf.lock().unwrap().drops, 1);
    assert_eq!(
        root.trace.lock().unwrap().polls.last(),
        Some(&(WidgetPhase::Active, true))
    );
    tree.layout(resized, &mut due);
    assert_eq!(
        root.trace.lock().unwrap().layouts.last(),
        Some(&vec![0, 40])
    );
}

#[test]
fn redeclaring_same_key_revives_original_widget_and_cancels_its_old_deadline() {
    let now = Instant::now();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry).with_frame_time(now);
    let mut tree = UiTree::new();
    let mut root = Item::new(0);
    let mut child = Item::new(1);
    child.retain = true;
    let item = child.trace.clone();
    root.children.push((1, child));
    tree.update(&root, &mut ctx);
    let (_, child) = root.children.pop().unwrap();
    tree.update(&root, &mut ctx);
    assert_eq!(
        tree.poll(&mut ctx).next_poll,
        ControlFlow::WaitUntil(now + DELAY)
    );
    root.children.push((20, child));
    tree.update(&root, &mut ctx);
    let mut past_exit = ctx.reborrow().with_frame_time(now + DELAY * 2);
    assert_eq!(tree.poll(&mut past_exit).next_poll, ControlFlow::Wait);
    tree.layout(bounds(), &mut past_exit);
    let _ = tree.input(&mouse_event(25., 5.), &mut past_exit);
    let item = item.lock().unwrap();
    assert_eq!((item.builds, item.drops, item.inputs), (1, 0, 1));
    assert_eq!(item.polls.last().unwrap().0, WidgetPhase::Active);
    assert_eq!(root.trace.lock().unwrap().layouts.last(), Some(&vec![20]));
}

#[test]
fn redeclaration_before_first_exit_poll_keeps_the_keyed_instance() {
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let mut tree = UiTree::new();
    let mut root = Item::new(0);
    let item = Item::new(1);
    let trace = item.trace.clone();
    root.children.push((10, item));
    tree.update(&root, &mut ctx);
    let (_, item) = root.children.pop().unwrap();
    tree.update(&root, &mut ctx);
    root.children.push((20, item));
    tree.update(&root, &mut ctx);
    let _ = tree.poll(&mut ctx);
    let trace = trace.lock().unwrap();
    assert_eq!((trace.builds, trace.drops), (1, 0));
    assert_eq!(trace.polls, [(WidgetPhase::Active, false)]);
}

#[test]
fn revival_then_second_omission_before_poll_restarts_the_exit_deadline() {
    let now = Instant::now();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry).with_frame_time(now);
    let mut tree = UiTree::new();
    let mut root = Item::new(0);
    let mut item = Item::new(1);
    item.retain = true;
    let trace = item.trace.clone();
    root.children.push((10, item));
    tree.update(&root, &mut ctx);
    let (_, item) = root.children.pop().unwrap();
    tree.update(&root, &mut ctx);
    assert_eq!(
        tree.poll(&mut ctx).next_poll,
        ControlFlow::WaitUntil(now + DELAY)
    );

    root.children.push((10, item));
    tree.update(&root, &mut ctx);
    root.children.clear();
    tree.update(&root, &mut ctx);
    let mut second = ctx.reborrow().with_frame_time(now + DELAY);
    assert_eq!(
        tree.poll(&mut second).next_poll,
        ControlFlow::WaitUntil(now + DELAY * 2)
    );
    assert_eq!(trace.lock().unwrap().drops, 0);

    let mut due = ctx.reborrow().with_frame_time(now + DELAY * 2);
    assert_eq!(tree.poll(&mut due).next_poll, ControlFlow::Wait);
    assert_eq!(trace.lock().unwrap().drops, 1);
}

#[test]
fn cancelling_pending_child_neither_requests_exit_nor_changes_active_children() {
    let now = Instant::now();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry).with_frame_time(now);
    let mut tree = UiTree::new();
    let mut root = Item::new(0);
    let mut child = Item::new(1);
    child.enter_at = Some(now + DELAY);
    child.retain = true;
    let item = child.trace.clone();
    root.children.push((0, child));
    tree.update(&root, &mut ctx);
    assert_eq!(
        tree.poll(&mut ctx).next_poll,
        ControlFlow::WaitUntil(now + DELAY)
    );
    root.children.clear();
    tree.update(&root, &mut ctx);
    assert_eq!(tree.poll(&mut ctx), PollSummary::default());
    assert_eq!(
        root.trace.lock().unwrap().polls.last(),
        Some(&(WidgetPhase::Active, false))
    );
    let item = item.lock().unwrap();
    assert_eq!(item.drops, 1);
    assert_eq!(item.polls, [(WidgetPhase::PendingEnter, false)]);
}

#[test]
fn finishing_parent_discards_its_retained_descendants_and_their_wake() {
    let now = Instant::now();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry).with_frame_time(now);
    let mut tree = UiTree::new();
    let mut root = Item::new(0);
    let mut parent = Item::new(1);
    let mut child = Item::new(2);
    child.retain = true;
    let trace = child.trace.clone();
    parent.children.push((0, child));
    root.children.push((0, parent));
    tree.update(&root, &mut ctx);

    root.children[0].1.children.clear();
    tree.update(&root, &mut ctx);
    assert_eq!(
        tree.poll(&mut ctx).next_poll,
        ControlFlow::WaitUntil(now + DELAY)
    );
    root.children.clear();
    tree.update(&root, &mut ctx);
    assert_eq!(trace.lock().unwrap().drops, 0);
    assert_eq!(tree.poll(&mut ctx).next_poll, ControlFlow::Wait);
    let trace = trace.lock().unwrap();
    assert_eq!(trace.drops, 1);
    assert_eq!(trace.polls, [(WidgetPhase::Exiting, false); 2]);
}

#[test]
fn reordering_siblings_keeps_the_retiring_link_before_its_old_successor() {
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);
    let mut tree = UiTree::new();
    let mut root = Item::new(0);
    let mut middle = Item::new(2);
    middle.retain = true;
    root.children = vec![(10, Item::new(1)), (20, middle), (30, Item::new(3))];
    tree.update(&root, &mut ctx);
    let _ = tree.poll(&mut ctx);

    root.children.remove(1);
    root.children.swap(0, 1);
    tree.update(&root, &mut ctx);
    let _ = tree.poll(&mut ctx);
    tree.layout(bounds(), &mut ctx);
    {
        let trace = root.trace.lock().unwrap();
        assert_eq!(trace.polls.last(), Some(&(WidgetPhase::Active, true)));
        assert_eq!(trace.layouts.last(), Some(&vec![20, 30, 10]));
    }

    // Changing just the surviving sibling's properties requires layout, not a membership notice.
    root.children[0].0 = 31;
    tree.update(&root, &mut ctx);
    let _ = tree.poll(&mut ctx);
    tree.layout(bounds(), &mut ctx);
    let trace = root.trace.lock().unwrap();
    assert_eq!(trace.polls.last(), Some(&(WidgetPhase::Active, false)));
    assert_eq!(trace.layouts.last(), Some(&vec![20, 31, 10]));
}

#[test]
fn replacing_window_with_declaration_host_does_not_leave_reused_content_waiting_for_poll() {
    let event_loop = matcha_window::headless_interface::HeadlessEventLoop::new();
    let mut open = super::windows::headless_factory(&event_loop);
    let mut windows = WindowRegistry::default();
    let mut root_ctx = Context::new(&mut windows);
    let mut ctx = root_ctx.for_window_declarations(&mut open);
    let mut tree = UiTree::new();
    let mut item = Item::new(1);
    item.enter_at = Some(ctx.frame_time() + DELAY);
    tree.update(&crate::ui_tree::window::Window::new(item), &mut ctx);
    let root = tree.root.unwrap();
    let child = child_at(&tree, root, 0);
    assert_eq!(
        tree.node_ref(child).unwrap().phase,
        WidgetPhase::PendingEnter
    );

    let mut host = Item::new(0);
    host.children.push((0, Item::new(1)));
    tree.update(&host, &mut ctx);
    assert!(ctx.window_registry().ids().is_empty());
    assert_eq!(child_at(&tree, root, 0), child);
    assert_eq!(tree.node_ref(child).unwrap().phase, WidgetPhase::Active);
}

#[test]
fn finishing_adjacent_exits_does_not_skip_the_next_sibling() {
    let now = Instant::now();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry).with_frame_time(now);
    let mut tree = UiTree::new();
    let mut root = Item::new(0);
    for key in 1..=3 {
        let mut child = Item::new(key);
        child.retain = true;
        root.children.push((key as u16, child));
    }
    let traces: Vec<_> = root.children.iter().map(|(_, i)| i.trace.clone()).collect();
    tree.update(&root, &mut ctx);
    root.children.drain(..2);
    tree.update(&root, &mut ctx);
    assert_eq!(
        tree.poll(&mut ctx).next_poll,
        ControlFlow::WaitUntil(now + DELAY)
    );

    let mut due = ctx.reborrow().with_frame_time(now + DELAY);
    assert_eq!(tree.poll(&mut due).next_poll, ControlFlow::Wait);
    for trace in &traces[..2] {
        let trace = trace.lock().unwrap();
        assert_eq!((trace.drops, trace.polls.len()), (1, 2));
    }
    assert_eq!(traces[2].lock().unwrap().polls.len(), 2);
    tree.layout(bounds(), &mut due);
    assert_eq!(root.trace.lock().unwrap().layouts.last(), Some(&vec![3]));
}
