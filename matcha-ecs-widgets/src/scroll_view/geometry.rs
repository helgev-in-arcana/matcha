//! Scrollbar geometry — **pure functions over plain numbers**.
//!
//! Nothing here knows about ECS, `bevy_ecs`, `wgpu` or `matcha-ecs`. Given the
//! three numbers that describe a scroll — how long the content is, how much of
//! it is on screen, and where we currently are — plus a style, these produce
//! the rectangles a scrollbar is made of, and the inverse map a thumb drag
//! needs.
//!
//! Keeping this layer free of the backend is deliberate: it is the part of a
//! scrollbar that is genuinely *about* scrolling, and it should survive
//! unchanged if the widget layer is ever rebuilt on something other than an
//! ECS. Everything in `super` is a thin adapter over what is here.

/// Which axis a scrollbar runs along.
///
/// `X` is the horizontal bar (it runs along the bottom edge); `Y` is the
/// vertical one (along the right edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
}

impl Axis {
    /// Index into an `[f32; 2]` for this axis.
    pub const fn main(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
        }
    }

    /// Index into an `[f32; 2]` for the perpendicular axis.
    pub const fn cross(self) -> usize {
        1 - self.main()
    }

    pub const fn other(self) -> Axis {
        match self {
            Axis::X => Axis::Y,
            Axis::Y => Axis::X,
        }
    }
}

/// The whole state of a scroll, on both axes.
///
/// This is the "model" a scrollbar is a function of: the length of the content,
/// the length that is visible, and where we are within it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScrollGeometry {
    /// Size of the visible window.
    pub viewport: [f32; 2],
    /// Size of the content laid out inside it.
    pub content: [f32; 2],
    /// How far the content is shifted up/left, in pixels. Always `>= 0` once
    /// [`Self::clamped_offset`] has been applied.
    pub offset: [f32; 2],
}

impl ScrollGeometry {
    /// The largest meaningful offset per axis — how much content is off screen.
    pub fn max_offset(&self) -> [f32; 2] {
        [
            (self.content[0] - self.viewport[0]).max(0.0),
            (self.content[1] - self.viewport[1]).max(0.0),
        ]
    }

    /// `offset` restricted to `0..=max_offset`, which is the only range that
    /// shows content.
    pub fn clamped_offset(&self) -> [f32; 2] {
        let max = self.max_offset();
        [
            self.offset[0].clamp(0.0, max[0]),
            self.offset[1].clamp(0.0, max[1]),
        ]
    }

    /// Whether the content overflows on `axis` by enough to be worth scrolling.
    pub fn overflows(&self, axis: Axis) -> bool {
        self.max_offset()[axis.main()] > EPSILON
    }
}

/// Sub-pixel slack. Below this, an "overflow" is float noise from a layout that
/// in fact fits exactly, and showing a scrollbar for it would be wrong.
const EPSILON: f32 = 0.01;

/// How a scrollbar looks. Purely visual — none of it affects layout, because
/// bars are drawn as an overlay and never take space from the content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarStyle {
    /// Thickness of the bar across its short axis.
    pub thickness: f32,
    /// Margin between the bar and the viewport's edges.
    pub gap: f32,
    /// Corner radius of the track and the thumb.
    pub radius: f32,
    pub thumb_color: [f32; 4],
    /// `None` draws no track, leaving just the thumb floating over the content.
    pub track_color: Option<[f32; 4]>,
    /// Floor on the thumb's length, so a very long content still leaves
    /// something grabbable.
    pub min_thumb_len: f32,
}

impl Default for ScrollbarStyle {
    fn default() -> Self {
        Self {
            thickness: 8.0,
            gap: 2.0,
            radius: 4.0,
            thumb_color: [0.62, 0.62, 0.68, 0.75],
            track_color: Some([0.0, 0.0, 0.0, 0.18]),
            min_thumb_len: 24.0,
        }
    }
}

