//! Headless verification of clipping's world-facing half: which clips a frame
//! declares, how they nest, which one each drawable item ends up inside, and
//! that a clipped-away widget stops being pickable.
//!
//! GPU-free, per this suite's convention — `RenderItem::builder` is never
//! invoked. What the clip *looks like* is the renderer's business and is
//! covered on its side; what is asserted here is the shape of the arena the
//! renderer is handed.

use bevy_ecs::{entity::Entity, world::World};
use matcha_ecs::{
    components::view::ViewChildren,
    layout::{layout_root, Constraints},
    pick::{PickQuery, Picker, RectZPicker},
    render::extract_items,
    view::run_view,
};
use matcha_ecs_widgets::{Button, ColorRect, Column, Panel};

fn laid_out(view: impl Fn(&mut matcha_ecs::view::Scope)) -> (World, Entity) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| view(s));
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));
    (world, root)
}

/// `[min_x, min_y, max_x, max_y]` of a clip rect, from its unit-quad transform.
fn rect_of(clip: &matcha_ecs::clip::ClipRect) -> [f32; 4] {
    let origin = clip.transform.transform_point(&nalgebra::Point3::origin());
    let far = clip
        .transform
        .transform_point(&nalgebra::Point3::new(1.0, 1.0, 0.0));
    [origin.x, origin.y, far.x, far.y]
}

#[test]
fn a_frame_with_no_clip_widget_declares_no_clips() {
    let (world, root) = laid_out(|s| {
        s.node(Column::new(), |s| {
            s.leaf(ColorRect::new(40.0, 40.0));
        });
    });

    let frame = extract_items(&world, root);
    assert!(frame.clips.is_empty());
    assert!(frame.items.iter().all(|i| i.clip.is_none()));
}

#[test]
fn a_clip_covers_the_declaring_entity_as_well_as_its_children() {
    let (world, root) = laid_out(|s| {
        s.node(Panel::new(100.0, 80.0).clip(true), |s| {
            s.leaf(ColorRect::new(400.0, 400.0));
        });
    });

    let frame = extract_items(&world, root);
    assert_eq!(frame.clips.len(), 1);
    // Both the panel's own item and its child's sit inside it. Covering the
    // declaring entity is what lets a widget clip content it paints itself.
    assert_eq!(frame.items.len(), 2);
    assert!(frame.items.iter().all(|i| i.clip == Some(0)));
}

#[test]
fn the_clip_rect_is_the_declaring_entitys_laid_out_box() {
    let (world, root) = laid_out(|s| {
        s.node(Column::new(), |s| {
            s.leaf(ColorRect::new(40.0, 25.0));
            s.node(Panel::new(100.0, 80.0).clip(true), |s| {
                s.leaf(ColorRect::new(10.0, 10.0));
            });
        });
    });

    let frame = extract_items(&world, root);
    assert_eq!(frame.clips.len(), 1);
    // Second child of a Column: offset down by the first child's height.
    assert_eq!(rect_of(&frame.clips.as_slice()[0]), [0.0, 25.0, 100.0, 105.0]);
}

#[test]
fn nested_clips_chain_outermost_first() {
    let (world, root) = laid_out(|s| {
        s.node(Panel::new(200.0, 200.0).clip(true), |s| {
            s.node(Panel::new(100.0, 100.0).clip(true), |s| {
                s.leaf(ColorRect::new(20.0, 20.0));
            });
        });
    });

    let frame = extract_items(&world, root);
    assert_eq!(frame.clips.len(), 2);

    let clips = frame.clips.as_slice();
    // A parent is always seen before its child, so its index is smaller — the
    // property the renderer relies on to resolve a chain in one forward pass.
    assert_eq!(clips[0].parent, None);
    assert_eq!(clips[1].parent, Some(0));
    assert_eq!(frame.clips.chain(1), vec![0, 1]);

    // The innermost leaf names only the innermost clip; the rest is inherited.
    let leaf = frame.items.last().expect("leaf item");
    assert_eq!(leaf.clip, Some(1));
}

#[test]
fn a_sibling_outside_the_clip_is_unaffected_by_it() {
    let (world, root) = laid_out(|s| {
        s.node(Column::new(), |s| {
            s.node(Panel::new(100.0, 80.0).clip(true), |s| {
                s.leaf(ColorRect::new(10.0, 10.0));
            });
            s.leaf(ColorRect::new(40.0, 40.0));
        });
    });

    let frame = extract_items(&world, root);
    assert_eq!(frame.clips.len(), 1);
    // Panel, its child, then the sibling — the sibling is not inside the clip
    // even though it was visited after it.
    let clips: Vec<_> = frame.items.iter().map(|i| i.clip).collect();
    assert_eq!(clips, vec![Some(0), Some(0), None]);
}

#[test]
fn toggling_clip_off_removes_it_on_the_next_view() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();

    let build = |world: &mut World, clip: bool| {
        run_view(world, root, |s| {
            s.node(Panel::new(100.0, 80.0).clip(clip), |s| {
                s.leaf(ColorRect::new(10.0, 10.0));
            });
        });
        layout_root(world, root, Constraints::from_max_size([800.0, 600.0]));
    };

    build(&mut world, true);
    assert_eq!(extract_items(&world, root).clips.len(), 1);

    // Same widget type, so the entity is patched rather than rebuilt: the
    // marker has to be removed, not merely left off a fresh bundle.
    build(&mut world, false);
    assert!(extract_items(&world, root).clips.is_empty());
}

#[test]
fn a_widget_clipped_out_of_sight_is_no_longer_pickable() {
    // The button is laid out at the panel's origin but is wider and taller
    // than the panel, so its bottom-right corner is clipped away.
    let (world, root) = laid_out(|s| {
        s.node(Panel::new(50.0, 50.0).clip(true), |s| {
            s.leaf(Button::<()>::new("hit me").size(200.0, 200.0));
        });
    });
    let picker = RectZPicker::build(&world, root);

    // Inside the panel: still pickable.
    assert!(picker
        .pick(&world, &PickQuery {
            viewport_pos: [25.0, 25.0]
        })
        .is_some());

    // Over the button but outside the panel: clipped, so nothing is there.
    assert!(picker
        .pick(&world, &PickQuery {
            viewport_pos: [120.0, 120.0]
        })
        .is_none());
}

#[test]
fn a_widget_entirely_outside_its_clip_is_dropped_from_picking() {
    let (world, root) = laid_out(|s| {
        s.node(Column::new(), |s| {
            // Zero-height panel: nothing inside it can be seen at all.
            s.node(Panel::new(50.0, 0.0).clip(true), |s| {
                s.leaf(Button::<()>::new("invisible").size(80.0, 40.0));
            });
        });
    });
    let picker = RectZPicker::build(&world, root);

    for pos in [[10.0, 0.0], [40.0, 20.0], [70.0, 35.0]] {
        assert!(
            picker.pick(&world, &PickQuery { viewport_pos: pos }).is_none(),
            "nothing should be pickable at {pos:?}"
        );
    }
}
