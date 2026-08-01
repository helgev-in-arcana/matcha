//! CSS-style box sizing: [`Length`], [`Sizing`], and the pure helpers that
//! resolve them.
//!
//! A widget that wants CSS `width`/`height`/`min-*`/`max-*`/`aspect-ratio`
//! holds a [`Sizing`] and calls two functions from its `Layout::measure`:
//! [`Sizing::content_constraints`] to decide what to measure its content
//! against, then [`Sizing::measured`] to turn the content's own report into its
//! own. The builders come from the [`sizing_builders!`](crate::sizing_builders)
//! macro so every sized widget spells them the same way.
//!
//! Everything here is a pure function of plain numbers — no ECS, no GPU — so
//! it is unit-testable directly and would survive a backend change. The only
//! ECS-facing part is `Sizing` being a `Component`.

use bevy_ecs::component::Component;

use matcha_ecs::layout::{Constraints, Measured};

/// One CSS length.
///
/// `Percent` is a percentage (`Percent(50.0)` is half), resolved against the
/// space the parent offered on that axis. A percentage or [`Fill`](Self::Fill)
/// against an *indefinite* basis — an axis measured at
/// [`Constraints::UNBOUNDED`], which is what a scrolling container does — has
/// nothing to be a fraction of, and falls back to `Auto`.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Length {
    /// Content-driven: whatever the content measures to.
    #[default]
    Auto,
    /// A fixed number of pixels.
    Px(f32),
    /// A percentage of the space offered on this axis.
    Percent(f32),
    /// All of the space offered on this axis.
    ///
    /// Distinct from `Percent(100.0)` in what it means to a *distributing*
    /// parent: `Fill` is the one flex distribution understands as "grow into
    /// the leftover", so two `Fill` siblings on a row's main axis will
    /// eventually split the row between them, whereas two `Percent(100.0)`
    /// siblings each claim the whole row and overflow. That distribution
    /// itself is not implemented yet, so today they resolve alike except that
    /// `Fill` still reports its content size as its minimum.
    Fill,
}

impl Length {
    /// The definite size this resolves to against `available`, or `None` when
    /// it is content-driven.
    pub fn definite(self, available: f32) -> Option<f32> {
        // An unbounded axis is offering "as much as you like", which no
        // fraction of is meaningful.
        let indefinite = available >= Constraints::UNBOUNDED;
        match self {
            Length::Auto => None,
            Length::Px(v) => Some(v.max(0.0)),
            Length::Percent(p) if !indefinite => Some((available * p / 100.0).max(0.0)),
            Length::Percent(_) => None,
            Length::Fill if !indefinite => Some(available),
            Length::Fill => None,
        }
    }
}

/// The CSS sizing properties of one box. All-`Auto` (the default) is exactly
/// the fit-content behaviour every widget had before this type existed.
#[derive(Component, Clone, Copy, PartialEq, Debug, Default)]
pub struct Sizing {
    pub width: Length,
    pub height: Length,
    /// `Auto` means no lower bound of its own.
    pub min_width: Length,
    pub min_height: Length,
    /// `Auto` means no upper bound of its own (CSS's `max-width: none`).
    pub max_width: Length,
    pub max_height: Length,
    /// CSS `aspect-ratio`, as width ÷ height. Applies only when exactly one
    /// axis is definite; the other is then derived from it.
    pub aspect_ratio: Option<f32>,
}

impl Sizing {
    /// The definite size of each axis, `None` where it is content-driven.
    fn definite(&self, c: Constraints) -> [Option<f32>; 2] {
        let mut w = self.width.definite(c.max_width());
        let mut h = self.height.definite(c.max_height());
        if let Some(ratio) = self.aspect_ratio.filter(|r| *r > 0.0) {
            match (w, h) {
                (Some(w), None) => h = Some(w / ratio),
                (None, Some(h)) => w = Some(h * ratio),
                _ => {}
            }
        }
        [w, h]
    }

    /// This box's own lower and upper bounds per axis, from `min-*`/`max-*`.
    fn bounds(&self, c: Constraints) -> ([f32; 2], [f32; 2]) {
        let lo = [
            self.min_width.definite(c.max_width()).unwrap_or(0.0),
            self.min_height.definite(c.max_height()).unwrap_or(0.0),
        ];
        let hi = [
            self.max_width
                .definite(c.max_width())
                .unwrap_or(f32::INFINITY),
            self.max_height
                .definite(c.max_height())
                .unwrap_or(f32::INFINITY),
        ];
        (lo, hi)
    }

