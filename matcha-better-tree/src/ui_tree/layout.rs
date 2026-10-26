//! Measurement, relative placement, and layout cache management.
use crate::metrics::Position;

use super::*;

impl UiTree {
    /// Assign root-local bounds and lay out the tree.
    pub fn layout(&mut self, bounds: Bounds, ctx: &mut Context) {
        if let Some(root) = self.root {
            self.layout_node(
                root,
                Position {
                    bounds,
                    affine: Matrix4::identity(),
                },
                ctx,
            );
        }
    }

    pub(super) fn layout_node(&mut self, id: UiId, position: Position, ctx: &mut Context) {
        let Some(node) = self.node_ref_mut(id) else {
            return;
        };
        let mut scoped = node.scoped_context(ctx);
        let ctx = &mut scoped;
        if node
            .placement
            .is_none_or(|old| old.bounds != position.bounds)
        {
            node.cache.invalidate(UpdateResult::RENDER);
        }
        node.placement = Some(Placement::new(position, node.placement));

        // Movement changes placement but does not invalidate the children's local layout.
        if node.cache.laid_out(position.bounds) {
            return;
        }

        let widget = node
            .widget
            .take()
            .expect("writing back the widget into ui_node has been forgotten somewhere");
        widget.layout(
            position.bounds,
            Children {
                tree: self,
                current: id,
            },
            ctx,
        );
        let node = self
            .node_ref_mut(id)
            .expect("tree cannot be changed during layout");
        node.widget = Some(widget);
        node.cache.cache_layout(position.bounds);
    }
}

impl Children<'_> {
    pub fn measure(
        &mut self,
        id: ChildId,
        constraints: Constraints,
        ctx: &mut Context,
    ) -> Option<Bounds> {
        let (node, ui_id) = self.get(id)?;
        let mut scoped = node.scoped_context(ctx);
        let ctx = &mut scoped;

        if let Some(cached_bounds) = node.cache.measured(constraints) {
            return Some(cached_bounds);
        }

        let widget = std::mem::take(&mut node.widget)
            .expect("writing back the widget into ui_node has been forgotten somewhere");
        let bounds = {
            let children = Children {
                tree: self.tree,
                current: ui_id,
            };
            widget.measure(constraints, children, ctx)
        };
        let (node, _) = self
            .get(id)
            .expect("tree cannot be changed during this method");
        node.widget = Some(widget);
        node.cache.cache_measure(constraints, bounds);

        Some(bounds)
    }

    /// Place a child using bounds in its own coordinates and an affine mapping to this parent.
    pub fn layout(&mut self, id: ChildId, position: Position, ctx: &mut Context) {
        let Some((_, ui_id)) = self.get(id) else {
            return;
        };
        self.tree.layout_node(ui_id, position, ctx);
    }
}
