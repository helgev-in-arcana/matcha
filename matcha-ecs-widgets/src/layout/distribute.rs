//! Distributing a container's leftover (or overflow) main-axis space across
//! its children, by [`grow`](crate::sizing::Sizing::grow) and
//! [`shrink`](crate::sizing::Sizing::shrink) weights.
//!
//! Pure functions over plain numbers: no ECS, no widgets, no GPU. This is
//! CSS's "resolve flexible lengths" (CSS Flexbox §9.7) with the parts that
//! only exist to serve `flex-basis` left out — a box's base size here is
//! whatever it measured to, since `width`/`height` already say what a basis
//! would have said.

/// One box's inputs to a distribution.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Item {
    /// The size it measured to before any distribution.
    pub base: f32,
    /// It will not be shrunk below this. Callers pass the declared
    /// `min-width`/`min-height`, or the min-content size where that is `Auto`.
    pub min: f32,
    /// It will not be grown beyond this. `f32::INFINITY` where no maximum is
    /// declared — growth is bounded by an explicit maximum, never by the
    /// content's own max-content size.
    pub max: f32,
    pub grow: f32,
    pub shrink: f32,
}

impl Item {
    /// A box that neither grows nor shrinks: it keeps `base`.
    pub fn fixed(base: f32) -> Self {
        Self {
            base,
            min: base,
            max: base,
            grow: 0.0,
            shrink: 0.0,
        }
    }
}

/// Sizes within a hundredth of a pixel of each other are the same size; the
/// quantizer this codebase's `Constraints` uses is coarser than that anyway.
const EPSILON: f32 = 0.01;

/// Resolve each item's main-axis size so they total `available` where the
/// weights allow it.
///
/// Returns `base` clamped to `[min, max]` for every item when nothing can
/// flex, so a caller never has to special-case "no weights set" — which is
/// the overwhelmingly common case, every container whose children all left
/// `grow`/`shrink` alone and already fit.
pub fn distribute(items: &[Item], available: f32) -> Vec<f32> {
    let mut sizes: Vec<f32> = items
        .iter()
        .map(|i| i.base.clamp(i.min, i.max.max(i.min)))
        .collect();
    let mut frozen: Vec<bool> = vec![false; items.len()];

    // Each pass freezes at least one item at a bound, or finishes; without a
    // violated bound the first pass already lands exactly on `available`.
    for _ in 0..=items.len() {
        let used: f32 = sizes.iter().sum();
        let free = available - used;
        if free.abs() < EPSILON {
            break;
        }
        let growing = free > 0.0;

        // CSS weights shrinking by the item's own size, so a large box gives
        // back more than a small one at the same shrink factor; growing is
        // weighted by the factor alone.
        let weight = |i: usize| {
            if growing {
                items[i].grow
            } else {
                items[i].shrink * items[i].base
            }
        };

        let total: f32 = (0..items.len())
            .filter(|&i| !frozen[i])
            .map(weight)
            .sum();
        if total <= 0.0 {
            break;
        }

        let mut froze_any = false;
        for i in 0..items.len() {
            if frozen[i] {
                continue;
            }
            let target = sizes[i] + free * weight(i) / total;
            let clamped = target.clamp(items[i].min, items[i].max.max(items[i].min));
            if (clamped - target).abs() >= EPSILON {
                // It hit a bound: pin it there and let the next pass share
                // what it could not take among the rest.
                frozen[i] = true;
                froze_any = true;
            }
            sizes[i] = clamped;
        }

        if !froze_any {
            break;
        }
    }

    sizes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flexible(base: f32, grow: f32, shrink: f32) -> Item {
        Item {
            base,
            min: 0.0,
            max: f32::INFINITY,
            grow,
            shrink,
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.05
    }

    #[test]
    fn nothing_flexes_when_no_weights_are_set() {
        let items = [Item::fixed(50.0), Item::fixed(30.0)];
        assert_eq!(distribute(&items, 500.0), vec![50.0, 30.0]);
    }

    #[test]
    fn a_single_grower_takes_all_the_leftover() {
        let items = [Item::fixed(50.0), flexible(30.0, 1.0, 1.0)];
        let sizes = distribute(&items, 200.0);
        assert_eq!(sizes[0], 50.0);
        assert!(close(sizes[1], 150.0), "{sizes:?}");
    }

    #[test]
    fn growers_split_the_leftover_in_proportion_to_their_weights() {
        let items = [flexible(0.0, 1.0, 1.0), flexible(0.0, 3.0, 1.0)];
        let sizes = distribute(&items, 400.0);
        assert!(close(sizes[0], 100.0) && close(sizes[1], 300.0), "{sizes:?}");
    }

    #[test]
    fn growth_stops_at_a_declared_maximum_and_the_rest_goes_to_the_others() {
        let items = [
            Item {
                max: 80.0,
                ..flexible(0.0, 1.0, 1.0)
            },
            flexible(0.0, 1.0, 1.0),
        ];
        let sizes = distribute(&items, 400.0);
        assert!(close(sizes[0], 80.0), "{sizes:?}");
        assert!(close(sizes[1], 320.0), "second takes what the first refused: {sizes:?}");
    }

    #[test]
    fn overflow_is_given_back_in_proportion_to_size_not_just_factor() {
        // Equal shrink factors, unequal sizes: the bigger box gives back more.
        let items = [flexible(100.0, 0.0, 1.0), flexible(300.0, 0.0, 1.0)];
        let sizes = distribute(&items, 200.0);
        // 200 of overflow, weighted 100:300 — so 50 and 150 come off.
        assert!(close(sizes[0], 50.0) && close(sizes[1], 150.0), "{sizes:?}");
    }

    #[test]
    fn a_box_refusing_to_shrink_keeps_its_size_and_the_rest_absorb_it() {
        let items = [flexible(100.0, 0.0, 0.0), flexible(300.0, 0.0, 1.0)];
        let sizes = distribute(&items, 300.0);
        assert!(close(sizes[0], 100.0) && close(sizes[1], 200.0), "{sizes:?}");
    }

    #[test]
    fn shrinking_stops_at_the_minimum_and_the_container_overflows() {
        let items = [
            Item {
                min: 90.0,
                ..flexible(100.0, 0.0, 1.0)
            },
            Item {
                min: 0.0,
                ..flexible(100.0, 0.0, 1.0)
            },
        ];
        let sizes = distribute(&items, 100.0);
        assert!(close(sizes[0], 90.0), "{sizes:?}");
        assert!(close(sizes[1], 10.0), "{sizes:?}");
    }

    #[test]
    fn an_empty_container_distributes_nothing() {
        assert_eq!(distribute(&[], 500.0), Vec::<f32>::new());
    }

    #[test]
    fn a_base_outside_its_own_bounds_is_clamped_even_with_nothing_to_distribute() {
        let items = [Item {
            base: 10.0,
            min: 40.0,
            max: 80.0,
            grow: 0.0,
            shrink: 0.0,
        }];
        assert_eq!(distribute(&items, 40.0), vec![40.0]);
    }
}
