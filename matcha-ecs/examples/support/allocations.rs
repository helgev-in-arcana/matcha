//! Example-only CPU assembly allocation meter. Counts this thread while enabled,
//! excluding GPU recording and other threads. Bytes are allocation traffic, not
//! live memory or VRAM. The production framework does not install an allocator.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub allocations: usize,
    pub reallocations: usize,
    pub bytes: usize,
}
thread_local! {
    static SAMPLE: Cell<Option<Sample>> = const { Cell::new(None) };
}
struct Meter;
#[global_allocator]
static ALLOCATOR: Meter = Meter;

fn record(bytes: usize, reallocate: bool) {
    let _ = SAMPLE.try_with(|cell| {
        if let Some(mut sample) = cell.get() {
            if reallocate {
                sample.reallocations += 1;
            } else {
                sample.allocations += 1;
            }
            sample.bytes += bytes;
            cell.set(Some(sample));
        }
    });
}
// Every allocation operation delegates to System with exactly the same layout
// and pointer. The const TLS meter does not allocate or retain allocated memory.
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size(), false);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size(), false);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size, true);
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Sample) {
    SAMPLE.with(|cell| {
        assert!(cell.get().is_none(), "non-nested measurement");
        cell.set(Some(Sample::default()));
    });
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            SAMPLE.with(|cell| cell.set(None));
        }
    }
    let reset = Reset;
    let value = f();
    let sample = SAMPLE.with(|cell| cell.get().expect("measurement enabled"));
    drop(reset);
    (value, sample)
}
