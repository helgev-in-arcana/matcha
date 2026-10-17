use std::sync::{Arc, Mutex};

use matcha_better_tree::UiTree;
use matcha_better_tree::context::Context;
use matcha_better_tree::metrics::{Bounds, Constraints, Position};
use matcha_better_tree::widget::{
    ChildId, Children, DeclContext, Draw, InputResult, UpdateResult, View, Widget,
};
use matcha_window::event::device_event::{
    DeviceEvent, DeviceEventData, DeviceEventState, MouseInput,
};
use nalgebra::{Matrix4, Vector3};

struct NodeView {
    key: u64,
    child: Option<Box<NodeView>>,
    input_log: Arc<Mutex<Vec<u64>>>,
    drop_log: Arc<Mutex<Vec<u64>>>,
}

struct NodeWidget {
    key: u64,
    child: Option<ChildId>,
    input_log: Arc<Mutex<Vec<u64>>>,
    drop_log: Arc<Mutex<Vec<u64>>>,
}

impl View for NodeView {
    fn label(&self) -> Option<&str> {
        None
    }
    fn key(&self) -> Option<u64> {
        Some(self.key)
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a>) -> impl Widget<V = Self> {
        NodeWidget {
            key: self.key,
            child: self.child.as_deref().map(|child| ctx.child(child)),
            input_log: self.input_log.clone(),
            drop_log: self.drop_log.clone(),
        }
    }
}

impl Drop for NodeWidget {
    fn drop(&mut self) {
        self.drop_log.lock().unwrap().push(self.key);
    }
}

impl Widget for NodeWidget {
    type V = NodeView;
    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a>) -> UpdateResult {
        self.child = view.child.as_deref().map(|child| ctx.child(child));
        UpdateResult::empty()
    }
    fn input(&mut self, _: Bounds, _: &DeviceEvent, _: Context) -> InputResult {
        self.input_log.lock().unwrap().push(self.key);
        InputResult::default()
    }
    fn measure(&self, _: Constraints, _: Children<'_>, _: Context) -> Bounds {
        Bounds {
            x: 10.0,
            y: 10.0,
            z: 0.0,
        }
    }
    fn layout(&self, _: Bounds, mut children: Children<'_>, ctx: Context) {
        if let Some(child) = self.child {
            let bounds = children
                .measure(child, Constraints::default(), ctx.clone())
                .unwrap();
            children.layout(
                child,
                Position {
                    bounds,
                    affine: Matrix4::new_translation(&Vector3::new(20.0, 0.0, 0.0)),
                },
                ctx,
            );
        }
    }
    fn render(&self, _: Bounds, _: Context) -> Draw {
        Draw {
            color: Default::default(),
            mask: None,
        }
    }
}

#[test]
fn public_tree_uses_opaque_contexts_and_rebuilds_after_clear() {
    let input_log = Arc::new(Mutex::new(Vec::new()));
    let drop_log = Arc::new(Mutex::new(Vec::new()));
    let mut view = NodeView {
        key: 1,
        child: Some(Box::new(NodeView {
            key: 2,
            child: None,
            input_log: input_log.clone(),
            drop_log: drop_log.clone(),
        })),
        input_log: input_log.clone(),
        drop_log: drop_log.clone(),
    };
    let mut tree = UiTree::new();
    tree.update(&view);
    tree.update(&view);
    tree.layout(
        Bounds {
            x: 10.0,
            y: 10.0,
            z: 0.0,
        },
        Context::new(),
    );
    let event = DeviceEventState::default()
        .process(DeviceEvent::stateless(DeviceEventData::MouseInput {
            dragging_from_primary: None,
            dragging_from_secondary: None,
            dragging_from_middle: None,
            event: Some(MouseInput::Moved {
                position: [25.0, 5.0],
            }),
        }))
        .unwrap();
    assert_eq!(tree.input(&event, Context::new()), InputResult::default());
    assert_eq!(*input_log.lock().unwrap(), vec![2, 1]);
    assert!(drop_log.lock().unwrap().is_empty());
    tree.clear();
    assert_eq!(*drop_log.lock().unwrap(), vec![2, 1]);
    input_log.lock().unwrap().clear();
    assert_eq!(tree.input(&event, Context::new()), InputResult::default());
    assert!(input_log.lock().unwrap().is_empty());
    tree.update(&view);
    tree.layout(
        Bounds {
            x: 10.0,
            y: 10.0,
            z: 0.0,
        },
        Context::new(),
    );
    assert_eq!(tree.input(&event, Context::new()), InputResult::default());
    assert_eq!(*input_log.lock().unwrap(), vec![2, 1]);
    view.child = None;
    tree.update(&view);
    assert_eq!(*drop_log.lock().unwrap(), vec![2, 1, 2]);
    tree.clear();
    tree.clear();
    assert_eq!(*drop_log.lock().unwrap(), vec![2, 1, 2, 1]);
}
