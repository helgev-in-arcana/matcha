use matcha_window::event::device_event::DeviceEventData;

use super::*;

impl UiTree {
    /// Dispatch a viewport-coordinate mouse event through the hit target's ancestor path.
    /// Layout must be current. Focus/capture and non-mouse delivery are separate routes.
    pub fn input(&mut self, event: &DeviceEvent, ctx: &mut Context) -> InputResult {
        let Some(root) = self.root else {
            return InputResult::default();
        };
        self.input_from(root, event, ctx)
    }

    pub(super) fn input_from(
        &mut self,
        root: UiId,
        event: &DeviceEvent,
        ctx: &mut Context,
    ) -> InputResult {
        if !matches!(event.event(), DeviceEventData::MouseInput { .. }) {
            return InputResult::default();
        }

        let Some([x, y]) = event.mouse_position() else {
            return InputResult::default();
        };
        let Some(target) = self.hit_test(root, Point { x, y, z: 0.0 }, ctx) else {
            return InputResult::default();
        };

        // Snapshot the selected path and each node's local event before any callback runs.
        let mut ids = Vec::new();
        let mut current = Some(target);
        while let Some(id) = current {
            let node = self
                .node_ref(id)
                .expect("hit target and ancestors are live");
            ids.push(id);
            current = if id == root { None } else { node.parent };
        }
        ids.reverse();
        let mut local_event = event.clone();
        let path: Vec<_> = ids
            .into_iter()
            .map(|id| {
                let placement = self
                    .node_ref(id)
                    .expect("hit path contains live nodes")
                    .placement
                    .expect("hit path contains placed nodes");
                local_event = local_event.transform(placement.affine);
                (id, placement.bounds, local_event.clone())
            })
            .collect();

        let mut result = InputResult::default();
        let mut deepest_changed: Option<usize> = None;
        let preview = (0..path.len()).map(|index| (index, true));
        let bubble = (0..path.len()).rev().map(|index| (index, false));
        for (index, is_preview) in preview.chain(bubble) {
            let (id, bounds, event) = &path[index];
            let mut scoped = ctx.unobserved();
            let widget = self
                .node_ref_mut(*id)
                .expect("input callbacks cannot remove nodes")
                .body
                .as_mut()
                .expect("input callbacks cannot remove widgets from the tree")
                .as_widget_mut();
            let response = if is_preview {
                widget.preview_input(*bounds, event, &mut scoped)
            } else {
                widget.input(*bounds, event, &mut scoped)
            };
            result.update |= response.update;
            if response.update.contains(UpdateResult::RENDER) {
                self.invalidate_node(*id, UpdateResult::RENDER);
            }
            if response.update.contains(UpdateResult::LAYOUT) {
                deepest_changed = Some(deepest_changed.map_or(index, |old| old.max(index)));
            }
            if response.propagation == InputPropagation::Stop {
                result.propagation = InputPropagation::Stop;
                break;
            }
        }

        // All layout requests lie on the hit path, so the deepest one covers them all,
        // including ancestors whose input callback was skipped by Stop.
        if let Some(last) = deepest_changed {
            self.invalidate_node(path[last].0, UpdateResult::LAYOUT);
        }
        result
    }

    /// Reverse paint order: last declared child's subtree first, then this widget itself.
    /// Assumes parent content is behind children. There is no implicit clipping to parent bounds;
    /// explicit render-mask/clip policy will need to be shared with rendering when it is added.
    pub(super) fn hit_test(&self, id: UiId, point: Point, ctx: &mut Context) -> Option<UiId> {
        let node = self.node_ref(id)?;
        // Retiring widgets still paint, but the entire subtree stops receiving new input.
        if node.phase != WidgetPhase::Active {
            return None;
        }
        let mut scoped = ctx.unobserved();
        let ctx = &mut scoped;
        let NodeBody::Widget(widget) = node.body() else {
            return None;
        };
        let placement = node.placement?;
        let point = placement.local_point(point)?;

        for child in node.child_ids().rev() {
            if let Some(target) = self.hit_test(child, point, ctx) {
                return Some(target);
            }
        }
        widget.is_inside(placement.bounds, point, ctx).then_some(id)
    }
}
