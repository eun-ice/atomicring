#![feature(const_fn)]
extern crate atomic64;

mod atomic_ring;

pub use atomic_ring::AtomicRingBuffer;
