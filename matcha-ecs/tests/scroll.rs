//! Headless verification of scrolling: that an offset reaches the content's
//! transform, that it is clamped to what the content allows, that an axis which
//! does not scroll cannot be moved, that scrolled-away content stops being
//! pickable, and that a scroll nobody can act on chains to an enclosing view.
//!
//! GPU-free, per this suite's convention — `RenderItem::builder` is never
//! invoked. What a scrollbar *looks like* is checked by unit tests over the
//! pure geometry functions (`scroll_view::geometry`) and, for the pixels, by
//! `examples/scroll_demo.rs`.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{
        input::PointerPhase,
        layout::{GlobalTransform, LayoutOutput},
        view::ViewChildren,
    },
    input::dispatch_pointer,
    layout::{layout_root, Constraints},
    pick::{PickQuery, Picker, RectZPicker},
    render::extract_items,
    view::{run_view, Scope},
};
use matcha_ecs_widgets::{
    scroll_view::{scroll_view, Overflow, ScrollState, ScrollView},
    ColorRect, Column,
};

const WINDOW: [f32; 2] = [800.0, 600.0];

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

/// Top-left corner of an entity's laid-out box, in window space.
fn origin_of(world: &World, e: Entity) -> [f32; 2] {
    let p = world
        .get::<GlobalTransform>(e)
        .expect("laid out")
        .affine
        .transform_point(&nalgebra::Point3::origin());
    [p.x, p.y]
}

fn state_of(world: &World, e: Entity) -> ScrollState {
    world.get::<ScrollState>(e).expect("a scroll view").clone()
}

/// Build a view and lay it out. Returns the world and the root entity.
fn laid_out(view: impl Fn(&mut Scope)) -> (World, Entity) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| view(s));
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));
    (world, root)
}

/// A 200x100 viewport over a 200x400 rectangle — 300px of vertical overflow.
fn tall_content(world: &mut World, root: Entity) {
    run_view(world, root, |s| {
        scroll_view(s, ScrollView::new(200.0, 100.0), |s| {
            s.leaf(ColorRect::new(200.0, 400.0));
        });
    });
    layout_root(world, root, Constraints::from_max_size(WINDOW));
}

#[test]
fn the_offset_moves_the_content_and_leaves_the_viewport_where_it_is() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    tall_content(&mut world, root);

    let view = children(&world, root)[0];
    let content = children(&world, view)[0];

    assert_eq!(origin_of(&world, content), [0.0, 0.0], "unscrolled");

    state_of(&world, view).set_offset([0.0, 50.0]);
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    // The content slid up by the offset...
    assert_eq!(origin_of(&world, content), [0.0, -50.0]);
    // ...while the window onto it did not move. This is what keeps the clip
    // rect and the hit area in place while the content scrolls behind them.
    assert_eq!(origin_of(&world, view), [0.0, 0.0]);
    assert_eq!(
        world.get::<LayoutOutput>(view).expect("laid out").size,
        [200.0, 100.0]
    );
}

#[test]
fn the_offset_is_clamped_to_the_content_that_exists() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    tall_content(&mut world, root);
    let view = children(&world, root)[0];
    let state = state_of(&world, view);

    // 400 tall in a 100 tall window leaves exactly 300 to scroll.
    assert_eq!(state.geometry().max_offset(), [0.0, 300.0]);

    state.set_offset([0.0, 9999.0]);
    assert_eq!(state.offset(), [0.0, 300.0]);

    state.set_offset([0.0, -40.0]);
    assert_eq!(state.offset(), [0.0, 0.0]);

    // Nothing overflows horizontally, so that axis cannot move at all.
    state.set_offset([80.0, 0.0]);
    assert_eq!(state.offset(), [0.0, 0.0]);
}

#[test]
fn an_axis_that_does_not_scroll_measures_its_content_to_the_viewport() {
    let (mut world, root) = laid_out(|s| {
        scroll_view(
            s,
            ScrollView::new(200.0, 100.0).overflow_y(Overflow::Hidden),
            |s| {
                s.leaf(ColorRect::new(200.0, 400.0));
            },
        );
    });

    let view = children(&world, root)[0];
    let state = state_of(&world, view);

    // Pinned to the viewport rather than measured at its natural 400, so there
    // is no overflow and therefore nothing to scroll.
    assert_eq!(state.geometry().content, [200.0, 100.0]);
    assert_eq!(state.geometry().max_offset(), [0.0, 0.0]);

    state.set_offset([0.0, 200.0]);
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));
    let content = children(&world, view)[0];
    assert_eq!(origin_of(&world, content), [0.0, 0.0]);
}

#[test]
fn a_scrolling_axis_measures_its_content_at_its_natural_size() {
    let (world, root) = laid_out(|s| {
        scroll_view(s, ScrollView::new(200.0, 100.0), |s| {
            // A column far taller than the viewport: without an unbounded
            // measure it would report the viewport's height and never scroll.
            s.node(Column::new(), |s| {
                for _ in 0..6 {
                    s.leaf(ColorRect::new(200.0, 90.0));
                }
            });
        });
    });

    let view = children(&world, root)[0];
    assert_eq!(state_of(&world, view).geometry().content, [200.0, 540.0]);
}