    /// What to measure this box's content (children, text) against.
    ///
    /// A definite axis offers the content up to the resolved size; a
    /// content-driven one passes the incoming maximum straight through. Note
    /// the minimum is dropped on both: pinning *this box* to a size says
    /// nothing about how small its content may be.
    pub fn content_constraints(&self, c: Constraints) -> Constraints {
        let definite = self.definite(c);
        Constraints::new(
            [0.0, definite[0].unwrap_or_else(|| c.max_width())],
            [0.0, definite[1].unwrap_or_else(|| c.max_height())],
        )
    }

    /// This box's report to its parent, given what its content measured to
    /// under [`content_constraints`](Self::content_constraints).
    pub fn measured(&self, c: Constraints, content: Measured) -> Measured {
        let definite = self.definite(c);
        let (bound_lo, bound_hi) = self.bounds(c);
        let incoming_lo = [c.min_width(), c.min_height()];
        let incoming_hi = [c.max_width(), c.max_height()];

        let mut min = [0.0; 2];
        let mut preferred = [0.0; 2];
        let mut max = [0.0; 2];

        for axis in 0..2 {
            let (mut lo, mut pref, mut hi) = match definite[axis] {
                // `Fill` wants everything offered but can still report how
                // little it could live with, which is what lets a future flex
                // pass shrink it.
                Some(size) if self.axis_length(axis) == Length::Fill => {
                    (content.min[axis], size, size)
                }
                // An explicit width/height is not negotiable.
                Some(size) => (size, size, size),
                None => (
                    content.min[axis],
                    content.preferred[axis],
                    content.max[axis],
                ),
            };

            let clamp = |v: f32| v.clamp(bound_lo[axis], bound_hi[axis].max(bound_lo[axis]));
            lo = clamp(lo);
            pref = clamp(pref);
            hi = clamp(hi);

            // A definite size overflows its parent rather than being squeezed
            // by it — CSS's rule, and what makes an explicit width win over a
            // stretching parent. A content-driven axis honours what it was
            // offered.
            if definite[axis].is_none() {
                lo = lo.clamp(incoming_lo[axis], incoming_hi[axis].max(incoming_lo[axis]));
                pref = pref.clamp(incoming_lo[axis], incoming_hi[axis].max(incoming_lo[axis]));
                hi = hi.clamp(incoming_lo[axis], incoming_hi[axis].max(incoming_lo[axis]));
            }

            // Keep `min <= preferred <= max` whatever the inputs were: content
            // larger than the space offered would otherwise invert the range.
            min[axis] = lo.min(pref);
            preferred[axis] = pref;
            max[axis] = hi.max(pref);
        }

        Measured::new(min, preferred, max)
    }

    /// [`measured`](Self::measured) keeping only the resolved size.
    pub fn resolve(&self, c: Constraints, content: [f32; 2]) -> [f32; 2] {
        self.measured(c, Measured::exact(content)).preferred
    }

    fn axis_length(&self, axis: usize) -> Length {
        if axis == 0 {
            self.width
        } else {
            self.height
        }
    }
}

