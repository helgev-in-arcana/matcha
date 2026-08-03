use crate::types::size::Size;

/// Defines a size that represents either a fixed track (evaluated to pixels)
/// or a flexible track that participates in distributing remaining space.
///
/// - `Fixed(Size)`:
///   Evaluated as an actual size function (pixels). Use for tracks with concrete sizes
///   (e.g., px, vw, or functions that resolve to pixel values).
///
/// - `Grow(Size)`:
///   Represents a *weight* for proportional distribution of the leftover space after
///   fixed tracks and gaps are accounted for. The contained `Size` MUST be a function
///   that returns a non-negative scalar interpreted as the weight (not pixels).
///   During layout, the algorithm computes:
///     1. sum_fixed = sum of fixed track sizes (pixels)
///     2. sum_weights = sum of weights returned by all `Grow` tracks
///     3. available = parent_size - sum_fixed - total_gaps
///     4. per_weight_px = max(0, available / sum_weights)
///     5. track_px = per_weight_px * weight_for_track
///
/// Note: `Grow(Size::px(w))` is a convenient way to create a `Grow` with weight `w`.
#[derive(Clone, PartialEq)]
pub enum GrowSize {
    /// A fixed size determined by a `Size` function.
    Fixed(Size),
    /// A flexible size that grows in proportion to other `Grow` tracks; the inner `Size`
    /// function must return a scalar weight used by the layout algorithm (see docs).
    Grow(Size),
}
