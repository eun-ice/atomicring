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
    index: usize
}

unsafe impl Send for SmallType {}

unsafe impl Sync for SmallType {}

impl SmallType {
    fn new(index: usize) -> SmallType {
        SmallType { index }
    }
}

#[allow(dead_code)]
#[derive(Default)]
struct MediumType {
    index: usize,
    some1: [usize; 31],
}

impl MediumType {
    fn new(index: usize) -> MediumType {
        MediumType { index, some1: [0; 31] }
    }
}

unsafe impl Send for MediumType {}

unsafe impl Sync for MediumType {}


#[derive(Default)]
#[allow(dead_code)]
struct LargeType {
    index: usize,
    some1: [usize; 31],
    some2: [usize; 32],
    some3: [usize; 32],
    some4: [usize; 32],
}

unsafe impl Send for LargeType {}

unsafe impl Sync for LargeType {}


impl LargeType {
    fn new(index: usize) -> LargeType {
        LargeType { index, some1: [0; 31], some2: [0; 32], some3: [0; 32], some4: [0; 32] }
    }
}

#[bench]
fn bench_ring_singlethread_small(b: &mut Bencher) {
    let ring: AtomicRingBuffer<SmallType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            let _ = ring.try_push(SmallType::new(i)).ok().expect("!!");
        }
        for i in 0..10000 {
            assert_eq!(ring.try_pop().expect("!!").index, i);
        }
    });
}

#[bench]
fn bench_ring_singlethread_optionsmall(b: &mut Bencher) {
    let ring: AtomicRingBuffer<Option<SmallType>> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            let _ = ring.try_push(Some(SmallType::new(i))).ok().expect("!!");
        }
        for i in 0..10000 {
            assert_eq!(ring.try_pop().expect("!!").unwrap().index, i);
        }
    });
}


#[bench]
fn bench_ring_singlethread_medium(b: &mut Bencher) {
    let ring: AtomicRingBuffer<MediumType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            let _ = ring.try_push(MediumType::new(i)).ok().expect("!!");
        }
        for i in 0..10000 {
            assert_eq!(ring.try_pop().expect("!!").index, i);
        }
    });
}


#[bench]
fn bench_ring_singlethread_large(b: &mut Bencher) {
    let ring: AtomicRingBuffer<LargeType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            let _ = ring.try_push(LargeType::new(i)).ok().expect("!!");
        }
        for i in 0..10000 {
            assert_eq!(ring.try_pop().expect("!!").index, i);
        }
    });
}


#[bench]
fn bench_ring_inline_singlethread_small(b: &mut Bencher) {
    let ring: AtomicRingBuffer<SmallType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            ring.try_write(|w| { w.index = i }).ok().expect("!!");
        }
        for i in 0..10000 {
            ring.try_read(|r| { assert_eq!(r.index, i) }).expect("!!");
        }
    });
}

#[bench]
fn bench_ring_inline_singlethread_optionsmall(b: &mut Bencher) {
    let ring: AtomicRingBuffer<Option<SmallType>> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            ring.try_write(|w| { *w = Some(SmallType::new(i)) }).ok().expect("!!");
        }
        for i in 0..10000 {
            ring.try_read(|r| { assert_eq!(r.as_mut().unwrap().index, i) }).expect("!!");
        }
    });
}


#[bench]
fn bench_ring_inline_singlethread_medium(b: &mut Bencher) {
    let ring: AtomicRingBuffer<MediumType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            ring.try_write(|w| { w.index = i }).ok().expect("!!");
        }
        for i in 0..10000 {
            ring.try_read(|r| { assert_eq!(r.index, i) }).expect("!!");
        }
    });
}


#[bench]
fn bench_ring_inline_singlethread_large(b: &mut Bencher) {
    let ring: AtomicRingBuffer<LargeType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            ring.try_unsafe_write(|w| unsafe { ::std::ptr::write_unaligned(w, LargeType::new(i)) }).ok().expect("!!");
        }
        for i in 0..10000 {
            ring.try_read(|r| { assert_eq!(r.index, i) }).expect("!!");
        }
    });
}


#[bench]
fn bench_ring_unsafe_singlethread_small(b: &mut Bencher) {
    let ring: AtomicRingBuffer<SmallType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            ring.try_unsafe_write(|w| unsafe { ::std::ptr::write_unaligned(w, SmallType::new(i)) }).ok().expect("!!");
        }
        for i in 0..10000 {
            ring.try_read(|r| { assert_eq!(r.index, i) }).expect("!!");
        }
    });
}

#[bench]
fn bench_ring_unsafe_singlethread_optionsmall(b: &mut Bencher) {
    let ring: AtomicRingBuffer<Option<SmallType>> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            ring.try_unsafe_write(|w| unsafe { ::std::ptr::write_unaligned(w, Some(SmallType::new(i))) }).ok().expect("!!");
        }
        for i in 0..10000 {
            ring.try_read(|r| { assert_eq!(r.as_mut().unwrap().index, i) }).expect("!!");
        }
    });
}


#[bench]
fn bench_ring_unsafe_singlethread_medium(b: &mut Bencher) {
    let ring: AtomicRingBuffer<MediumType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            ring.try_unsafe_write(|w| unsafe { ::std::ptr::write_unaligned(w, MediumType::new(i)) }).ok().expect("!!");
        }
        for i in 0..10000 {
            ring.try_read(|r| { assert_eq!(r.index, i) }).expect("!!");
        }
    });
}


#[bench]
fn bench_ring_unsafe_singlethread_large(b: &mut Bencher) {
    let ring: AtomicRingBuffer<LargeType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            ring.try_unsafe_write(|w| unsafe { ::std::ptr::write_unaligned(w, LargeType::new(i)) }).ok().expect("!!");
        }
        for i in 0..10000 {
            ring.try_read(|r| { assert_eq!(r.index, i) }).expect("!!");
        }
    });
}


#[bench]
fn bench_mpmc_singlethread_small(b: &mut Bencher) {
    let ring: Queue<SmallType> = Queue::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            let _ = ring.push(SmallType::new(i));
        }
        for i in 0..10000 {
            assert_eq!(ring.pop().unwrap().index, i);
        }
    });
}

#[bench]
fn bench_mpmc_singlethread_medium(b: &mut Bencher) {
    let ring: Queue<MediumType> = Queue::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            let _ = ring.push(MediumType::new(i));
        }
        for i in 0..10000 {
            assert_eq!(ring.pop().unwrap().index, i);
        }
    });
}


#[bench]
fn bench_mpmc_singlethread_large(b: &mut Bencher) {
    let ring: Queue<LargeType> = Queue::with_capacity(10000);
    b.iter(|| {
        for i in 0..10000 {
            let _ = ring.push(LargeType::new(i));
        }
        for i in 0..10000 {
            assert_eq!(ring.pop().unwrap().index, i);
        }
    });
}