/// Where a scrollbar's parts sit, in the viewport's own coordinates
/// (origin at its top-left corner). Both rects are `[x, y, w, h]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarMetrics {
    pub track: [f32; 4],
    pub thumb: [f32; 4],
}

/// The intermediate values `metrics` and `offset_for_thumb_start` both need,
/// resolved once so the forward and inverse maps cannot drift apart.
struct Resolved {
    /// Start of the track along the scrolling axis.
    track_start: f32,
    track_len: f32,
    thumb_len: f32,
    /// Position of the track across the scrolling axis.
    cross_start: f32,
    max_offset: f32,
}

fn resolve(
    axis: Axis,
    geom: &ScrollGeometry,
    scrollable: [bool; 2],
    style: &ScrollbarStyle,
) -> Option<Resolved> {
    let (main, cross) = (axis.main(), axis.cross());

    if !scrollable[main] || !geom.overflows(axis) {
        return None;
    }

    // When the other bar is showing too, stop short of it so the two do not
    // overlap in the corner.
    let other = axis.other();
    let other_showing = scrollable[other.main()] && geom.overflows(other);
    let corner = if other_showing {
        style.thickness + style.gap
    } else {
        0.0
    };

    let track_len = geom.viewport[main] - style.gap * 2.0 - corner;
    if track_len <= 0.0 {
        return None;
    }

    // Proportional to how much of the content is on screen, which is what makes
    // the thumb read as "how much of this am I seeing".
    let visible_ratio = (geom.viewport[main] / geom.content[main]).clamp(0.0, 1.0);
    let thumb_len = (track_len * visible_ratio)
        .max(style.min_thumb_len)
        .min(track_len);

    Some(Resolved {
        track_start: style.gap,
        track_len,
        thumb_len,
        cross_start: geom.viewport[cross] - style.gap - style.thickness,
        max_offset: geom.max_offset()[main],
    })
}

/// Build a `[x, y, w, h]` rect from along-axis and across-axis extents.
fn rect(axis: Axis, main_start: f32, main_len: f32, cross_start: f32, cross_len: f32) -> [f32; 4] {
    match axis {
        Axis::X => [main_start, cross_start, main_len, cross_len],
        Axis::Y => [cross_start, main_start, cross_len, main_len],
    }
}

