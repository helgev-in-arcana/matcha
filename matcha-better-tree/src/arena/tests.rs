use super::*;

#[test]
fn reservations_are_invisible_and_distinct_until_filled() {
    let mut arena = Arena::new();
    let parent = arena.reserve();
    let child = arena.reserve();
    assert_ne!(parent, child);
    assert!(arena.get(parent).is_none());
    assert!(arena.get_mut(parent).is_none());
    assert_eq!(arena.iter().count(), 0);
    assert_eq!(arena.insert_reserved(child, String::from("child")), Ok(()));
    assert_eq!(arena.get(child).map(String::as_str), Some("child"));
    assert!(arena.get(parent).is_none());
    assert_eq!(
        arena.insert_reserved(parent, String::from("parent")),
        Ok(())
    );
    arena.get_mut(parent).unwrap().push('!');
    assert_eq!(arena.get(parent).map(String::as_str), Some("parent!"));
    assert_eq!(arena.iter().count(), 2);
}

#[test]
fn cancelling_and_reusing_a_slot_rejects_the_old_reservation() {
    let mut arena = Arena::new();
    let old = arena.reserve();
    assert!(arena.cancel_reserved(old));
    assert!(!arena.cancel_reserved(old));
    let current = arena.reserve();
    assert_eq!(current.index, old.index);
    assert_ne!(current.generation, old.generation);
    assert!(!arena.cancel_reserved(old));
    assert_eq!(arena.insert_reserved(old, 11), Err(11));
    assert_eq!(arena.insert_reserved(current, 22), Ok(()));
    assert_eq!(arena.get(current), Some(&22));
    assert!(arena.get(old).is_none());
    assert!(arena.get_mut(old).is_none());
    assert_eq!(arena.remove(old), None);
}

#[test]
fn operations_reject_incorrect_states_without_mutating_live_values() {
    let mut arena = Arena::new();
    let id = arena.reserve();
    assert_eq!(arena.remove(id), None);
    assert_eq!(arena.insert_reserved(id, 7), Ok(()));
    assert_eq!(arena.insert_reserved(id, 8), Err(8));
    assert!(!arena.cancel_reserved(id));
    assert_eq!(arena.get(id), Some(&7));
    assert_eq!(arena.remove(id), Some(7));
    assert_eq!(arena.remove(id), None);
    assert_eq!(arena.insert_reserved(id, 9), Err(9));
    assert!(!arena.cancel_reserved(id));
    let reused = arena.reserve();
    assert_eq!(id.index, reused.index);
    assert_ne!(id, reused);
    assert_eq!(arena.insert_reserved(reused, 10), Ok(()));
    assert_eq!(arena.remove(id), None);
    assert_eq!(arena.get(reused), Some(&10));
}

#[test]
fn invalid_index_and_generation_are_rejected() {
    let mut arena = Arena::new();
    let id = arena.reserve();
    for invalid in [
        Id {
            index: 999,
            generation: 0,
        },
        Id {
            generation: id.generation + 1,
            ..id
        },
    ] {
        assert_eq!(arena.insert_reserved(invalid, 1), Err(1));
        assert!(!arena.cancel_reserved(invalid));
        assert!(arena.get(invalid).is_none());
        assert!(arena.get_mut(invalid).is_none());
        assert_eq!(arena.remove(invalid), None);
    }
    assert_eq!(arena.insert_reserved(id, 2), Ok(()));
}

#[test]
fn clear_reclaims_all_slot_states_without_resurrecting_old_ids() {
    let mut arena = Arena::new();
    let occupied = arena.reserve();
    let reserved = arena.reserve();
    let free = arena.reserve();
    assert_eq!(arena.insert_reserved(occupied, 1), Ok(()));
    assert!(arena.cancel_reserved(free));
    arena.clear();
    assert_eq!(arena.iter().count(), 0);
    let fresh: Vec<_> = (0..3).map(|_| arena.reserve()).collect();
    for old in [occupied, reserved, free] {
        assert_eq!(arena.insert_reserved(old, 2), Err(2));
        assert!(!arena.cancel_reserved(old));
        assert!(arena.get(old).is_none());
    }
    for (value, id) in fresh.into_iter().enumerate() {
        assert_eq!(arena.insert_reserved(id, value), Ok(()));
    }
    assert_eq!(arena.iter().count(), 3);
    let appended = arena.reserve();
    assert_eq!(appended.index, 3);
    assert_eq!(arena.iter().count(), 3);
}

#[test]
fn mixed_removals_and_cancellations_preserve_the_free_list() {
    let mut arena = Arena::new();
    let a = arena.reserve();
    let b = arena.reserve();
    let c = arena.reserve();
    assert_eq!(arena.insert_reserved(a, 1), Ok(()));
    assert_eq!(arena.insert_reserved(c, 3), Ok(()));
    assert_eq!(arena.remove(a), Some(1));
    assert!(arena.cancel_reserved(b));
    let b2 = arena.reserve();
    let a2 = arena.reserve();
    assert_eq!(b2.index, b.index);
    assert_eq!(a2.index, a.index);
    assert_eq!(arena.insert_reserved(a2, 4), Ok(()));
    assert_eq!(arena.insert_reserved(b2, 5), Ok(()));
    assert_eq!(arena.get(c), Some(&3));
    assert_eq!(arena.iter().count(), 3);
}

#[test]
fn generic_values_are_dropped_exactly_once() {
    use std::cell::Cell;
    use std::rc::Rc;
    struct Value(Rc<Cell<usize>>);
    impl Drop for Value {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = Rc::new(Cell::new(0));
    let mut arena = Arena::new();
    let removed = arena.reserve();
    let cleared = arena.reserve();
    let cancelled = arena.reserve();
    assert!(arena.insert_reserved(removed, Value(drops.clone())).is_ok());
    assert!(arena.insert_reserved(cleared, Value(drops.clone())).is_ok());
    assert!(arena.cancel_reserved(cancelled));
    let value = arena.remove(removed).unwrap();
    assert_eq!(drops.get(), 0);
    drop(value);
    assert_eq!(drops.get(), 1);
    arena.clear();
    assert_eq!(drops.get(), 2);
    drop(arena);
    assert_eq!(drops.get(), 2);
}
