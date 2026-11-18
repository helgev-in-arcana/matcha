//! Mutable widget work before measurement, layout, and rendering.
use super::*;

impl UiTree {
    /// Poll the ordinary-widget tree once, children before parents, even when caches are valid.
    /// Native Window nodes and their content are handled separately by `poll_window`.
    /// Call this before measurement/layout/rendering and use the same Context frame time
    /// for those passes. The returned timing replaces the previous full-pass request.
    pub fn poll(&mut self, ctx: &mut Context) -> PollResult {
        let Some(root) = self.root else {
            return PollResult::default();
        };
        self.poll_from(root, ctx)
    }

    pub(super) fn poll_from(&mut self, root: UiId, ctx: &mut Context) -> PollResult {
        let result = self.poll_node(root, &mut ctx.unobserved());
        if result.update.contains(UpdateResult::LAYOUT) {
            // Descendants have already propagated to this root. Complete its boundary once.
            self.invalidate_node(root, UpdateResult::LAYOUT);
        }
        result
    }

    fn poll_node(&mut self, id: UiId, ctx: &mut Context) -> PollResult {
        let node = self.node_ref(id).expect("live poll tree");
        if node.body().is_window() {
            return PollResult::default();
        }
        let child_count = node.body().as_widget().child_count();
        let mut result = PollResult::default();
        for index in 0..child_count {
            let child = self
                .node_ref(id)
                .expect("poll cannot remove nodes")
                .body()
                .as_widget()
                .child_id(index);
            let child_result = self.poll_node(child, ctx);
            result.update |= child_result.update;
            result.next_poll = result.next_poll.merge(child_result.next_poll);
        }
        let node = self.node_ref_mut(id).expect("poll cannot remove nodes");
        let local = node
            .body
            .as_mut()
            .expect("poll keeps mounted bodies in the tree")
            .as_widget_mut()
            .poll(ctx);
        // Only layout changes propagate from children. Drawing remains widget-local.
        node.invalidate_caches(local.update | (result.update & UpdateResult::LAYOUT));
        result.update |= local.update;
        result.next_poll = result.next_poll.merge(local.next_poll);
        result
    }
}
