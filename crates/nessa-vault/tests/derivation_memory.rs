//! This test binary has one test and one fixed-size Argon2 scratch allocation.
//! Observe that initialized allocation before freeing it; never inspect freed memory.
use nessa_vault::{
    application::Repository,
    contract,
    domain::Passphrase,
    infrastructure::{EncryptedFileVault, OsEntropy},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

const SCRATCH_BYTES: usize = contract::KDF_MEMORY_KI_B * 1024;
static OBSERVE: AtomicBool = AtomicBool::new(false);
static SCRATCH: AtomicUsize = AtomicUsize::new(0);
static OBSERVED: AtomicBool = AtomicBool::new(false);
static ERASED: AtomicBool = AtomicBool::new(false);
struct ObserveScratch;

// SAFETY: all requests retain System's layout and allocation contracts. Observation
// is read-only and occurs before System deallocates the live allocation.
unsafe impl GlobalAlloc for ObserveScratch {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forward the caller's allocation contract unchanged.
        let pointer = unsafe { System.alloc(layout) };
        if OBSERVE.load(Ordering::SeqCst) && layout.size() == SCRATCH_BYTES && !pointer.is_null() {
            SCRATCH.store(pointer as usize, Ordering::SeqCst);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if layout.size() == SCRATCH_BYTES && SCRATCH.load(Ordering::SeqCst) == pointer as usize {
            // SAFETY: in this single-test binary the observed allocation belongs
            // to Argon2's Vec<Block>. Its Copy block initialization cannot panic;
            // all SCRATCH_BYTES are initialized before its first possible drop.
            // We inspect bytes while they are still allocated, before forwarding
            // deallocation. Neither the plaintext nor unrelated memory is read.
            let bytes = unsafe { std::slice::from_raw_parts(pointer, layout.size()) };
            ERASED.store(bytes.iter().all(|byte| *byte == 0), Ordering::SeqCst);
            OBSERVED.store(true, Ordering::SeqCst);
        }
        // SAFETY: forward the original pointer and layout exactly once.
        unsafe { System.dealloc(pointer, layout) };
    }
}
#[global_allocator]
static ALLOCATOR: ObserveScratch = ObserveScratch;

#[test]
fn derivation_erases_its_initialized_scratch_before_deallocation() {
    let root = tempfile::tempdir().unwrap();
    let repository = EncryptedFileVault::new(root.path().join("vault"), Arc::new(OsEntropy));
    let passphrase = Passphrase::new("disposable scratch verification words".into()).unwrap();
    OBSERVE.store(true, Ordering::SeqCst);
    let created = repository.create(&passphrase, &[]);
    OBSERVE.store(false, Ordering::SeqCst);
    assert!(created.is_ok());
    assert!(
        OBSERVED.load(Ordering::SeqCst),
        "scratch allocation was not observed"
    );
    assert!(
        ERASED.load(Ordering::SeqCst),
        "Argon2 scratch was freed without erasure"
    );
}
