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
            node.render_cache.invalidate();
        }
        node.placement = Some(Placement::new(position, node.placement));

        // Movement changes placement but does not invalidate the children's local layout.
        if node.layout_cache.get(&position.bounds).is_some() {
            return;
        }

        let widget = node
            .widget
            .take()
            .expect("writing back the widget into ui_node has been forgotten somewhere");
        let ((), reads) = ctx.observe_env_reads(|ctx| widget.layout(position.bounds, self, ctx));
        let node = self
            .node_ref_mut(id)
            .expect("tree cannot be changed during layout");
        node.widget = Some(widget);
        node.env_reads.record(ReadPhase::Layout, reads);
        node.layout_cache.store(position.bounds, ());
    }

    pub(super) fn measure_node(
        &mut self,
        id: UiId,
        constraints: Constraints,
        ctx: &mut Context,
    ) -> Bounds {
        let node = self.node_ref_mut(id).expect("live measured node");
        let mut scoped = node.scoped_context(ctx);
        let ctx = &mut scoped;

        if let Some(&cached_bounds) = node.measure_cache.get(&constraints) {
            return cached_bounds;
        }

        let widget = std::mem::take(&mut node.widget)
            .expect("writing back the widget into ui_node has been forgotten somewhere");
        let (bounds, reads) = ctx.observe_env_reads(|ctx| widget.measure(constraints, self, ctx));
        let node = self
            .node_ref_mut(id)
            .expect("tree cannot be changed during measurement");
        node.widget = Some(widget);
        node.env_reads.record(ReadPhase::Measure, reads);
        node.measure_cache.store(constraints, bounds);

        bounds
    }
}

impl Child<'_> {
    pub fn measure(&mut self, constraints: Constraints, ctx: &mut Context) -> Bounds {
        self.tree.measure_node(self.id, constraints, ctx)
    }

    /// Place the child using bounds in its own coordinates and an affine mapping to its parent.
    pub fn layout(&mut self, position: Position, ctx: &mut Context) {
        self.tree.layout_node(self.id, position, ctx);
    }
}
