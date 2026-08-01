//! Space distribution (`grow`/`shrink`), plus the other per-child box
//! properties a container reads: `align_self`, `order`, and `reverse`.
//!
//! `Container` is used as the child throughout because it is one of the
//! widgets carrying `Sizing` today; the container reads these off whatever
//! child it has, so nothing here is specific to that choice.
//!
//! Headless, GPU-free: only `LayoutOutput` is inspected.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{layout::LayoutOutput, view::ViewChildren},
    layout::{layout_root, Constraints},
    view::{run_view, Scope},
};
use matcha_ecs_widgets::{AlignItems, ColorRect, Column, Container, Length, Row};

const WINDOW: [f32; 2] = [800.0, 600.0];

fn setup() -> (World, Entity) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    (world, root)
}

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

fn output(world: &World, e: Entity) -> LayoutOutput {
    *world
        .get::<LayoutOutput>(e)
        .unwrap_or_else(|| panic!("entity {e:?} has no LayoutOutput after layout_root"))
}

/// Lay out `build` under the window and return the top widget's children.
fn run(build: impl Fn(&mut Scope) + Send + Sync + 'static) -> (World, Vec<Entity>) {
    let (mut world, root) = setup();
    run_view(&mut world, root, build);
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));
    let top = children(&world, root)[0];
    let kids = children(&world, top);
    (world, kids)
}

/// A `Container` of the given width wrapping a rect, so the child carries
/// `Sizing` for the row to read.
fn boxed(width: f32, key: u64) -> Container {
    Container::new().key(key).width(Length::Px(width))
}

#[test]
fn a_single_grower_absorbs_all_the_leftover_space() {
    let (world, kids) = run(|s| {
        s.node(Row::new().width(Length::Fill), |s| {
            s.node(boxed(50.0, 1), |s| s.leaf(ColorRect::new(50.0, 20.0)));
            s.node(boxed(50.0, 2).grow(1.0), |s| {
                s.leaf(ColorRect::new(50.0, 20.0))
            });
        });
    });

    assert_eq!(output(&world, kids[0]).size[0], 50.0);
    assert_eq!(output(&world, kids[1]).size[0], WINDOW[0] - 50.0);
    // And it starts where the fixed one ended, not somewhere else.
    assert_eq!(output(&world, kids[1]).origin[0], 50.0);
}

#[test]
fn growers_split_the_leftover_in_proportion_to_their_weights() {
    let (world, kids) = run(|s| {
        s.node(Row::new().width(Length::Fill), |s| {
            s.node(boxed(0.0, 1).grow(1.0), |s| {
                s.leaf(ColorRect::new(0.0, 20.0))
            });
            s.node(boxed(0.0, 2).grow(3.0), |s| {
                s.leaf(ColorRect::new(0.0, 20.0))
            });
        });
    });

    assert_eq!(output(&world, kids[0]).size[0], 200.0);
    assert_eq!(output(&world, kids[1]).size[0], 600.0);
}

#[test]
fn growth_stops_at_a_declared_maximum() {
    let (world, kids) = run(|s| {
        s.node(Row::new().width(Length::Fill), |s| {
            s.node(boxed(0.0, 1).grow(1.0).max_width(Length::Px(100.0)), |s| {
                s.leaf(ColorRect::new(0.0, 20.0))
            });
            s.node(boxed(0.0, 2).grow(1.0), |s| {
                s.leaf(ColorRect::new(0.0, 20.0))
            });
        });
    });

    assert_eq!(output(&world, kids[0]).size[0], 100.0);
    // What the capped one refused goes to the other, rather than being lost.
    assert_eq!(output(&world, kids[1]).size[0], WINDOW[0] - 100.0);
}

#[test]
fn overflowing_children_shrink_in_proportion_to_their_size() {
    // Two 300-wide boxes in a 400-wide row: 200 of overflow, shared equally
    // because they are the same size at the same shrink factor.
    let (world, kids) = run(|s| {
        s.node(Row::new().width(Length::Px(400.0)), |s| {
            s.node(boxed(300.0, 1), |s| s.leaf(ColorRect::new(10.0, 20.0)));
            s.node(boxed(300.0, 2), |s| s.leaf(ColorRect::new(10.0, 20.0)));
        });
    });

    assert_eq!(output(&world, kids[0]).size[0], 200.0);
    assert_eq!(output(&world, kids[1]).size[0], 200.0);
}

