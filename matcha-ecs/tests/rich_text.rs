//! Headless verification of `RichText` (parley-backed), mirroring
//! `tests/text.rs`'s coverage of the suzuri-backed `Text` widget. Same
//! GPU-free approach: `RenderItem::builder` is never invoked, only its
//! `cache` `Arc` identity is asserted. Actual glyph rasterisation/atlas
//! upload needs a real `wgpu::Device` and is left to manual/demo
//! verification.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    animation::Easing,
    components::{layout::LayoutOutput, render::RenderItem, view::ViewChildren},
    layout::{layout_root, Constraints},
    view::run_view,
};
use matcha_ecs_widgets::{parley, RichText};

fn setup() -> (World, Entity) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    (world, root)
}

fn first_child(world: &World, root: Entity) -> Entity {
    world
        .get::<ViewChildren>(root)
        .expect("root has ViewChildren")
        .slots[0]
        .1
}

fn output(world: &World, e: Entity) -> LayoutOutput {
    *world
        .get::<LayoutOutput>(e)
        .unwrap_or_else(|| panic!("entity {e:?} has no LayoutOutput after layout_root"))
}

const LONG_SENTENCE: &str =
    "the quick brown fox jumps over the lazy dog and keeps running past the hills";

#[test]
fn empty_content_measures_to_near_zero_size() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new(""));
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let child = first_child(&world, root);
    let out = output(&world, child);
    assert_eq!(out.size, [0.0, 0.0]);
}

#[test]
fn narrow_constraint_wraps_to_a_taller_block_than_a_wide_one() {
    let build = |s: &mut matcha_ecs::view::Scope| {
        s.leaf(RichText::new(LONG_SENTENCE).font_size(16.0));
    };

    let (mut world_wide, root_wide) = setup();
    run_view(&mut world_wide, root_wide, build);
    layout_root(&mut world_wide, root_wide, Constraints::from_max_size([2000.0, 2000.0]));
    let wide = output(&world_wide, first_child(&world_wide, root_wide));

    let (mut world_narrow, root_narrow) = setup();
    run_view(&mut world_narrow, root_narrow, build);
    layout_root(&mut world_narrow, root_narrow, Constraints::from_max_size([60.0, 2000.0]));
    let narrow = output(&world_narrow, first_child(&world_narrow, root_narrow));

    assert!(
        narrow.size[1] > wide.size[1],
        "wrapping at a narrow width must produce more lines (taller block): wide={:?} narrow={:?}",
        wide,
        narrow
    );
    assert!(
        narrow.size[0] <= 60.0 + 0.01,
        "wrapped width must respect the narrow constraint: {:?}",
        narrow
    );
}

#[test]
fn unchanged_props_do_not_invalidate_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_size(16.0).color([0.0, 0.0, 0.0, 1.0]));
    });
    let child = first_child(&world, root);
    let cache_before = world
        .get::<RenderItem>(child)
        .expect("RichText carries a RenderItem")
        .cache
        .clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_size(16.0).color([0.0, 0.0, 0.0, 1.0]));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must be unchanged when no draw-relevant prop changed"
    );
}

#[test]
fn changed_content_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello"));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("goodbye"));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when content changed"
    );
}

#[test]
fn changed_span_style_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello ").span("world", |span| span.font_size(16.0)));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello ").span("world", |span| span.font_size(32.0)));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when a span's style changed, even though the base text and span text are unchanged"
    );
}

#[test]
fn enabling_underline_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").underline(false));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").underline(true));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when underline is enabled"
    );
}

#[test]
fn underline_color_only_change_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").underline(true).underline_color(Some([1.0, 0.0, 0.0, 1.0])));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").underline(true).underline_color(Some([0.0, 1.0, 0.0, 1.0])));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when underline_color changes, even with the same enabled state"
    );
}

#[test]
fn enabling_strikethrough_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").strikethrough(false));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").strikethrough(true));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when strikethrough is enabled"
    );
}

#[test]
fn span_underline_override_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello ").span("world", |span| span.underline(false)));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello ").span("world", |span| span.underline(true)));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when a span's underline override changes"
    );
}

#[test]
fn span_larger_font_size_measures_a_taller_block() {
    let build_with_span_size = |span_size: f32| {
        move |s: &mut matcha_ecs::view::Scope| {
            s.leaf(RichText::new("hello ").font_size(16.0).span("world", move |span| span.font_size(span_size)));
        }
    };

    let (mut world_small, root_small) = setup();
    run_view(&mut world_small, root_small, build_with_span_size(16.0));
    layout_root(&mut world_small, root_small, Constraints::from_max_size([800.0, 600.0]));
    let small = output(&world_small, first_child(&world_small, root_small));

    let (mut world_large, root_large) = setup();
    run_view(&mut world_large, root_large, build_with_span_size(64.0));
    layout_root(&mut world_large, root_large, Constraints::from_max_size([800.0, 600.0]));
    let large = output(&world_large, first_child(&world_large, root_large));

    assert!(
        large.size[1] > small.size[1],
        "a span with a much larger font_size must measure a taller block: small={:?} large={:?}",
        small,
        large
    );
}

