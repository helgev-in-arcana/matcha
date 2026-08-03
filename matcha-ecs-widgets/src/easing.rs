//! [`Easing`] — the shape a `0.0..=1.0` progress is bent into before it is
//! interpolated.
//!
//! Its own module because more than one thing animates. Opacity fades
//! ([`crate::animation`]) and interaction colour transitions
//! ([`crate::interaction`]) are separate, independently registrable plugins,
//! and the curve is not the property of either.

/// An easing curve applied to the raw `0.0..=1.0` tween progress before
/// interpolating.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Easing {
    Linear,
    EaseInOut,
}

impl Easing {
    pub fn apply(self, t: f32) -> f32 {
        match self {
            Easing::Linear => t,
            Easing::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}