#[test]
fn a_child_refusing_to_shrink_keeps_its_size_and_the_rest_absorb_the_overflow() {
    let (world, kids) = run(|s| {
        s.node(Row::new().width(Length::Px(400.0)), |s| {
            s.node(boxed(300.0, 1).shrink(0.0), |s| {
                s.leaf(ColorRect::new(10.0, 20.0))
            });
            s.node(boxed(300.0, 2), |s| s.leaf(ColorRect::new(10.0, 20.0)));
        });
    });

    assert_eq!(output(&world, kids[0]).size[0], 300.0);
    assert_eq!(output(&world, kids[1]).size[0], 100.0);
}

#[test]
fn shrinking_stops_at_a_declared_minimum() {
    let (world, kids) = run(|s| {
        s.node(Row::new().width(Length::Px(400.0)), |s| {
            s.node(boxed(300.0, 1).min_width(Length::Px(250.0)), |s| {
                s.leaf(ColorRect::new(10.0, 20.0))
            });
            s.node(boxed(300.0, 2), |s| s.leaf(ColorRect::new(10.0, 20.0)));
        });
    });

    assert_eq!(output(&world, kids[0]).size[0], 250.0);
    assert_eq!(output(&world, kids[1]).size[0], 150.0);
}

#[test]
fn nothing_is_distributed_when_the_children_already_fit() {
    // The overwhelmingly common case, and the one that has to stay exactly as
    // it was before distribution existed.
    let (world, kids) = run(|s| {
        s.node(Row::new(), |s| {
            s.node(boxed(50.0, 1), |s| s.leaf(ColorRect::new(50.0, 20.0)));
            s.node(boxed(30.0, 2), |s| s.leaf(ColorRect::new(30.0, 20.0)));
        });
    });

    assert_eq!(output(&world, kids[0]).size[0], 50.0);
    assert_eq!(output(&world, kids[1]).size[0], 30.0);
}

#[test]
fn align_self_overrides_the_containers_align_items_for_one_child() {
    let (world, kids) = run(|s| {
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            s.node(boxed(100.0, 1), |s| s.leaf(ColorRect::new(100.0, 20.0)));
            s.node(boxed(40.0, 2).align_self(AlignItems::End), |s| {
                s.leaf(ColorRect::new(40.0, 20.0))
            });
        });
    });

    // Column is 100 wide; the second child is 40 and pushed to the far edge.
    assert_eq!(output(&world, kids[0]).origin[0], 0.0);
    assert_eq!(output(&world, kids[1]).origin[0], 60.0);
}

#[test]
fn order_lays_children_out_in_a_different_sequence_than_declared() {
    let (world, kids) = run(|s| {
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            s.node(boxed(50.0, 1).order(1), |s| {
                s.leaf(ColorRect::new(50.0, 20.0))
            });
            s.node(boxed(50.0, 2).order(-1), |s| {
                s.leaf(ColorRect::new(50.0, 40.0))
            });
        });
    });

    // `kids` is declaration order, so the second declared one is on top.
    assert_eq!(output(&world, kids[1]).origin[1], 0.0);
    assert_eq!(output(&world, kids[0]).origin[1], 40.0);
}

#[test]
fn reverse_lays_the_children_out_from_the_far_end() {
    let (world, kids) = run(|s| {
        s.node(Row::new().reverse(true), |s| {
            s.node(boxed(50.0, 1), |s| s.leaf(ColorRect::new(50.0, 20.0)));
            s.node(boxed(30.0, 2), |s| s.leaf(ColorRect::new(30.0, 20.0)));
        });
    });

    assert_eq!(output(&world, kids[1]).origin[0], 0.0);
    assert_eq!(output(&world, kids[0]).origin[0], 30.0);
}

#[test]
fn a_grown_child_leaves_no_leftover_for_justify_content_to_distribute() {
    // Growing and justifying compete for the same space, and growing wins:
    // there is nothing left by the time positions are decided.
    let (world, kids) = run(|s| {
        s.node(
            Row::new()
                .width(Length::Fill)
                .justify_content(matcha_ecs_widgets::JustifyContent::Center),
            |s| {
                s.node(boxed(50.0, 1).grow(1.0), |s| {
                    s.leaf(ColorRect::new(50.0, 20.0))
                });
            },
        );
    });

    assert_eq!(output(&world, kids[0]).origin[0], 0.0);
    assert_eq!(output(&world, kids[0]).size[0], WINDOW[0]);
}
