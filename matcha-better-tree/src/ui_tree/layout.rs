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

    pub(super) fn layout_node(
        &mut self,
        id: UiId,
        position: Position,
        ctx: &mut Context,
    ) -> EnvReads {
        let Some(node) = self.node_ref_mut(id) else {
            return EnvReads::empty();
        };
        // Direct roots (including a Window's content) have no Children iterator to filter them.
        if node.phase == WidgetPhase::PendingEnter {
            return EnvReads::empty();
        }
        if node
            .placement
            .is_none_or(|old| old.bounds != position.bounds)
        {
            node.render_cache.invalidate();
        }
        node.placement = Some(Placement::new(position, node.placement));

        // Movement changes placement but does not invalidate the children's local layout.
        let key = (position.bounds, ctx.geometry_environment());
        if let Some(&dependencies) = node
            .layout_cache
            .get_if(|(bounds, env), reads| *bounds == key.0 && env.matches(&key.1, *reads))
        {
            return dependencies;
        }

        let body = node.body.take().expect("live node during layout");
        let (child_reads, reads) =
            ctx.observe_env_reads(|ctx| body.as_widget().layout(position.bounds, self, ctx));
        let node = self
            .node_ref_mut(id)
            .expect("tree cannot be changed during layout");
        node.body = Some(body);
        node.env_reads.record(ReadPhase::Layout, reads);
        let dependencies = reads | child_reads;
        node.layout_cache.store(key, dependencies);
        dependencies
    }

    pub(super) fn measure_node(
        &mut self,
        id: UiId,
        constraints: Constraints,
        ctx: &mut Context,
    ) -> (Bounds, ChildGeometry) {
        let node = self.node_ref_mut(id).expect("live measured node");
        let key = (constraints, ctx.geometry_environment());
        if let Some(&cached) = node.measure_cache.get_if(|(constraints, env), (_, reads)| {
            *constraints == key.0 && env.matches(&key.1, *reads)
        }) {
            let (bounds, dependencies) = cached;
            return (
                bounds,
                ChildGeometry {
                    dependencies,
                    placed: false,
                },
            );
        }

        let body = node.body.take().expect("live node during measurement");
        let ((bounds, mut geometry), reads) =
            ctx.observe_env_reads(|ctx| body.as_widget().measure(constraints, self, ctx));
        let node = self
            .node_ref_mut(id)
            .expect("tree cannot be changed during measurement");
        node.body = Some(body);
        node.env_reads.record(ReadPhase::Measure, reads);
        geometry.dependencies |= reads;
        if geometry.placed {
            // Measurement wrote descendant placement. After a metric change the parent can
            // replay layout and measurement in a different order, or omit either call.
            let metrics = EnvReads::DPI | EnvReads::VIEWPORT_SIZE;
            geometry.dependencies |= metrics;
            if let Some(layout_dependencies) = node.layout_cache.value_mut() {
                *layout_dependencies |= metrics;
            }
        }
        node.measure_cache
            .store(key, (bounds, geometry.dependencies));
        (bounds, geometry)
    }
}

impl Child<'_> {
    pub fn measure(&mut self, constraints: Constraints, ctx: &mut Context) -> Bounds {
        let (bounds, geometry) =
            ctx.with_child_geometry(|ctx| self.tree.measure_node(self.id, constraints, ctx));
        self.geometry.dependencies |= geometry.dependencies;
        self.geometry.placed |= geometry.placed;
        bounds
    }

    /// Place the child using bounds in its own coordinates and an affine mapping to its parent.
    pub fn layout(&mut self, position: Position, ctx: &mut Context) {
        self.geometry.dependencies |=
            ctx.with_child_geometry(|ctx| self.tree.layout_node(self.id, position, ctx));
        self.geometry.placed = true;
    }
}
