//! Values a `RenderItem` builder reads *after* it was captured.
//!
//! # Why this exists
//!
//! A widget's `RenderItem` builder is a closure captured at `bundle()` or
//! `patch()` time. It runs on the render thread, with no `World` and no `&self`
//! — so there are exactly three ways a value can reach it:
//!
//! 1. a [`RenderCtx`](matcha_ecs::components::render::RenderCtx) field, for
//!    things the core knows about (size, opacity, focus, hover);
//! 2. **a shared cell the closure captured**, for things it does not;
//! 3. rebuilding the closure, which costs an invalidation and a rasterisation.
//!
//! This module is (2). A wrap width settled by `arrange`, a scroll offset moved
//! by a drag, a caret's blink phase, a colour mid-transition: all are written
//! by something running in the schedule and read by a builder that was captured
//! long before, and none of them should force a rebuild.
//!
//! # What they are
//!
//! `f32` bit-cast into an `AtomicU32`, `Relaxed`. Relaxed is right because each
//! cell is independent — nothing here orders one write against another, and a
//! builder reading a value one frame stale draws one frame stale, which is
//! already true of everything else it sees.
//!
//! The cells are **not** `Arc`-wrapped: some callers hold several behind one
//! shared `Arc<Inner>`, others want a single standalone cell and write
//! `Arc<LiveF32>`. Wrapping here would force the first group to pay for an
//! allocation per field.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// One shared `f32`.
#[derive(Debug, Default)]
pub struct LiveF32(AtomicU32);

impl LiveF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }

    /// Store `v`, reporting whether it differed from what was there.
    ///
    /// Bitwise, so `NaN` and `-0.0` compare as their representations rather
    /// than by `PartialEq` — which is what a "do I need to redraw?" caller
    /// wants, since it is asking whether the bits a builder reads changed.
    pub fn replace(&self, v: f32) -> bool {
        let bits = v.to_bits();
        self.0.swap(bits, Ordering::Relaxed) != bits
    }
}

/// One shared `bool`.
#[derive(Debug, Default)]
pub struct LiveBool(AtomicBool);

impl LiveBool {
    pub fn new(v: bool) -> Self {
        Self(AtomicBool::new(v))
    }

    pub fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Store `v`, reporting whether it differed from what was there.
    pub fn replace(&self, v: bool) -> bool {
        self.0.swap(v, Ordering::Relaxed) != v
    }
}

/// A shared fixed-length array of `f32` — a size, an offset, an RGBA colour.
///
/// The elements are updated one at a time, so a reader can in principle catch
/// a half-written pair. Harmless here: every consumer is a builder painting a
/// frame, and the worst case is one frame drawn with a mixed offset before the
/// next arrives.
#[derive(Debug)]
pub struct LiveVec<const N: usize>([AtomicU32; N]);

// Hand-written: `[T; N]` implements `Default` only for the small fixed lengths
// the standard library spells out, not for a const-generic `N`.
impl<const N: usize> Default for LiveVec<N> {
    fn default() -> Self {
        Self::new([0.0; N])
    }
}

impl<const N: usize> LiveVec<N> {
    pub fn new(v: [f32; N]) -> Self {
        Self(v.map(|c| AtomicU32::new(c.to_bits())))
    }

    pub fn get(&self) -> [f32; N] {
        std::array::from_fn(|i| f32::from_bits(self.0[i].load(Ordering::Relaxed)))
    }

    pub fn set(&self, v: [f32; N]) {
        for (slot, c) in self.0.iter().zip(v) {
            slot.store(c.to_bits(), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_float_round_trips_through_its_bits() {
        let cell = LiveF32::new(12.5);
        assert_eq!(cell.get(), 12.5);
        cell.set(-3.25);
        assert_eq!(cell.get(), -3.25);
    }

    #[test]
    fn replace_reports_only_a_real_change() {
        let cell = LiveF32::new(1.0);
        assert!(!cell.replace(1.0), "the same value is not a change");
        assert!(cell.replace(2.0));
    }

    #[test]
    fn f32_max_survives_the_round_trip() {
        // Text wrap widths start at f32::MAX, meaning "effectively no wrap".
        let cell = LiveF32::new(f32::MAX);
        assert_eq!(cell.get(), f32::MAX);
    }

    #[test]
    fn a_vec_round_trips_every_element() {
        let cell = LiveVec::new([0.1, 0.2, 0.3, 1.0]);
        assert_eq!(cell.get(), [0.1, 0.2, 0.3, 1.0]);
        cell.set([1.0, 0.0, 0.5, 0.25]);
        assert_eq!(cell.get(), [1.0, 0.0, 0.5, 0.25]);
    }

    #[test]
    fn a_default_vec_is_zeroed() {
        assert_eq!(LiveVec::<2>::default().get(), [0.0, 0.0]);
    }
}
