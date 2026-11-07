use super::*;
use crate::ui_tree::window::WindowRegistry;

/// Each declaration creates a fresh value. Equal settings should preserve warmed caches.
trait Allocation: PartialEq + 'static {
    fn from_label(label: &str) -> Self;
    fn width(&self, measured: f32) -> f32;
}

#[derive(PartialEq)]
struct ExtraWidth(String);

impl Allocation for ExtraWidth {
    fn from_label(label: &str) -> Self {
        Self(label.to_owned())
    }

    fn width(&self, measured: f32) -> f32 {
        measured + self.0.len() as f32
    }
}

#[derive(PartialEq)]
struct FixedWidth(u16);

impl Allocation for FixedWidth {
    fn from_label(label: &str) -> Self {
        Self(label.parse().unwrap())
    }

    fn width(&self, _: f32) -> f32 {
        f32::from(self.0)
    }
}

#[derive(Default)]
struct Measurements {
    builds: usize,
    measures: usize,
    layouts: Vec<f32>,
    renders: usize,
}

struct Leaf {
    key: u64,
    natural_width: f32,
    log: Arc<Mutex<Measurements>>,
}

struct LeafWidget {
    width: f32,
    log: Arc<Mutex<Measurements>>,
}

impl View for Leaf {
    type ChildData = ();

    fn label(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> Option<u64> {
        Some(self.key)
    }

    fn build<'a>(&'a self, _: &mut DeclContext<'a, '_>) -> impl Widget<V = Self> {
        self.log.lock().unwrap().builds += 1;
        LeafWidget {
            width: self.natural_width,
            log: self.log.clone(),
        }
    }
}

impl Widget for LeafWidget {
    type V = Leaf;

    fn update<'a>(&mut self, _: &'a Leaf, _: &mut DeclContext<'a, '_>) -> UpdateResult {
        UpdateResult::empty()
    }

    fn measure(&self, _: Constraints, _: Children<'_>, _: &mut Context) -> Bounds {
        self.log.lock().unwrap().measures += 1;
        Bounds {
            x: self.width,
            ..bounds()
        }
    }

    fn layout(&self, assigned: Bounds, _: Children<'_>, _: &mut Context) {
        self.log.lock().unwrap().layouts.push(assigned.x);
    }

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        self.log.lock().unwrap().renders += 1;
        Draw::default()
    }

    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }
}

#[derive(Default)]
struct RowLog {
    measures: usize,
    renders: usize,
    // (natural width, allocated width) pairs as seen on each completed layout.
    layouts: Vec<Vec<(f32, f32)>>,
}

struct Row<V, P> {
    slots: Vec<(&'static str, V)>,
    log: Arc<Mutex<RowLog>>,
    allocation: PhantomData<P>,
}

impl<V, P> Row<V, P> {
    fn new(slots: Vec<(&'static str, V)>) -> Self {
        Self {
            slots,
            log: Arc::default(),
            allocation: PhantomData,
        }
    }
}

impl<V: View, P: Allocation> Row<V, P> {
    fn declare<'a>(&'a self, ctx: &mut DeclContext<'a, '_, P>) {
        for (label, child) in &self.slots {
            ctx.child_with(child, P::from_label(label));
        }
    }
}

struct RowWidget<V, P> {
    log: Arc<Mutex<RowLog>>,
    view_type: PhantomData<fn() -> Row<V, P>>,
}

impl<V: View, P: Allocation> View for Row<V, P> {
    type ChildData = P;

    fn label(&self) -> Option<&str> {
        None
    }

    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_, P>) -> impl Widget<V = Self> {
        self.declare(ctx);
        RowWidget::<V, P> {
            log: self.log.clone(),
            view_type: PhantomData,
        }
    }
}

impl<V: View, P: Allocation> Widget for RowWidget<V, P> {
    type V = Row<V, P>;

    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a, '_, P>) -> UpdateResult {
        view.declare(ctx);
        UpdateResult::empty()
    }

    fn measure(
        &self,
        constraints: Constraints,
        mut children: Children<'_, P>,
        ctx: &mut Context,
    ) -> Bounds {
        self.log.lock().unwrap().measures += 1;
        let mut width = 0.0;
        children.for_each(|allocation, mut child| {
            width += allocation.width(child.measure(constraints, ctx).x);
        });
        Bounds {
            x: width,
            ..bounds()
        }
    }

    fn layout(&self, _: Bounds, mut children: Children<'_, P>, ctx: &mut Context) {
        let mut widths = Vec::new();
        children.for_each(|_, mut child| widths.push(child.measure(Constraints::default(), ctx).x));
        let mut widths = widths.into_iter();
        let mut x = 0.0;
        let mut assigned = Vec::new();
        children.for_each(|allocation, mut child| {
            let measured = widths.next().unwrap();
            let width = allocation.width(measured);
            child.layout(
                Position {
                    bounds: Bounds {
                        x: width,
                        ..bounds()
                    },
                    affine: translation(x, 0.0, 0.0),
                },
                ctx,
            );
            x += width;
            assigned.push((measured, width));
        });
        assert!(widths.next().is_none());
        self.log.lock().unwrap().layouts.push(assigned);
    }

    fn render(&self, _: Bounds, _: &mut Context) -> Draw {
        self.log.lock().unwrap().renders += 1;
        Draw::default()
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: &mut Context) -> InputResult {
        InputResult::default()
    }
}