/// The rectangles to draw for `axis`, or `None` when no bar belongs there —
/// either the axis does not scroll, or the content already fits.
///
/// `scrollable` says which axes are allowed to scroll at all (an
/// `Overflow::Hidden` axis clips without ever showing a bar).
pub fn metrics(
    axis: Axis,
    geom: &ScrollGeometry,
    scrollable: [bool; 2],
    style: &ScrollbarStyle,
) -> Option<ScrollbarMetrics> {
    let r = resolve(axis, geom, scrollable, style)?;

    let progress = if r.max_offset > 0.0 {
        (geom.clamped_offset()[axis.main()] / r.max_offset).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let thumb_start = r.track_start + progress * (r.track_len - r.thumb_len);

    Some(ScrollbarMetrics {
        track: rect(
            axis,
            r.track_start,
            r.track_len,
            r.cross_start,
            style.thickness,
        ),
        thumb: rect(axis, thumb_start, r.thumb_len, r.cross_start, style.thickness),
    })
}

/// Inverse of the thumb placement in [`metrics`]: given where the thumb's
/// leading edge has been dragged to, the offset that puts it there.
///
/// Returns `0.0` when the axis has no bar, so a stray drag cannot scroll
/// something that does not scroll.
pub fn offset_for_thumb_start(
    axis: Axis,
    geom: &ScrollGeometry,
    scrollable: [bool; 2],
    thumb_start: f32,
    style: &ScrollbarStyle,
) -> f32 {
    let Some(r) = resolve(axis, geom, scrollable, style) else {
        return 0.0;
    };

    // A thumb that fills its track (or is held at `min_thumb_len` on a track
    // that short) has nowhere to travel, so every position means offset 0.
    let travel = r.track_len - r.thumb_len;
    if travel <= 0.0 {
        return 0.0;
    }

    let progress = ((thumb_start - r.track_start) / travel).clamp(0.0, 1.0);
    progress * r.max_offset
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [bool; 2] = [true, true];

    /// 100-tall viewport over 400-tall content, scrolled to `offset`.
    fn tall(offset: f32) -> ScrollGeometry {
        ScrollGeometry {
            viewport: [200.0, 100.0],
            content: [200.0, 400.0],
            offset: [0.0, offset],
        }
    }

    fn style() -> ScrollbarStyle {
        ScrollbarStyle {
            thickness: 10.0,
            gap: 0.0,
            radius: 0.0,
            min_thumb_len: 0.0,
            ..Default::default()
        }
    }

    #[test]
    fn no_bar_when_the_content_fits() {
        let geom = ScrollGeometry {
            viewport: [200.0, 100.0],
            content: [200.0, 100.0],
            offset: [0.0, 0.0],
        };
        assert!(metrics(Axis::Y, &geom, BOTH, &style()).is_none());
    }

    #[test]
    fn no_bar_on_an_axis_that_is_not_scrollable() {
        // Content overflows vertically, but the Y axis is `Overflow::Hidden`.
        assert!(metrics(Axis::Y, &tall(0.0), [true, false], &style()).is_none());
    }

    #[test]
    fn thumb_length_is_proportional_to_the_visible_fraction() {
        let m = metrics(Axis::Y, &tall(0.0), BOTH, &style()).expect("bar");
        // A quarter of the content is on screen, over a 100px track.
        assert_eq!(m.track[3], 100.0);
        assert_eq!(m.thumb[3], 25.0);
    }

    #[test]
    fn the_thumb_spans_the_track_from_start_to_end() {
        let s = style();
        let at_top = metrics(Axis::Y, &tall(0.0), BOTH, &s).expect("bar");
        assert_eq!(at_top.thumb[1], 0.0);

        // max_offset is 300; fully scrolled puts the thumb's far edge on the
        // track's far edge.
        let at_bottom = metrics(Axis::Y, &tall(300.0), BOTH, &s).expect("bar");
        assert_eq!(at_bottom.thumb[1] + at_bottom.thumb[3], 100.0);

        let midway = metrics(Axis::Y, &tall(150.0), BOTH, &s).expect("bar");
        assert_eq!(midway.thumb[1], 37.5);
    }

    #[test]
    fn an_out_of_range_offset_is_clamped_rather_than_running_off_the_track() {
        let s = style();
        let past_end = metrics(Axis::Y, &tall(9999.0), BOTH, &s).expect("bar");
        assert_eq!(past_end.thumb[1] + past_end.thumb[3], 100.0);

        let before_start = metrics(Axis::Y, &tall(-50.0), BOTH, &s).expect("bar");
        assert_eq!(before_start.thumb[1], 0.0);
    }

    #[test]
    fn min_thumb_len_floors_a_thumb_that_would_be_unusably_short() {
        let s = ScrollbarStyle {
            min_thumb_len: 40.0,
            ..style()
        };
        let geom = ScrollGeometry {
            viewport: [200.0, 100.0],
            content: [200.0, 10_000.0],
            offset: [0.0, 0.0],
        };
        let m = metrics(Axis::Y, &geom, BOTH, &s).expect("bar");
        assert_eq!(m.thumb[3], 40.0);

        // Still reaches the end of the track despite the floor.
        let end = metrics(
            Axis::Y,
            &ScrollGeometry {
                offset: [0.0, 9900.0],
                ..geom
            },
            BOTH,
            &s,
        )
        .expect("bar");
        assert_eq!(end.thumb[1] + end.thumb[3], 100.0);
    }

    #[test]
    fn offset_and_thumb_position_round_trip() {
        let s = style();
        for offset in [0.0, 1.0, 73.5, 150.0, 299.0, 300.0] {
            let geom = tall(offset);
            let m = metrics(Axis::Y, &geom, BOTH, &s).expect("bar");
            let back = offset_for_thumb_start(Axis::Y, &geom, BOTH, m.thumb[1], &s);
            assert!(
                (back - offset).abs() < 0.001,
                "offset {offset} -> thumb {} -> {back}",
                m.thumb[1]
            );
        }
    }

    #[test]
    fn dragging_the_thumb_past_either_end_saturates() {
        let s = style();
        let geom = tall(0.0);
        assert_eq!(
            offset_for_thumb_start(Axis::Y, &geom, BOTH, -500.0, &s),
            0.0
        );
        assert_eq!(
            offset_for_thumb_start(Axis::Y, &geom, BOTH, 500.0, &s),
            300.0
        );
    }

    #[test]
    fn the_two_axes_are_mirror_images() {
        let s = style();
        let vertical = metrics(Axis::Y, &tall(150.0), BOTH, &s).expect("bar");
        // Same numbers with the axes swapped.
        let horizontal = metrics(
            Axis::X,
            &ScrollGeometry {
                viewport: [100.0, 200.0],
                content: [400.0, 200.0],
                offset: [150.0, 0.0],
            },
            BOTH,
            &s,
        )
        .expect("bar");

        assert_eq!(horizontal.track, [vertical.track[1], vertical.track[0], vertical.track[3], vertical.track[2]]);
        assert_eq!(horizontal.thumb, [vertical.thumb[1], vertical.thumb[0], vertical.thumb[3], vertical.thumb[2]]);
    }

    #[test]
    fn the_bars_stop_short_of_each_other_when_both_are_showing() {
        let s = style();
        let one_axis = ScrollGeometry {
            viewport: [200.0, 100.0],
            content: [200.0, 400.0],
            offset: [0.0, 0.0],
        };
        let both_axes = ScrollGeometry {
            content: [400.0, 400.0],
            ..one_axis
        };

        let alone = metrics(Axis::Y, &one_axis, BOTH, &s).expect("bar");
        let shared = metrics(Axis::Y, &both_axes, BOTH, &s).expect("bar");
        // Shortened by the other bar's thickness (+ gap, which is 0 here).
        assert_eq!(alone.track[3], 100.0);
        assert_eq!(shared.track[3], 90.0);
    }

    #[test]
    fn the_bar_sits_against_the_far_edge_inside_the_gap() {
        let s = ScrollbarStyle {
            thickness: 10.0,
            gap: 3.0,
            ..style()
        };
        let m = metrics(Axis::Y, &tall(0.0), BOTH, &s).expect("bar");
        // Right edge: viewport width 200 - gap 3 - thickness 10.
        assert_eq!(m.track[0], 187.0);
        assert_eq!(m.track[2], 10.0);
        assert_eq!(m.track[1], 3.0);
        assert_eq!(m.track[3], 94.0);
    }

    #[test]
    fn a_viewport_too_small_for_a_track_gets_no_bar() {
        let s = ScrollbarStyle { gap: 60.0, ..style() };
        assert!(metrics(Axis::Y, &tall(0.0), BOTH, &s).is_none());
    }

    #[test]
    fn clamped_offset_and_max_offset_agree_with_the_content() {
        let geom = ScrollGeometry {
            viewport: [200.0, 100.0],
            content: [150.0, 400.0],
            offset: [50.0, -20.0],
        };
        // Content narrower than the viewport cannot scroll horizontally.
        assert_eq!(geom.max_offset(), [0.0, 300.0]);
        assert_eq!(geom.clamped_offset(), [0.0, 0.0]);
        assert!(!geom.overflows(Axis::X));
        assert!(geom.overflows(Axis::Y));
    }
}
