//! Mutable widget work before measurement, layout, and rendering.
use super::widget::ControlFlow;
use super::*;

/// Cache invalidation and wake requests remaining after a full tree or Window poll.
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub struct PollSummary {
    pub update: UpdateResult,
    pub next_poll: ControlFlow,
}

impl Default for PollSummary {
    fn default() -> Self {
        Self {
            update: UpdateResult::empty(),
            next_poll: ControlFlow::Wait,
        }
    }
}

impl UiTree {
    /// Poll the ordinary-widget tree once, children before parents, even when caches are valid.
    /// Native Window nodes and their content are handled separately by `poll_window`.
    /// Call this before measurement/layout/rendering and use the same Context frame time
    /// for those passes. The returned timing replaces the previous full-pass request.
    pub fn poll(&mut self, ctx: &mut Context) -> PollSummary {
        let Some(root) = self.root else {
            return PollSummary::default();
        };
        self.poll_from(root, ctx)
    }

    pub(super) fn poll_from(&mut self, root: UiId, ctx: &mut Context) -> PollSummary {
        // The tree root (or a window's single content root) is always declared.
        let result = match self.poll_node(root, &mut ctx.unobserved()) {
            Lifecycle::Keep { update, next_poll } | Lifecycle::Enter { update, next_poll } => {
                PollSummary { update, next_poll }
            }
            Lifecycle::FinishExit => unreachable!("poll roots cannot be retiring children"),
        };
        if result.update.contains(UpdateResult::LAYOUT) {
            // Descendants have already propagated to this root. Complete its boundary once.
            self.invalidate_node(root, UpdateResult::LAYOUT);
        }
        result
    }

    // Return the transition of this node alone, carrying aggregate work for surviving children.
    fn poll_node(&mut self, id: UiId, ctx: &mut Context) -> Lifecycle {
        let node = self.node_ref_mut(id).expect("live poll tree");
        if node.body().is_window() {
            return Lifecycle::default();
        }
        let mut body = node.body.take().expect("live widget during poll");
        let mut membership_changed = false;
        let mut update = UpdateResult::empty();
        let mut next_poll = ControlFlow::Wait;
        body.as_widget_mut().retain_children(&mut |child| {
            let result = self.poll_node(child, ctx);
            membership_changed |= !matches!(result, Lifecycle::Keep { .. });
            match result {
                Lifecycle::Keep {
                    update: child_update,
                    next_poll: child_poll,
                }
                | Lifecycle::Enter {
                    update: child_update,
                    next_poll: child_poll,
                } => {
                    update |= child_update;
                    next_poll = next_poll.merge(child_poll);
                    true
                }
                Lifecycle::FinishExit => {
                    self.remove_reverse_scan_order(child, ctx);
                    false
                }
            }
        });

        let node = self
            .node_ref_mut(id)
            .expect("children cannot remove their parent");
        let info = PollInfo {
            phase: node.phase,
            active_children_changed: std::mem::take(&mut node.active_children_changed)
                || membership_changed,
        };
        let response = body.as_widget_mut().poll(info, ctx);
        node.body = Some(body);

        let (local_update, local_poll, entered) = match response {
            Lifecycle::Keep { update, next_poll } => (update, next_poll, false),
            Lifecycle::Enter { update, next_poll } => {
                (update, next_poll, node.phase == WidgetPhase::PendingEnter)
            }
            Lifecycle::FinishExit if node.phase == WidgetPhase::Exiting => {
                return Lifecycle::FinishExit;
            }
            Lifecycle::FinishExit => (UpdateResult::empty(), ControlFlow::Wait, false),
        };
        if entered {
            node.phase = WidgetPhase::Active;
        }
        if membership_changed || entered {
            update |= UpdateResult::LAYOUT;
        }
        // Hidden widgets can invalidate local caches and wake the Window. Their work
        // doesn't affect the visible layout or request a redraw until they participate.
        node.invalidate_caches(local_update | (update & UpdateResult::LAYOUT));
        update |= local_update;
        next_poll = next_poll.merge(local_poll);
        if node.phase == WidgetPhase::PendingEnter {
            update = UpdateResult::empty();
        }
        if entered {
            Lifecycle::Enter { update, next_poll }
        } else {
            Lifecycle::Keep { update, next_poll }
        }
    }
}
