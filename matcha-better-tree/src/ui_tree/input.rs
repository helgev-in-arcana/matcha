use matcha_window::event::device_event::DeviceEventData;

use super::*;

impl UiTree {
    /// Dispatch a viewport-coordinate mouse event through the hit target's ancestor path.
    /// Layout must be current. Focus/capture and non-mouse delivery are separate routes.
    pub fn input(&mut self, event: &DeviceEvent, ctx: Context) -> InputResult {
        if !matches!(event.event(), DeviceEventData::MouseInput { .. }) {
            return InputResult::default();
        }
        let Some(root) = self.root else {
            return InputResult::default();
        };
        let Some([x, y]) = event.mouse_position() else {
            return InputResult::default();
        };
        let Some(target) = self.hit_test(root, Point { x, y, z: 0.0 }, &ctx) else {
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
            current = node.parent;
        }
        ids.reverse();
        let mut local_event = event.clone();
        let path: Vec<_> = ids
            .into_iter()
            .map(|id| {
                let placement = self.node_ref(id).unwrap().placement.unwrap();
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
            let widget = self
                .node_ref_mut(*id)
                .unwrap()
                .widget
                .as_mut()
                .expect("input callbacks cannot remove widgets from the tree");
            let response = if is_preview {
                widget.preview_input(*bounds, event, ctx.clone())
            } else {
                widget.input(*bounds, event, ctx.clone())
            };
            result.update |= response.update;
            if response.update.contains(UpdateResult::RENDER) {
                self.node_ref_mut(*id).unwrap().draw_cache = None;
            }
            if response.update.contains(UpdateResult::LAYOUT) {
                deepest_changed = Some(deepest_changed.map_or(index, |old| old.max(index)));
            }
            if response.propagation == InputPropagation::Stop {
                result.propagation = InputPropagation::Stop;
                break;
            }
        }

        // All changed nodes lie on one path. Invalidate its prefix once, including ancestors
        // whose normal input callback was skipped by Stop. Keep placement and other subtrees.
        if let Some(last) = deepest_changed {
            for (id, _, _) in &path[..=last] {
                let node = self.node_ref_mut(*id).unwrap();
                node.measure_cache = None;
                node.layout_cache = None;
            }
        }
        result
    }

    /// Reverse paint order: last declared child's subtree first, then this widget itself.
    /// Assumes parent content is behind children. There is no implicit clipping to parent bounds;
    /// explicit render-mask/clip policy will need to be shared with rendering when it is added.
    pub(super) fn hit_test(&self, id: UiId, point: Point, ctx: &Context) -> Option<UiId> {
        let node = self.node_ref(id)?;
        let placement = node.placement?;
        let local = placement.affine_inverse? * Vector4::new(point.x, point.y, point.z, 1.0);
        let point = Point {
            x: local.x,
            y: local.y,
            z: local.z,
        };
        if ![point.x, point.y, point.z].iter().all(|v| v.is_finite()) {
            return None;
        }

        for (_, child) in node.children.iter().rev() {
            if let Some(target) = self.hit_test(*child, point, ctx) {
                return Some(target);
            }
        }
        node.widget
            .as_ref()?
            .is_inside(placement.bounds, point, ctx.clone())
            .then_some(id)
    }
}
