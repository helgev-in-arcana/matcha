//! Example-only measurement of allocation traffic on the calling thread.
//! Counts requested bytes, not live memory, VRAM, driver or other-thread activity.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

#[derive(Clone, Copy, Default)]
pub struct Counts {
    pub allocations: u64,
    pub reallocations: u64,
    pub requested_bytes: u64,
}
thread_local! {
    static ACTIVE: Cell<Option<Counts>> = const { Cell::new(None) };
}
struct Meter;
#[global_allocator]
static ALLOCATOR: Meter = Meter;

fn record(bytes: usize, reallocation: bool) {
    let _ = ACTIVE.try_with(|cell| {
        if let Some(mut counts) = cell.get() {
            counts.allocations = counts.allocations.saturating_add(u64::from(!reallocation));
            counts.reallocations = counts.reallocations.saturating_add(u64::from(reallocation));
            counts.requested_bytes = counts.requested_bytes.saturating_add(bytes as u64);
            cell.set(Some(counts));
        }
    });
}
// SAFETY: Every allocation operation delegates unchanged to System. The meter
// uses only a thread-local Cell, performs no allocations, and never unwinds.
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size(), false);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record(layout.size(), false);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !pointer.is_null() {
            record(new_size, true);
        }
        pointer
    }
}
pub fn measure<T>(operation: impl FnOnce() -> T) -> (T, Counts) {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            ACTIVE.with(|cell| cell.set(None));
        }
    }
    ACTIVE.with(|cell| {
        assert!(cell.get().is_none(), "allocation measurements cannot nest");
        cell.set(Some(Counts::default()));
    });
    let guard = Guard;
    let value = operation();
    let counts = ACTIVE.with(|cell| cell.get().expect("measurement is active"));
    drop(guard);
    (value, counts)
}