#[test]
fn changed_font_size_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_size(16.0));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_size(24.0));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when font_size changed"
    );
}

#[test]
fn changed_color_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").color([1.0, 0.0, 0.0, 1.0]));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").color([0.0, 1.0, 0.0, 1.0]));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when color changed"
    );
}

#[test]
fn changed_font_family_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_family("serif"));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_family("monospace"));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when font_family changed"
    );
}

#[test]
fn changed_font_weight_invalidates_cache() {
    use parley::FontWeight;

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_weight(FontWeight::NORMAL));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_weight(FontWeight::BOLD));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when font_weight changed"
    );
}

#[test]
fn changed_font_style_invalidates_cache() {
    use parley::FontStyle;

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_style(FontStyle::Normal));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_style(FontStyle::Italic));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when font_style changed"
    );
}

#[test]
fn changed_font_width_invalidates_cache() {
    use parley::FontWidth;

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_width(FontWidth::NORMAL));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_width(FontWidth::CONDENSED));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when font_width changed"
    );
}

#[test]
fn changed_font_variations_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_variations("'wght' 400"));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_variations("'wght' 700"));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when font_variations changed"
    );
}

#[test]
fn changed_font_features_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_features("'liga' 0"));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").font_features("'liga' 1"));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when font_features changed"
    );
}

#[test]
fn changed_line_height_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").line_height(parley::LineHeight::FontSizeRelative(1.0)));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").line_height(parley::LineHeight::FontSizeRelative(2.0)));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when line_height changed"
    );
}

#[test]
fn changed_letter_spacing_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").letter_spacing(0.0));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").letter_spacing(5.0));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when letter_spacing changed"
    );
}

#[test]
fn changed_word_spacing_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").word_spacing(0.0));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").word_spacing(5.0));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when word_spacing changed"
    );
}

#[test]
fn changed_word_break_invalidates_cache() {
    use parley::WordBreak;

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").word_break(WordBreak::Normal));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").word_break(WordBreak::BreakAll));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when word_break changed"
    );
}

#[test]
fn changed_overflow_wrap_invalidates_cache() {
    use parley::OverflowWrap;

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").overflow_wrap(OverflowWrap::Normal));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").overflow_wrap(OverflowWrap::Anywhere));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when overflow_wrap changed"
    );
}

#[test]
fn changed_locale_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").locale("en"));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").locale("ja"));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when locale changed"
    );
}

#[test]
fn changed_text_align_invalidates_cache() {
    use parley::Alignment;

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").text_align(Alignment::Start));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").text_align(Alignment::Center));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when text_align changed"
    );
}

#[test]
fn changed_text_indent_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").text_indent(0.0));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").text_indent(20.0));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when text_indent changed"
    );
}

#[test]
fn unchanged_new_props_do_not_invalidate_cache() {
    let build = |s: &mut matcha_ecs::view::Scope| {
        s.leaf(
            RichText::new("hello")
                .font_family("Inter, system-ui, sans-serif")
                .font_weight(parley::FontWeight::new(650.0))
                .font_style(parley::FontStyle::Italic)
                .font_width(parley::FontWidth::CONDENSED)
                .font_variations("'wght' 650")
                .font_features("'liga' 0")
                .line_height(parley::LineHeight::FontSizeRelative(1.5))
                .letter_spacing(1.0)
                .word_spacing(1.0)
                .word_break(parley::WordBreak::BreakAll)
                .overflow_wrap(parley::OverflowWrap::Anywhere)
                .locale("ja")
                .text_align(parley::Alignment::Center)
                .text_indent(10.0),
        );
    };

    let (mut world, root) = setup();
    run_view(&mut world, root, build);
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, build);
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must be unchanged when an identical full property set is re-declared"
    );
}

#[test]
fn taller_line_height_measures_taller() {
    let build_with_line_height = |lh: f32| {
        move |s: &mut matcha_ecs::view::Scope| {
            s.leaf(RichText::new("hello").font_size(16.0).line_height(parley::LineHeight::FontSizeRelative(lh)));
        }
    };

    let (mut world_short, root_short) = setup();
    run_view(&mut world_short, root_short, build_with_line_height(1.0));
    layout_root(&mut world_short, root_short, Constraints::from_max_size([800.0, 600.0]));
    let short = output(&world_short, first_child(&world_short, root_short));

    let (mut world_tall, root_tall) = setup();
    run_view(&mut world_tall, root_tall, build_with_line_height(3.0));
    layout_root(&mut world_tall, root_tall, Constraints::from_max_size([800.0, 600.0]));
    let tall = output(&world_tall, first_child(&world_tall, root_tall));

    assert!(
        tall.size[1] > short.size[1],
        "a larger line_height must measure taller: short={:?} tall={:?}",
        short,
        tall
    );
}