#[test]
fn the_view_declares_one_clip_that_covers_its_content() {
    let (world, root) = laid_out(|s| {
        scroll_view(s, ScrollView::new(200.0, 100.0), |s| {
            s.leaf(ColorRect::new(200.0, 400.0));
        });
    });

    let frame = extract_items(&world, root);
    assert_eq!(frame.clips.len(), 1);
    assert!(
        frame.items.iter().all(|i| i.clip == Some(0)),
        "everything inside the view, bars included, is clipped to it"
    );
}

#[test]
fn the_bars_are_painted_after_the_content() {
    let (world, root) = laid_out(|s| {
        scroll_view(s, ScrollView::new(200.0, 100.0), |s| {
            s.leaf(ColorRect::new(200.0, 400.0));
        });
    });

    let frame = extract_items(&world, root);
    // Content first — this is what makes an overlay scrollbar an overlay, and
    // it comes purely from the bars being declared after the content.
    assert_eq!(
        frame.items.first().expect("content item").size,
        [200.0, 400.0]
    );
    // The vertical bar's track is thin and comes later.
    assert!(
        frame
            .items
            .iter()
            .skip(1)
            .any(|i| i.size[0] > 0.0 && i.size[0] < 20.0 && i.size[1] > 20.0),
        "a thin vertical track should be painted after the content"
    );
}

#[test]
fn a_bar_with_nothing_to_scroll_collapses_to_nothing() {
    let (world, root) = laid_out(|s| {
        // Content smaller than the viewport: no overflow on either axis.
        scroll_view(s, ScrollView::new(200.0, 100.0), |s| {
            s.leaf(ColorRect::new(50.0, 50.0));
        });
    });

    let view = children(&world, root)[0];
    // [content, bar(Y), bar(X)] — bars are declared unconditionally because
    // whether they are needed is only known after layout.
    let kids = children(&world, view);
    assert_eq!(kids.len(), 3);

    for &bar in &kids[1..] {
        assert_eq!(
            world.get::<LayoutOutput>(bar).expect("laid out").size,
            [0.0, 0.0]
        );
        let thumb = children(&world, bar)[0];
        assert_eq!(
            world.get::<LayoutOutput>(thumb).expect("laid out").size,
            [0.0, 0.0]
        );
    }
}

