//! Measurement, relative placement, and layout cache management.
use crate::metrics::Position;

use super::*;

impl UiTree {
    /// Assign root-local bounds and lay out the tree.
    pub fn layout(&mut self, bounds: Bounds, ctx: Context) {
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

    fn layout_node(&mut self, id: UiId, position: Position, ctx: Context) {
        let epoch = self.cache_epochs.layout;
        let Some(node) = self.node_ref_mut(id) else {
            return;
        };
        node.sync_layout_epoch(epoch);
        if node
            .placement
            .is_none_or(|old| old.bounds != position.bounds)
        {
            node.draw_cache = None;
        }
        let affine_inverse = match node.placement {
            Some(placement) if placement.affine == position.affine => placement.affine_inverse,
            _ => position.affine.try_inverse(),
        };
        node.placement = Some(Placement {
            bounds: position.bounds,
            affine: position.affine,
            affine_inverse,
        });

        // Movement changes placement but does not invalidate the children's local layout.
        if node.layout_cache == Some(position.bounds) {
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
        node.layout_cache = Some(position.bounds);
    }
}

impl Children<'_> {
    pub fn measure(
        &mut self,
        id: ChildId,
        constraints: Constraints,
        ctx: Context,
    ) -> Option<Bounds> {
        let epoch = self.tree.cache_epochs.layout;
        let (node, ui_id) = self.get(id)?;
        node.sync_layout_epoch(epoch);

        if let Some((cached_constraints, cached_bounds)) = node.measure_cache
            && cached_constraints == constraints
        {
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
        node.measure_cache = Some((constraints, bounds));

        Some(bounds)
    }

    /// Place a child using bounds in its own coordinates and an affine mapping to this parent.
    pub fn layout(&mut self, id: ChildId, position: Position, ctx: Context) {
        let Some((_, ui_id)) = self.get(id) else {
            return;
        };
        self.tree.layout_node(ui_id, position, ctx);
    }
}