#[test]
fn property_comparison_controls_parent_layout_and_keeps_keyed_settings_with_their_children() {
    let a = Arc::default();
    let b = Arc::default();
    let row = Row::<_, ExtraWidth>::new(vec![
        (
            "aa",
            Leaf {
                key: 1,
                natural_width: 10.0,
                log: Arc::clone(&a),
            },
        ),
        (
            "bbbbb",
            Leaf {
                key: 2,
                natural_width: 20.0,
                log: Arc::clone(&b),
            },
        ),
    ]);
    let row_log = row.log.clone();
    // Even when its measured size changes, the row still receives width 100.
    let mut root = Row::<_, FixedWidth>::new(vec![("100", row)]);
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);

    tree.update(&root, &mut ctx);
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    tree.update(&root, &mut ctx);
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    assert_eq!(
        root.log.lock().unwrap().layouts.as_slice(),
        &[vec![(37.0, 100.0)]]
    );
    assert_eq!(row_log.lock().unwrap().measures, 1);
    assert_eq!(
        row_log.lock().unwrap().layouts.as_slice(),
        &[vec![(10.0, 12.0), (20.0, 25.0)]]
    );

    root.slots[0].1.slots[0].0 = "aaaa";
    tree.update(&root, &mut ctx);
    assert!(matches!(
        tree.render(&mut ctx),
        Err(UiError::LayoutRequired)
    ));
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    assert_eq!(root.log.lock().unwrap().layouts[1], vec![(39.0, 100.0)]);
    assert_eq!(row_log.lock().unwrap().measures, 2);
    assert_eq!(row_log.lock().unwrap().renders, 1);
    assert_eq!(root.log.lock().unwrap().renders, 1);
    assert_eq!(
        row_log.lock().unwrap().layouts[1],
        vec![(10.0, 14.0), (20.0, 25.0)]
    );
    assert_eq!(a.lock().unwrap().measures, 1);
    assert_eq!(a.lock().unwrap().layouts, vec![12.0, 14.0]);
    assert_eq!(a.lock().unwrap().renders, 2);
    assert_eq!(b.lock().unwrap().measures, 1);
    assert_eq!(b.lock().unwrap().layouts, vec![25.0]);
    assert_eq!(b.lock().unwrap().renders, 1);

    root.slots[0].1.slots.swap(0, 1);
    tree.update(&root, &mut ctx);
    tree.layout(bounds(), &mut ctx);
    tree.render(&mut ctx).unwrap();
    assert_eq!(
        row_log.lock().unwrap().layouts[2],
        vec![(20.0, 25.0), (10.0, 14.0)]
    );
    assert_eq!(a.lock().unwrap().builds, 1);
    assert_eq!(b.lock().unwrap().builds, 1);
    assert_eq!(a.lock().unwrap().layouts, vec![12.0, 14.0]);
    assert_eq!(b.lock().unwrap().layouts, vec![25.0]);
}

#[test]
fn replacing_parent_type_redeclares_the_properties_of_a_retained_child() {
    let log = Arc::default();
    let leaf = || Leaf {
        key: 1,
        natural_width: 10.0,
        log: Arc::clone(&log),
    };
    let old = Row::<_, ExtraWidth>::new(vec![("xx", leaf())]);
    let new = Row::<_, FixedWidth>::new(vec![("51", leaf())]);
    let mut tree = UiTree::new();
    let mut registry = WindowRegistry::default();
    let mut ctx = Context::new(&mut registry);

    tree.update(&old, &mut ctx);
    tree.layout(bounds(), &mut ctx);
    tree.update(&new, &mut ctx);
    tree.layout(bounds(), &mut ctx);
    let log = log.lock().unwrap();
    assert_eq!(log.builds, 1);
    assert_eq!(log.layouts, vec![12.0, 51.0]);
}