#[test]
fn wider_letter_spacing_measures_wider() {
    let build_with_letter_spacing = |ls: f32| {
        move |s: &mut matcha_ecs::view::Scope| {
            s.leaf(RichText::new("hello world").font_size(16.0).letter_spacing(ls));
        }
    };

    let (mut world_tight, root_tight) = setup();
    run_view(&mut world_tight, root_tight, build_with_letter_spacing(0.0));
    layout_root(&mut world_tight, root_tight, Constraints::from_max_size([2000.0, 600.0]));
    let tight = output(&world_tight, first_child(&world_tight, root_tight));

    let (mut world_wide, root_wide) = setup();
    run_view(&mut world_wide, root_wide, build_with_letter_spacing(10.0));
    layout_root(&mut world_wide, root_wide, Constraints::from_max_size([2000.0, 600.0]));
    let wide = output(&world_wide, first_child(&world_wide, root_wide));

    assert!(
        wide.size[0] > tight.size[0],
        "a larger letter_spacing must measure wider: tight={:?} wide={:?}",
        tight,
        wide
    );
}

#[test]
fn uppercase_text_transform_measures_wider() {
    use matcha_ecs_widgets::TextTransform;

    let build_with_transform = |t: TextTransform| {
        move |s: &mut matcha_ecs::view::Scope| {
            s.leaf(RichText::new("iiiiiiiiii").font_size(16.0).text_transform(t));
        }
    };

    let (mut world_plain, root_plain) = setup();
    run_view(&mut world_plain, root_plain, build_with_transform(TextTransform::None));
    layout_root(&mut world_plain, root_plain, Constraints::from_max_size([2000.0, 600.0]));
    let plain = output(&world_plain, first_child(&world_plain, root_plain));

    let (mut world_upper, root_upper) = setup();
    run_view(&mut world_upper, root_upper, build_with_transform(TextTransform::Uppercase));
    layout_root(&mut world_upper, root_upper, Constraints::from_max_size([2000.0, 600.0]));
    let upper = output(&world_upper, first_child(&world_upper, root_upper));

    assert!(
        upper.size[0] > plain.size[0],
        "uppercase 'I's must measure wider than lowercase 'i's: plain={:?} upper={:?}",
        plain,
        upper
    );
}

#[test]
fn changed_text_transform_invalidates_cache() {
    use matcha_ecs_widgets::TextTransform;

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").text_transform(TextTransform::None));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").text_transform(TextTransform::Uppercase));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when text_transform changed"
    );
}

#[test]
fn changed_white_space_invalidates_cache() {
    use matcha_ecs_widgets::WhiteSpace;

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello   world").white_space(WhiteSpace::Normal));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello   world").white_space(WhiteSpace::Pre));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when white_space changed"
    );
}

#[test]
fn normal_white_space_collapses_runs_to_a_narrower_block() {
    let build_with_white_space = |ws: matcha_ecs_widgets::WhiteSpace| {
        move |s: &mut matcha_ecs::view::Scope| {
            s.leaf(RichText::new("hello          world").font_size(16.0).white_space(ws));
        }
    };

    let (mut world_normal, root_normal) = setup();
    run_view(&mut world_normal, root_normal, build_with_white_space(matcha_ecs_widgets::WhiteSpace::Normal));
    layout_root(&mut world_normal, root_normal, Constraints::from_max_size([2000.0, 600.0]));
    let normal = output(&world_normal, first_child(&world_normal, root_normal));

    let (mut world_pre, root_pre) = setup();
    run_view(&mut world_pre, root_pre, build_with_white_space(matcha_ecs_widgets::WhiteSpace::Pre));
    layout_root(&mut world_pre, root_pre, Constraints::from_max_size([2000.0, 600.0]));
    let pre = output(&world_pre, first_child(&world_pre, root_pre));

    assert!(
        normal.size[0] < pre.size[0],
        "collapsing the run of spaces must measure narrower than preserving them: normal={:?} pre={:?}",
        normal,
        pre
    );
}

#[test]
fn enter_fade_builder_starts_from_transparent() {
    use matcha_ecs::animation::{Animated, Opacity};

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(RichText::new("hello").enter_fade(std::time::Duration::from_millis(200), Easing::Linear));
    });
    let child = first_child(&world, root);
    let animated = world.get::<Animated<Opacity>>(child).expect("Animated<Opacity> present");
    assert_eq!(animated.0, Opacity(0.0));
}
