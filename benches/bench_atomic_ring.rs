#![feature(test)]
extern crate atomicring;
extern crate mpmc;
extern crate test;

use atomicring::AtomicRingBuffer;
use mpmc::Queue;
use test::Bencher;


#[allow(dead_code)]
#[derive(Default)]
struct ZeroType {}

unsafe impl Send for ZeroType {}

unsafe impl Sync for ZeroType {}


#[allow(dead_code)]
#[derive(Default)]
struct SmallType {
    some: usize
}

unsafe impl Send for SmallType {}

unsafe impl Sync for SmallType {}

#[allow(dead_code)]
#[derive(Default)]
struct MediumType {
    some1: [usize; 32],
}

impl MediumType {
    fn new() -> MediumType {
        MediumType { some1: [0; 32] }
    }
}

unsafe impl Send for MediumType {}

unsafe impl Sync for MediumType {}


#[derive(Default)]
#[allow(dead_code)]
struct LargeType {
    some1: [usize; 32],
    some2: [usize; 32],
    some3: [usize; 32],
    some4: [usize; 32],
}

unsafe impl Send for LargeType {}

unsafe impl Sync for LargeType {}


impl LargeType {
    fn new() -> LargeType {
        LargeType { some1: [0; 32], some2: [0; 32], some3: [0; 32], some4: [0; 32] }
    }
}

#[bench]
fn bench_ring_singlethread_small(b: &mut Bencher) {
    let ring: AtomicRingBuffer<SmallType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for _ in 0..10000 {
            ring.try_push(Default::default()).ok().expect("!!");
        }
        for _ in 0..10000 {
            ring.try_pop().expect("!!");;
        }
    });
}

#[bench]
fn bench_ring_singlethread_optionsmall(b: &mut Bencher) {
    let ring: AtomicRingBuffer<Option<SmallType>> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for _ in 0..10000 {
            let _ = ring.try_push(Default::default()).ok().expect("!!");;
        }
        for _ in 0..10000 {
            ring.try_pop().expect("!!");;
        }
    });
}


#[bench]
fn bench_ring_singlethread_medium(b: &mut Bencher) {
    let ring: AtomicRingBuffer<MediumType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for _ in 0..10000 {
            let _ = ring.try_push(MediumType::new()).ok().expect("!!");;
        }
        for _ in 0..10000 {
            ring.try_pop().expect("!!");;
        }
    });
}


#[bench]
fn bench_ring_singlethread_large(b: &mut Bencher) {
    let ring: AtomicRingBuffer<LargeType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for _ in 0..10000 {
            let _ = ring.try_push(LargeType::new());
        }
        for _ in 0..10000 {
            ring.try_pop();
        }
    });
}

#[bench]
fn bench_mpmc_singlethread_small(b: &mut Bencher) {
    let ring: Queue<SmallType> = Queue::with_capacity(10000);
    b.iter(|| {
        for _ in 0..10000 {
            let _ = ring.push(Default::default());
        }
        for _ in 0..10000 {
            ring.pop();
        }
    });
}

#[bench]
fn bench_mpmc_singlethread_medium(b: &mut Bencher) {
    let ring: Queue<MediumType> = Queue::with_capacity(10000);
    b.iter(|| {
        for _ in 0..10000 {
            let _ = ring.push(MediumType::new());
        }
        for _ in 0..10000 {
            ring.pop();
        }
    });
}


#[bench]
fn bench_mpmc_singlethread_large(b: &mut Bencher) {
    let ring: Queue<LargeType> = Queue::with_capacity(10000);
    b.iter(|| {
        for _ in 0..10000 {
            let _ = ring.push(LargeType::new());
        }
        for _ in 0..10000 {
            ring.pop();
        }
    });
}
