use std::time::Duration;
use std::time::Instant;

use parking_lot::{Condvar, Mutex};

use crate::AtomicRingBuffer;

///A constant-size almost lock-free concurrent ring buffer with blocking poll support
///
/// See AtomicRingQueue for implementation details
///
/// # Examples
///
///```
/// // create an AtomicRingQueue with capacity of 1024 elements
/// let ring = ::atomicring::AtomicRingQueue::with_capacity(900);
///
/// // try_pop removes an element of the buffer and returns None if the buffer is empty
/// assert_eq!(None, ring.try_pop());
/// // push_overwrite adds an element to the buffer, overwriting the oldest element if the buffer is full:
/// ring.push_overwrite(10);
/// assert_eq!(10, ring.pop());
/// assert_eq!(None, ring.try_pop());
///```
pub struct AtomicRingQueue<T> {
    mutex: Mutex<()>,
    condvar: Condvar,
    ring: AtomicRingBuffer<T>,
}

/// If T is Send, AtomicRingQueue is Send + Sync
unsafe impl<T: Send> Send for AtomicRingQueue<T> {}

/// Any particular `T` should never accessed concurrently, so T does not need to be Sync.
/// If T is Send, AtomicRingQueue is Send + Sync
unsafe impl<T: Send> Sync for AtomicRingQueue<T> {}


impl<T> AtomicRingQueue<T> {
    /// Constructs a new empty AtomicRingQueue<T> with the specified capacity
    /// the capacity is rounded up to the next power of 2
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingQueue with capacity of 1024 elements
    /// let ring = ::atomicring::AtomicRingQueue::with_capacity(900);
    ///
    /// // try_pop removes an element of the buffer and returns None if the buffer is empty
    /// assert_eq!(None, ring.try_pop());
    /// // push_overwrite adds an element to the buffer, overwriting the oldest element if the buffer is full:
    /// ring.push_overwrite(10);
    /// assert_eq!(10, ring.pop());
    /// assert_eq!(None, ring.try_pop());
    ///```
    pub fn with_capacity(capacity: usize) -> AtomicRingQueue<T> {
        AtomicRingQueue {
            mutex: Mutex::new(()),
            condvar: Condvar::new(),
            ring: AtomicRingBuffer::with_capacity(capacity),
        }
    }

    fn trigger(&self) {
        let _ = self.mutex.lock();
        self.condvar.notify_one();
    }

    /// Try to push an object to the atomic ring buffer.
    /// If the buffer has no capacity remaining, the pushed object will be returned to the caller as error.
    #[inline(always)]
    pub fn try_push(&self, content: T) -> Result<(), T> {
        let result = self.ring.try_push(content);
        if result.is_ok() {
            self.trigger();
        }
        result
    }

    /// Pushes an object to the atomic ring buffer.
    /// If the buffer is full, another object will be popped to make room for the new object.
    #[inline(always)]
    pub fn push_overwrite(&self, content: T) {
        self.ring.push_overwrite(content);
        self.trigger();
    }

    /// Pop an object from the ring buffer, returns None if the buffer is empty
    #[inline]
    pub fn try_pop(&self) -> Option<T> {
        self.ring.try_pop()
    }

    #[inline(always)]
    fn spinning_pop(&self) -> Option<T> {
        for i in 0..10 {
            if let res @ Some(_) = self.ring.try_pop() {
                return res;
            }
            for _ in 0..i << 1 {
                ::std::sync::atomic::spin_loop_hint();
            }
        }
        for _ in 0..10 {
            if let res @ Some(_) = self.ring.try_pop() {
                return res;
            }
            ::std::thread::yield_now();
        }
        None
    }

    /// Pop an object from the ring buffer, waits indefinitely if the buf is empty
    #[inline]
    pub fn pop(&self) -> T {
        loop {
            if let Some(res) = self.spinning_pop() {
                return res;
            }
            {
                let mut lock = self.mutex.lock();
                if let Some(res) = self.try_pop() {
                    return res;
                }
                self.condvar.wait(&mut lock);
            }
        }
    }

    /// Pop an object from the ring buffer, waiting until the given instant if the buffer is empty. Returns None on timeout
    #[inline]
    pub fn pop_until(&self, deadline: Instant) -> Option<T> {
        loop {
            if let res @ Some(_) = self.spinning_pop() {
                return res;
            }
            {
                let mut lock = self.mutex.lock();
                if let res @ Some(_) = self.try_pop() {
                    return res;
                }
                if self.condvar.wait_until(&mut lock, deadline).timed_out() {
                    return None;
                }
            }
        }
    }

    /// Pop an object from the ring buffer, waiting until the given instant if the buffer is empty. Returns None on timeout
    #[inline]
    pub fn pop_for(&self, timeout: Duration) -> Option<T> {
        self.pop_until(Instant::now() + timeout)
    }


    /// Returns the number of objects stored in the ring buffer that are not in process of being removed.
    #[inline]
    pub fn len(&self) -> usize {
        self.ring.len()
    }


    /// Returns the true if ring buffer is empty. Equivalent to `self.len() == 0`
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    /// Returns the maximum capacity of the ring buffer.
    /// Attention: In fact you can store one element less than the cap given here
    #[inline(always)]
    pub fn cap(&self) -> usize {
        self.ring.cap()
    }

    /// Returns the remaining capacity of the ring buffer.
    /// This is equal to `self.cap() - self.len() - pending writes + pending reads`.
    #[inline]
    pub fn remaining_cap(&self) -> usize {
        self.ring.remaining_cap()
    }