/// Emits the CSS sizing builders for a widget with a `sizing: Sizing` field.
///
/// Every sized widget spells these the same way, and there are seven of them,
/// so they are generated rather than retyped. Deliberately does *not* emit a
/// `size(w, h)` builder — several widgets already have one with their own
/// meaning.
#[macro_export]
macro_rules! sizing_builders {
    () => {
        /// CSS `width`.
        pub fn width(mut self, v: $crate::sizing::Length) -> Self {
            self.sizing.width = v;
            self
        }

        /// CSS `height`.
        pub fn height(mut self, v: $crate::sizing::Length) -> Self {
            self.sizing.height = v;
            self
        }

        /// CSS `min-width`.
        pub fn min_width(mut self, v: $crate::sizing::Length) -> Self {
            self.sizing.min_width = v;
            self
        }

        /// CSS `min-height`.
        pub fn min_height(mut self, v: $crate::sizing::Length) -> Self {
            self.sizing.min_height = v;
            self
        }

        /// CSS `max-width`.
        pub fn max_width(mut self, v: $crate::sizing::Length) -> Self {
            self.sizing.max_width = v;
            self
        }

        /// CSS `max-height`.
        pub fn max_height(mut self, v: $crate::sizing::Length) -> Self {
            self.sizing.max_height = v;
            self
        }

        /// CSS `aspect-ratio`, as width ÷ height. Applies to whichever axis is
        /// left `Auto` when the other is definite.
        pub fn aspect_ratio(mut self, ratio: f32) -> Self {
            self.sizing.aspect_ratio = Some(ratio);
            self
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered(w: f32, h: f32) -> Constraints {
        Constraints::from_max_size([w, h])
    }

    #[test]
    fn all_auto_is_exactly_the_content_size() {
        let s = Sizing::default();
        assert_eq!(s.resolve(offered(800.0, 600.0), [50.0, 20.0]), [50.0, 20.0]);
    }

    #[test]
    fn auto_still_cannot_exceed_the_space_offered() {
        let s = Sizing::default();
        assert_eq!(s.resolve(offered(40.0, 600.0), [50.0, 20.0]), [40.0, 20.0]);
    }

    #[test]
    fn fill_takes_the_whole_offered_axis() {
        let s = Sizing {
            height: Length::Fill,
            ..Default::default()
        };
        assert_eq!(s.resolve(offered(800.0, 600.0), [50.0, 20.0]), [50.0, 600.0]);
    }

    #[test]
    fn fill_reports_its_content_as_the_minimum_it_could_shrink_to() {
        let s = Sizing {
            height: Length::Fill,
            ..Default::default()
        };
        let m = s.measured(offered(800.0, 600.0), Measured::exact([50.0, 20.0]));
        assert_eq!(m.min[1], 20.0);
        assert_eq!(m.preferred[1], 600.0);
        assert_eq!(m.max[1], 600.0);
    }

    #[test]
    fn fill_on_an_unbounded_axis_falls_back_to_the_content_size() {
        // What a scrolling container offers on the axis it scrolls: there is
        // no "all of it" to take.
        let s = Sizing {
            height: Length::Fill,
            ..Default::default()
        };
        let c = Constraints::new([0.0, 800.0], [0.0, Constraints::UNBOUNDED]);
        assert_eq!(s.resolve(c, [50.0, 20.0]), [50.0, 20.0]);
    }

    #[test]
    fn percent_is_a_fraction_of_the_space_offered() {
        let s = Sizing {
            width: Length::Percent(25.0),
            ..Default::default()
        };
        assert_eq!(s.resolve(offered(800.0, 600.0), [50.0, 20.0]), [200.0, 20.0]);
    }

    #[test]
    fn an_explicit_size_overflows_its_parent_rather_than_being_squeezed() {
        // CSS's rule, and the reason an explicit width beats a stretching
        // parent: the incoming maximum does not clamp a definite size.
        let s = Sizing {
            width: Length::Px(500.0),
            ..Default::default()
        };
        assert_eq!(s.resolve(offered(100.0, 600.0), [50.0, 20.0]), [500.0, 20.0]);
    }

    #[test]
    fn an_explicit_size_also_beats_a_parent_pinning_min_equal_to_max() {
        // A `Column` under `AlignItems::Stretch` pins its children this way.
        let s = Sizing {
            width: Length::Px(30.0),
            ..Default::default()
        };
        let c = Constraints::new([200.0, 200.0], [0.0, 600.0]);
        assert_eq!(s.resolve(c, [50.0, 20.0])[0], 30.0);
    }

    #[test]
    fn min_and_max_bounds_clamp_the_resolved_size() {
        let s = Sizing {
            min_width: Length::Px(80.0),
            max_height: Length::Px(10.0),
            ..Default::default()
        };
        assert_eq!(s.resolve(offered(800.0, 600.0), [50.0, 20.0]), [80.0, 10.0]);
    }

    #[test]
    fn aspect_ratio_derives_the_axis_left_auto() {
        let s = Sizing {
            width: Length::Px(300.0),
            aspect_ratio: Some(3.0 / 2.0),
            ..Default::default()
        };
        assert_eq!(s.resolve(offered(800.0, 600.0), [50.0, 20.0]), [300.0, 200.0]);
    }

    #[test]
    fn aspect_ratio_is_ignored_when_both_axes_are_already_definite() {
        let s = Sizing {
            width: Length::Px(300.0),
            height: Length::Px(300.0),
            aspect_ratio: Some(3.0 / 2.0),
            ..Default::default()
        };
        assert_eq!(s.resolve(offered(800.0, 600.0), [50.0, 20.0]), [300.0, 300.0]);
    }

    #[test]
    fn content_is_offered_the_resolved_size_not_the_parents() {
        let s = Sizing {
            width: Length::Px(300.0),
            ..Default::default()
        };
        let inner = s.content_constraints(offered(800.0, 600.0));
        assert_eq!(inner.max_width(), 300.0);
        // And nothing forces the content to actually be that wide.
        assert_eq!(inner.min_width(), 0.0);
        assert_eq!(inner.max_height(), 600.0);
    }
}