#[test]
fn content_scrolled_out_of_the_viewport_is_no_longer_pickable() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        scroll_view(s, ScrollView::new(200.0, 100.0), |s| {
            s.node(Column::new(), |s| {
                s.leaf(matcha_ecs_widgets::Button::<()>::new("top").size(120.0, 40.0));
                s.leaf(ColorRect::new(200.0, 400.0));
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let view = children(&world, root)[0];
    let picker = RectZPicker::build(&world, root);
    let at = |picker: &RectZPicker, world: &World, pos: [f32; 2]| {
        picker
            .pick(world, &PickQuery { viewport_pos: pos })
            .map(|h| h.entity)
    };

    let column = children(&world, view)[0];
    let button = children(&world, column)[0];
    assert_eq!(at(&picker, &world, [20.0, 20.0]), Some(button));

    // Scroll the button off the top of the viewport.
    state_of(&world, view).set_offset([0.0, 200.0]);
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));
    let picker = RectZPicker::build(&world, root);

    let hit = at(&picker, &world, [20.0, 20.0]);
    assert_ne!(
        hit,
        Some(button),
        "the button is above the viewport now, so it cannot be clicked"
    );
}

#[test]
fn a_scroll_the_inner_view_cannot_act_on_chains_to_the_outer_one() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        scroll_view(s, ScrollView::new(200.0, 100.0), |s| {
            scroll_view(s, ScrollView::new(200.0, 200.0), |s| {
                s.leaf(ColorRect::new(200.0, 800.0));
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let outer = children(&world, root)[0];
    let inner = children(&world, outer)[0];
    let (outer_state, inner_state) = (state_of(&world, outer), state_of(&world, inner));

    // 200 tall inside a 100 tall window; 800 tall inside the 200 tall inner.
    assert_eq!(outer_state.geometry().max_offset(), [0.0, 100.0]);
    assert_eq!(inner_state.geometry().max_offset(), [0.0, 600.0]);

    // Away from the right edge, so the pick lands on content rather than a bar.
    let query = PickQuery {
        viewport_pos: [20.0, 20.0],
    };
    let hit = RectZPicker::build(&world, root)
        .pick(&world, &query)
        .map(|h| h.entity)
        .expect("something under the cursor");

    // While the inner view can still move, it consumes the scroll itself.
    let consumed = dispatch_pointer(
        &mut world,
        hit,
        query.viewport_pos,
        PointerPhase::Scroll { delta: [0.0, -40.0] },
    );
    assert!(consumed);
    assert_eq!(inner_state.offset(), [0.0, 40.0]);
    assert_eq!(outer_state.offset(), [0.0, 0.0], "the outer view stayed put");

    // Pin the inner view at its end; now it has nothing left to give.
    inner_state.set_offset([0.0, 600.0]);
    let consumed = dispatch_pointer(
        &mut world,
        hit,
        query.viewport_pos,
        PointerPhase::Scroll { delta: [0.0, -40.0] },
    );
    assert!(consumed, "the outer view took over");
    assert_eq!(inner_state.offset(), [0.0, 600.0], "still at its end");
    assert_eq!(outer_state.offset(), [0.0, 40.0], "the scroll chained out");

    // With both pinned, nothing consumes it — which is what lets a scroll pass
    // out of the widget tree entirely.
    outer_state.set_offset([0.0, 100.0]);
    let consumed = dispatch_pointer(
        &mut world,
        hit,
        query.viewport_pos,
        PointerPhase::Scroll { delta: [0.0, -40.0] },
    );
    assert!(!consumed);
}

/// Press the thumb, then drag: the press records the grab and the drag is
/// resolved by the viewport (which stays under the pointer even when the cursor
/// slips off a thumb a few pixels wide).
#[test]
fn dragging_the_thumb_scrolls_in_proportion_to_how_far_it_moved() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    tall_content(&mut world, root);

    let view = children(&world, root)[0];
    let bar_y = children(&world, view)[1];
    let thumb = children(&world, bar_y)[0];
    let state = state_of(&world, view);

    // Default style: 8px thick, 2px gap, 24px minimum thumb. A quarter of the
    // content is visible, so over a 96px track the thumb is its 24px minimum,
    // parked at the top.
    assert_eq!(origin_of(&world, thumb), [190.0, 2.0]);
    assert_eq!(
        world.get::<LayoutOutput>(thumb).expect("laid out").size,
        [8.0, 24.0]
    );

    // Grab the thumb 8px down from its top.
    let grab = [194.0, 10.0];
    let picked = RectZPicker::build(&world, root)
        .pick(&world, &PickQuery { viewport_pos: grab })
        .map(|h| h.entity);
    assert_eq!(picked, Some(thumb), "the press must land on the thumb");
    assert!(dispatch_pointer(
        &mut world,
        thumb,
        grab,
        PointerPhase::Press { count: 1 }
    ));
    assert_eq!(state.offset(), [0.0, 0.0], "pressing alone scrolls nothing");

    // Drag 30px down. The thumb bubbles the drag up to the viewport, which maps
    // the thumb's 72px of travel onto the content's 300px of overflow.
    assert!(dispatch_pointer(
        &mut world,
        thumb,
        [194.0, 40.0],
        PointerPhase::Drag
    ));
    assert_eq!(state.offset(), [0.0, 125.0]);

    // Dragging past the end saturates rather than running away.
    dispatch_pointer(&mut world, thumb, [194.0, 900.0], PointerPhase::Drag);
    assert_eq!(state.offset(), [0.0, 300.0]);

    // A fresh press elsewhere ends the drag, so later drags do not keep
    // scrolling.
    dispatch_pointer(&mut world, view, [20.0, 20.0], PointerPhase::Press { count: 1 });
    assert!(!dispatch_pointer(&mut world, view, [20.0, 60.0], PointerPhase::Drag));
    assert_eq!(state.offset(), [0.0, 300.0]);
}

#[test]
fn clicking_the_track_pages_towards_the_click() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    tall_content(&mut world, root);

    let view = children(&world, root)[0];
    let bar_y = children(&world, view)[1];
    let state = state_of(&world, view);

    // Below the thumb (which occupies the track's first 24px): page down by one
    // viewport height.
    assert!(dispatch_pointer(
        &mut world,
        bar_y,
        [194.0, 80.0],
        PointerPhase::Press { count: 1 }
    ));
    assert_eq!(state.offset(), [0.0, 100.0]);

    // Above the thumb, which has now moved down: page back up.
    assert!(dispatch_pointer(
        &mut world,
        bar_y,
        [194.0, 4.0],
        PointerPhase::Press { count: 1 }
    ));
    assert_eq!(state.offset(), [0.0, 0.0]);
}

#[test]
fn a_wheel_scroll_does_not_move_an_axis_that_is_not_scrollable() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        // Horizontal overflow, but only the vertical axis may scroll.
        scroll_view(s, ScrollView::new(100.0, 100.0), |s| {
            s.leaf(ColorRect::new(400.0, 400.0));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let view = children(&world, root)[0];
    let state = state_of(&world, view);
    // Width was pinned to the viewport, so there is no horizontal overflow to
    // scroll even though the rect asked to be wider.
    assert_eq!(state.geometry().max_offset(), [0.0, 300.0]);

    let query = PickQuery {
        viewport_pos: [20.0, 20.0],
    };
    let hit = RectZPicker::build(&world, root)
        .pick(&world, &query)
        .map(|h| h.entity)
        .expect("something under the cursor");
    dispatch_pointer(
        &mut world,
        hit,
        query.viewport_pos,
        PointerPhase::Scroll {
            delta: [-30.0, -30.0],
        },
    );

    assert_eq!(state.offset(), [0.0, 30.0]);
}
