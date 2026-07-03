use std::sync::Arc;

use bevy_ecs::component::Component;
use parking_lot::Mutex;
use renderer::RenderNode;

#[derive(Component)]
pub struct IfOverwritten {
    pub flag: bool,
}

pub struct Object {
    pub node: Arc<Mutex<Option<RenderNode>>>,
    pub logic: Box<dyn FnOnce() -> RenderNode + Send + Sync>,
}

#[derive(Component)]
pub struct RenderItem {
    pub objects: Vec<Object>,
}
