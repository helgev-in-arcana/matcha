pub mod column;
pub mod grid;
pub mod padding;
pub mod position;
pub mod row;
pub mod space;
pub mod visibility;

use matcha_tree::ui_tree::{
    context::UiContext,
    widget::{View, WidgetInteractionResult, WidgetPod},
};

/// Reconcile an optional single child pod against an optional new view.
/// Returns true if the child changed (layout needed).
pub(crate) fn reconcile_single_child(
    child: &mut Option<WidgetPod>,
    view: Option<&dyn View>,
    ctx: &UiContext,
) -> bool {
    match (child.as_mut(), view) {
        (Some(pod), Some(v)) => match pod.try_update(v, ctx) {
            Ok(WidgetInteractionResult::LayoutNeeded) => true,
            Ok(_) => false,
            Err(_) => {
                *child = Some(v.build(ctx));
                true
            }
        },
        (None, Some(v)) => {
            *child = Some(v.build(ctx));
            true
        }
        (Some(_), None) => {
            *child = None;
            true
        }
        (None, None) => false,
    }
}

/// Positional reconciliation for an ordered list of children.
/// Returns LayoutNeeded if anything changed.
pub(crate) fn update_children(
    children: &mut Vec<WidgetPod>,
    views: &[Box<dyn View>],
    ctx: &UiContext,
) -> WidgetInteractionResult {
    let mut layout_needed = children.len() != views.len();

    let existing = children.len().min(views.len());

    for i in 0..existing {
        match children[i].try_update(views[i].as_ref(), ctx) {
            Ok(WidgetInteractionResult::LayoutNeeded) => layout_needed = true,
            Ok(_) => {}
            Err(_) => {
                children[i] = views[i].build(ctx);
                layout_needed = true;
            }
        }
    }

    children.truncate(views.len());

    for view in views.iter().skip(existing) {
        children.push(view.build(ctx));
        layout_needed = true;
    }

    if layout_needed {
        WidgetInteractionResult::LayoutNeeded
    } else {
        WidgetInteractionResult::NoChange
    }
}
