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
    pick::{PickQuery, Picker, RectPicker},
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
    let picker = RectPicker::build(&world, root);

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
    let picker = RectPicker::build(&world, root);

    for pos in [[10.0, 0.0], [40.0, 20.0], [70.0, 35.0]] {
        assert!(
            picker.pick(&world, &PickQuery { viewport_pos: pos }).is_none(),
            "nothing should be pickable at {pos:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The clip model itself
//
// Picking used to carry a running rectangle intersection while drawing carried
// this arena: two models of the same thing, agreeing only for as long as every
// transform stayed a translation. These pin the arena's own containment rule,
// which both now use.
// ---------------------------------------------------------------------------

/// A unit-quad transform for the axis-aligned box at `origin` of `size`.
fn box_transform(origin: [f32; 2], size: [f32; 2]) -> nalgebra::Matrix4<f32> {
    nalgebra::Matrix4::new_translation(&nalgebra::Vector3::new(origin[0], origin[1], 0.0))
        * nalgebra::Matrix4::new_nonuniform_scaling(&nalgebra::Vector3::new(
            size[0], size[1], 1.0,
        ))
}

#[test]
fn containment_is_half_open_so_abutting_clips_never_both_claim_a_point() {
    let mut arena = matcha_ecs::clip::ClipArena::default();
    let a = arena.push(None, box_transform([0.0, 0.0], [1.0, 1.0]), [10.0, 10.0]);
    let b = arena.push(None, box_transform([10.0, 0.0], [1.0, 1.0]), [10.0, 10.0]);

    // x = 10 is a's exclusive far edge and b's inclusive near edge.
    assert!(arena.contains(Some(a), [9.99, 5.0]));
    assert!(!arena.contains(Some(a), [10.0, 5.0]));
    assert!(arena.contains(Some(b), [10.0, 5.0]));
}

#[test]
fn an_unclipped_item_is_contained_everywhere() {
    let arena = matcha_ecs::clip::ClipArena::default();
    assert!(arena.contains(None, [-1e6, 1e6]));
}

#[test]
fn a_chain_requires_every_clip_in_it() {
    let mut arena = matcha_ecs::clip::ClipArena::default();
    let outer = arena.push(None, box_transform([0.0, 0.0], [1.0, 1.0]), [100.0, 100.0]);
    let inner = arena.push(
        Some(outer),
        box_transform([50.0, 50.0], [1.0, 1.0]),
        [100.0, 100.0],
    );

    // Inside `inner`'s own box but outside `outer` — only the chain catches it.
    assert!(arena.contains(Some(inner), [70.0, 70.0]));
    assert!(!arena.contains(Some(inner), [120.0, 120.0]));
    assert!(arena.contains(Some(outer), [70.0, 70.0]));
}

#[test]
fn a_rotated_clip_cuts_along_its_own_edges_not_its_bounding_box() {
    // This is the case a rectangle intersection cannot express, and the reason
    // picking now reads the same arena the renderer does. A 100x100 box rotated
    // 45 degrees about its centre still has a 100x100 axis-aligned bounding
    // box, but its corners are no longer inside it.
    let centre = nalgebra::Vector3::new(50.0, 50.0, 0.0);
    let transform = nalgebra::Matrix4::new_translation(&centre)
        * nalgebra::Matrix4::from_euler_angles(0.0, 0.0, std::f32::consts::FRAC_PI_4)
        * nalgebra::Matrix4::new_translation(&-centre)
        * box_transform([0.0, 0.0], [1.0, 1.0]);

    let mut arena = matcha_ecs::clip::ClipArena::default();
    let clip = arena.push(None, transform, [100.0, 100.0]);

    assert!(
        arena.contains(Some(clip), [50.0, 50.0]),
        "the centre is inside under any rotation"
    );
    assert!(
        !arena.contains(Some(clip), [2.0, 2.0]),
        "the unrotated box's top-left corner is now outside the diamond"
    );
    assert!(
        arena.contains(Some(clip), [50.0, 5.0]),
        "but the diamond's own top vertex region is inside"
    );
}

#[test]
fn a_degenerate_clip_contains_nothing() {
    let mut arena = matcha_ecs::clip::ClipArena::default();
    let clip = arena.push(None, box_transform([0.0, 0.0], [1.0, 1.0]), [0.0, 50.0]);
    assert!(!arena.contains(Some(clip), [0.0, 25.0]));
}