    /// Pop everything from ring buffer and discard it.
    #[inline]
    pub fn clear(&self) {
        self.ring.clear()
    }
}


#[cfg(test)]
mod tests {
    #[test]
    pub fn test_pushpop() {
        let ring = super::AtomicRingQueue::with_capacity(900);
        assert_eq!(1024, ring.cap());
        assert_eq!(None, ring.try_pop());
        ring.push_overwrite(1);
        assert_eq!(1, ring.pop());
        assert_eq!(None, ring.try_pop());

        for i in 0..5000 {
            ring.push_overwrite(i);
            assert_eq!(i, ring.pop());
            assert_eq!(None, ring.try_pop());
        }


        for i in 0..199999 {
            ring.push_overwrite(i);
        }
        assert_eq!(ring.cap(), ring.len() + 1);
        assert_eq!(199999 - (ring.cap() - 1), ring.pop());
        assert_eq!(Ok(()), ring.try_push(199999));

        for i in 200000 - (ring.cap() - 1)..200000 {
            assert_eq!(i, ring.pop());
        }
    }

    #[test]
    pub fn test_pushpop_large() {
        let ring = super::AtomicRingQueue::with_capacity(65535);


        assert_eq!(None, ring.try_pop());
        ring.push_overwrite(1);
        assert_eq!(1, ring.pop());

        for i in 0..200000 {
            ring.push_overwrite(i);
            assert_eq!(i, ring.pop());
        }


        for i in 0..200000 {
            ring.push_overwrite(i);
        }
        assert_eq!(ring.cap(), ring.len() + 1);

        for i in 200000 - (ring.cap() - 1)..200000 {
            assert_eq!(i, ring.pop());
        }
    }

    #[test]
    pub fn test_pushpop_large2() {
        let ring = super::AtomicRingQueue::with_capacity(65536);


        assert_eq!(None, ring.try_pop());
        ring.push_overwrite(1);
        assert_eq!(1, ring.pop());

        for i in 0..200000 {
            ring.push_overwrite(i);
            assert_eq!(i, ring.pop());
        }


        for i in 0..200000 {
            ring.push_overwrite(i);
        }
        assert_eq!(ring.cap(), ring.len() + 1);

        for i in 200000 - (ring.cap() - 1)..200000 {
            assert_eq!(i, ring.pop());
        }
    }


    #[test]
    pub fn test_pushpop_large2_zerotype() {
        #[derive(Eq, PartialEq, Debug)]
        struct ZeroType {}

        let ring = super::AtomicRingQueue::with_capacity(65536);


        assert_eq!(None, ring.try_pop());
        ring.push_overwrite(ZeroType {});
        assert_eq!(ZeroType {}, ring.pop());

        for _i in 0..200000 {
            ring.push_overwrite(ZeroType {});
            assert_eq!(ZeroType {}, ring.pop());
        }


        for _i in 0..200000 {
            ring.push_overwrite(ZeroType {});
        }
        assert_eq!(ring.cap(), ring.len() + 1);

        for _i in 200000 - (ring.cap() - 1)..200000 {
            assert_eq!(ZeroType {}, ring.pop());
        }
    }


    #[test]
    pub fn test_threaded() {
        let cap = 65535;

        let buf: super::AtomicRingQueue<usize> = super::AtomicRingQueue::with_capacity(cap);
        for i in 0..cap {
            buf.try_push(i).expect("init");
        }
        let arc = ::std::sync::Arc::new(buf);

        let mut handles = Vec::new();
        let end = ::std::time::Instant::now() + ::std::time::Duration::from_millis(10000);
        for _thread_num in 0..100 {
            let buf = ::std::sync::Arc::clone(&arc);
            handles.push(::std::thread::spawn(move || {
                while ::std::time::Instant::now() < end {
                    let a = buf.pop();
                    let b = buf.pop();
                    while let Err(_) = buf.try_push(a) {};
                    while let Err(_) = buf.try_push(b) {};
                }
            }));
        }
        for (_idx, handle) in handles.into_iter().enumerate() {
            handle.join().expect("join");
        }

        assert_eq!(arc.len(), cap);

        let mut expected: Vec<usize> = Vec::new();
        let mut actual: Vec<usize> = Vec::new();
        for i in 0..cap {
            expected.push(i);
            actual.push(arc.pop());
        }
        actual.sort_by(|&a, b| a.partial_cmp(b).unwrap());
        assert_eq!(actual, expected);
    }

    static DROP_COUNT: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::AtomicUsize::new(0);

    #[allow(dead_code)]
    #[derive(Debug)]
    struct TestType {
        some: usize
    }


    impl Drop for TestType {
        fn drop(&mut self) {
            DROP_COUNT.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[test]
    pub fn test_dropcount() {
        DROP_COUNT.store(0, ::std::sync::atomic::Ordering::Relaxed);
        {
            let buf: super::AtomicRingQueue<TestType> = super::AtomicRingQueue::with_capacity(1024);
            buf.try_push(TestType { some: 0 }).expect("push");
            buf.try_push(TestType { some: 0 }).expect("push");

            assert_eq!(0, DROP_COUNT.load(::std::sync::atomic::Ordering::Relaxed));
            buf.pop();
            assert_eq!(1, DROP_COUNT.load(::std::sync::atomic::Ordering::Relaxed));
        }
        assert_eq!(2, DROP_COUNT.load(::std::sync::atomic::Ordering::Relaxed));
    }
}
