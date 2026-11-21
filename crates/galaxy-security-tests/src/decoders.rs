//! Release property tests for untrusted decoder entry points. See ADR 0120.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

use proptest::{
    collection::vec,
    prelude::*,
    test_runner::{Config, RngAlgorithm, RngSeed, TestCaseResult, TestRunner},
};

mod attestation;
mod evidence;
mod image;
mod lookup;
mod wire;
mod xrpl;

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

fn property<S: Strategy>(cases: u32, strategy: S, test: impl Fn(S::Value) -> TestCaseResult) {
    let config = Config {
        cases,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(0x4758_5345_4300_0001),
        failure_persistence: None,
        max_shrink_iters: 128,
        ..Config::default()
    };
    TestRunner::new(config).run(&strategy, test).unwrap();
}

fn noise(max: usize) -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 0..=max)
}

fn change(bytes: &mut [u8], position: usize, mask: u8) -> usize {
    let position = position % bytes.len();
    bytes[position] ^= mask;
    position
}

fn mutation() -> impl Strategy<Value = (usize, u8)> {
    (any::<usize>(), 1u8..=255)
}

// Measure total requested bytes, including growth via realloc. The generator, assertions,
// fixture loading and unrelated test threads are not charged to the decoder call.
thread_local! {
    static CHARGED: Cell<Option<usize>> = const { Cell::new(None) };
}

struct Meter;

#[global_allocator]
static ALLOCATOR: Meter = Meter;

fn charge(size: usize) {
    let _ = CHARGED.try_with(|slot| {
        if let Some(total) = slot.get() {
            slot.set(Some(total.saturating_add(size)));
        }
    });
}

// SAFETY: exactly forwards each allocation/deallocation to System with the original
// pointer and layout, and tracks sizes without allocating in the allocator callbacks.
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        charge(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        charge(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        charge(size);
        unsafe { System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn bounded<T>(decoder: &str, ceiling: usize, decode: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            CHARGED.set(None);
        }
    }
    assert!(CHARGED.replace(Some(0)).is_none(), "nested allocation measurement");
    let reset = Reset;
    let decoded = decode();
    let used = CHARGED.get().unwrap();
    drop(reset);
    assert!(used <= ceiling, "{decoder} requested {used} heap bytes (budget {ceiling})");
    decoded
}
