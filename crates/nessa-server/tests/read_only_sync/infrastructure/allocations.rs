//! Thread-local Rust allocation measurement around synchronous cache calls.
//! SQLite's C allocations are outside this measurement; SDK payload copies use Rust.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct MeasuredAllocator;
thread_local! {
    static BYTES: Cell<Option<usize>> = const { Cell::new(None) };
}
fn count(bytes: usize) {
    let _ = BYTES.try_with(|total| {
        if let Some(value) = total.get() {
            total.set(Some(value.saturating_add(bytes)));
        }
    });
}
// SAFETY: each operation forwards its unchanged pointer/layout to System.
unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(size);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;

pub(super) fn measure<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            BYTES.with(|bytes| bytes.set(None));
        }
    }
    BYTES.with(|bytes| {
        assert!(bytes.get().is_none());
        bytes.set(Some(0));
    });
    let reset = Reset;
    let result = operation();
    let bytes = BYTES.with(|bytes| bytes.get().unwrap());
    drop(reset);
    (result, bytes)
}
