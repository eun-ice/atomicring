#![feature(test)]
extern crate atomicring;
extern crate mpmc;
extern crate test;

use atomicring::AtomicRingBuffer;
use mpmc::Queue;
use test::Bencher;


#[bench]
fn bench_ring_singlethread_small(b: &mut Bencher) {
    let ring: AtomicRingBuffer<SmallType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for _ in 0..10000 {
            let _ = ring.try_push(Default::default());
        }
        for _ in 0..10000 {
            ring.try_pop();
        }
    });
}


#[bench]
fn bench_ring_singlethread_large(b: &mut Bencher) {
    let ring: AtomicRingBuffer<LargeType> = AtomicRingBuffer::with_capacity(10000);
    b.iter(|| {
        for _ in 0..10000 {
            let _ = ring.try_push(Default::default());
        }
        for _ in 0..10000 {
            ring.try_pop();
        }
    });
}


#[bench]
fn bench_mpmc_singlethread_large(b: &mut Bencher) {
    let ring: Queue<LargeType> = Queue::with_capacity(10000);
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

#[allow(dead_code)]
#[derive(Default)]
struct ZeroType {
    some: usize
}

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
struct LargeType {
    some1: [usize; 32],
    some2: [usize; 32],
    some3: [usize; 32],
    some4: [usize; 32],
}

unsafe impl Send for LargeType {}

unsafe impl Sync for LargeType {}
/*
TODO: This attempt at benching multiple threads is flawed. Inacurracy is larger than the resulting values

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Condvar;
use std::sync::Mutex;

#[bench]
fn bench_ring_multithread_small_5r_1w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 5, 1, 10000);
}

#[bench]
fn bench_ring_multithread_small_5r_2w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 5, 2, 10000);
}

#[bench]
fn bench_ring_multithread_small_5r_4w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 5, 4, 10000);
}

#[bench]
fn bench_ring_multithread_small_5r_5w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 5, 5, 10000);
}

#[bench]
fn bench_ring_multithread_small_5r_10w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 5, 10, 10000);
}

#[bench]
fn bench_ring_multithread_small_10r_1w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 10, 1, 10000);
}

#[bench]
fn bench_ring_multithread_small_10r_2w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 10, 2, 10000);
}

#[bench]
fn bench_ring_multithread_small_10r_4w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 10, 4, 10000);
}

#[bench]
fn bench_ring_multithread_small_10r_5w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 10, 5, 10000);
}

#[bench]
fn bench_ring_multithread_small_10r_10w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 10, 10, 10000);
}


#[bench]
fn bench_ring_multithread_small_1r_1w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 1, 1, 10000);
}

#[bench]
fn bench_ring_multithread_small_1r_2w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 1, 2, 10000);
}

#[bench]
fn bench_ring_multithread_small_1r_4w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 1, 4, 10000);
}

#[bench]
fn bench_ring_multithread_small_1r_5w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 1, 5, 10000);
}

#[bench]
fn bench_ring_multithread_small_1r_10w(b: &mut Bencher) {
    bench_ring::<SmallType>(b, 1, 10, 10000);
}


#[bench]
fn bench_mpmc_multithread_small_5r_1w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 5, 1, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_5r_2w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 5, 2, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_5r_4w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 5, 4, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_5r_5w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 5, 5, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_5r_10w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 5, 10, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_10r_1w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 10, 1, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_10r_2w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 10, 2, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_10r_4w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 10, 4, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_10r_5w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 10, 5, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_10r_10w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 10, 10, 10000);
}


#[bench]
fn bench_mpmc_multithread_small_1r_1w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 1, 1, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_1r_2w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 1, 2, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_1r_4w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 1, 4, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_1r_5w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 1, 5, 10000);
}

#[bench]
fn bench_mpmc_multithread_small_1r_10w(b: &mut Bencher) {
    bench_mpmc::<SmallType>(b, 1, 10, 10000);
}

enum Status {
    Wait,
    Start,
    Stop,
}

struct ThreadTrigger {
    mutex: Mutex<Status>,
    condvar: Condvar,
}

impl ThreadTrigger {
    fn new() -> ThreadTrigger {
        ThreadTrigger { mutex: Mutex::new(Status::Wait), condvar: Condvar::new() }
    }
    fn await(&self) -> Status {

        // Wait for the thread to start up.

        let mut status = self.mutex.lock().unwrap();
        // As long as the value inside the `Mutex` is false, we wait.
        loop {
            match *status {
                Status::Wait => status = self.condvar.wait(status).unwrap(),
                Status::Start => return Status::Start,
                Status::Stop => return Status::Stop,
            }
        }
    }
    fn set(&self, status: Status) {
        let mut started = self.mutex.lock().unwrap();
        *started = status;
        // We notify the condvar that the value has changed.
        self.condvar.notify_all();
    }
}

fn bench_ring<T: Default + Send + 'static>(b: &mut Bencher, readers: usize, writers: usize, write_count: usize) {
    let ring: Arc<AtomicRingBuffer<T>> = Arc::new(AtomicRingBuffer::with_capacity(write_count));
    if write_count % writers != 0 {
        panic!("write_count must be divisible by writers");
    }
    if write_count % readers != 0 {
        panic!("write_count must be divisible by writers");
    }
    let read_per_thread_count = write_count / readers;
    let write_per_thread_count = write_count / writers;
    let read_trigger = Arc::new(ThreadTrigger::new());
    let write_trigger = Arc::new(ThreadTrigger::new());
    let done_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(readers + writers);

    for _ in 0..writers {
        let ring = Arc::clone(&ring);
        let done_count = Arc::clone(&done_count);
        let write_trigger = Arc::clone(&write_trigger);

        ::std::thread::spawn(move || {
            loop {
                if let Status::Stop = write_trigger.await() {
                    done_count.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                for _ in 0..write_per_thread_count {
                    let _ = ring.try_push(Default::default());
                }
                done_count.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
    for _ in 0..readers {
        let ring = Arc::clone(&ring);
        let done_count = Arc::clone(&done_count);
        let read_trigger = Arc::clone(&read_trigger);
        handles.push(::std::thread::spawn(move || {
            loop {
                if let Status::Stop = read_trigger.await() {
                    done_count.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let mut read = 0;
                while read < read_per_thread_count {
                    if let Some(_) = ring.try_pop() {
                        read += 1;
                    }
                }
                done_count.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    b.iter(|| {
        done_count.store(0, Ordering::Relaxed);


        while done_count.load(Ordering::Relaxed) < writers {
            write_trigger.set(Status::Start);
        }
        done_count.store(0, Ordering::Relaxed);


        while done_count.load(Ordering::Relaxed) < readers {
            read_trigger.set(Status::Start);
        }
        read_trigger.set(Status::Wait);
    });

    done_count.store(0, Ordering::Relaxed);

    while done_count.load(Ordering::Relaxed) < readers {
        read_trigger.set(Status::Stop);
    }
    done_count.store(0, Ordering::Relaxed);

    while done_count.load(Ordering::Relaxed) < writers {
        write_trigger.set(Status::Stop);
    }


    for handle in handles {
        let _ = handle.join();
    }
}


fn bench_mpmc<T: Default + Send + Sync + 'static>(b: &mut Bencher, readers: usize, writers: usize, write_count: usize) {
    let ring: Arc<Queue<T>> = Arc::new(Queue::with_capacity(write_count));
    if write_count % writers != 0 {
        panic!("write_count must be divisible by writers");
    }
    if write_count % readers != 0 {
        panic!("write_count must be divisible by writers");
    }
    let read_per_thread_count = write_count / readers;
    let write_per_thread_count = write_count / writers;
    let read_trigger = Arc::new(ThreadTrigger::new());
    let write_trigger = Arc::new(ThreadTrigger::new());
    let done_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(readers + writers);

    for _ in 0..writers {
        let ring = Arc::clone(&ring);
        let done_count = Arc::clone(&done_count);
        let write_trigger = Arc::clone(&write_trigger);

        ::std::thread::spawn(move || {
            loop {
                if let Status::Stop = write_trigger.await() {
                    done_count.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                for _ in 0..write_per_thread_count {
                    let _ = ring.push(Default::default());
                }
                done_count.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
    for _ in 0..readers {
        let ring = Arc::clone(&ring);
        let done_count = Arc::clone(&done_count);
        let read_trigger = Arc::clone(&read_trigger);
        handles.push(::std::thread::spawn(move || {
            loop {
                if let Status::Stop = read_trigger.await() {
                    done_count.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let mut read = 0;
                while read < read_per_thread_count {
                    if let Some(_) = ring.pop() {
                        read += 1;
                    }
                }
                done_count.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    b.iter(|| {
        done_count.store(0, Ordering::Relaxed);


        while done_count.load(Ordering::Relaxed) < writers {
            write_trigger.set(Status::Start);
        }
        done_count.store(0, Ordering::Relaxed);


        while done_count.load(Ordering::Relaxed) < readers {
            read_trigger.set(Status::Start);
        }
        read_trigger.set(Status::Wait);
    });

    done_count.store(0, Ordering::Relaxed);

    while done_count.load(Ordering::Relaxed) < readers {
        read_trigger.set(Status::Stop);
    }
    done_count.store(0, Ordering::Relaxed);

    while done_count.load(Ordering::Relaxed) < writers {
        write_trigger.set(Status::Stop);
    }


    for handle in handles {
        let _ = handle.join();
    }
}

*/