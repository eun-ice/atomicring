use std::fmt;
use std::mem;
use std::ptr;
use std::sync::atomic::{Ordering, spin_loop_hint};


///A constant-size almost lock-free concurrent ring buffer
///
///Upsides
///
///- fast, try_push and pop are O(1)
///- scales well even during heavy concurrency
///- only 5 words of memory overhead
///- no memory allocations after initial creation
///
///
///Downsides
///
///- growing/shrinking is not supported
///- no blocking poll support
///- maximum capacity of (usize >> 16) entries
///- capacity is rounded up to the next power of 2
///
///This queue should perform similar to [mpmc](https://github.com/brayniac/mpmc) but with a lower memory overhead. 
///If memory overhead is not your main concern you should run benchmarks to decide which one to use.
///
///## Implementation details
///
///This implementation uses two atomics to store the read_index/write_index
///
///```Text
/// Read index atomic
///+63------------------------------------------------16+15-----8+7------0+
///|                     read_index                     | r_done | r_pend |
///+----------------------------------------------------+--------+--------+
/// Write index atomic
///+63------------------------------------------------16+15-----8+7------0+
///|                     write_index                    | w_done | w_pend |
///+----------------------------------------------------+--------+--------+
///```
///
///- write_index/read_index (16bit on 32bit arch, 48bits on 64bit arch): current read/write position in the ring buffer (head and tail).
///- r_pend/w_pend (8bit): number of pending concurrent read/writes
///- r_done/w_done (8bit): number of completed read/writes.
///
///For reading r_pend is incremented first, then the content of the ring buffer is read from memory.
///After reading is done r_done is incremented. read_index is only incremented if r_done is equal to r_pend.
///
///For writing first w_pend is incremented, then the content of the ring buffer is updated.
///After writing w_done is incremented. If w_done is equal to w_pend then both are set to 0 and write_index is incremented.
///
///In rare cases this can result in a race where multiple threads increment r_pend in turn and r_done never quite reaches r_pend.
///If r_pend == 255 or w_pend == 255 a spinloop waits it to be <255 to continue. This rarely happens in practice, that's why this is called almost lock-free.
///
///
///
///## Dependencies
///
///This package has no dependencies
///
///## Usage
///
///To use AtomicRingBuffer, add this to your `Cargo.toml`:
///
///```toml
///[dependencies]
///atomicring = "0.4.0"
///```
///
///
///And something like this to your code
///
///```rust
///
///// create an AtomicRingBuffer with capacity of 1024 elements 
///let ring = ::atomicring::AtomicRingBuffer::with_capacity(900);
///
///// try_pop removes an element of the buffer and returns None if the buffer is empty
///assert_eq!(None, ring.try_pop());
///// push_overwrite adds an element to the buffer, overwriting the oldest element if the buffer is full: 
///ring.push_overwrite(1);
///assert_eq!(Some(1), ring.try_pop());
///assert_eq!(None, ring.try_pop());
///```
///
///
///## License
///
///Licensed under the terms of MIT license and the Apache License (Version 2.0).
///
///See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE) for details.
///

pub struct AtomicRingBuffer<T: Sized> {
    read_counters: CounterStore,
    write_counters: CounterStore,
    cap_mask: usize,
    ptr: *mut [T],
}

/// Any particular `T` should never accessed concurrently, so T does not need to be Sync
/// If T is Send, AtomicRingBuffer is Send + Sync
unsafe impl<T: Send> Send for AtomicRingBuffer<T> {}

unsafe impl<T: Send> Sync for AtomicRingBuffer<T> {}


impl<T: Sized> AtomicRingBuffer<T> {
    /// Constructs a new empty AtomicRingBuffer<T> with the specified capacity
    /// the capacity is rounded up to the next power of 2
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 1024 elements
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(900);
    ///
    /// // try_pop removes an element of the buffer and returns None if the buffer is empty
    /// assert_eq!(None, ring.try_pop());
    /// // push_overwrite adds an element to the buffer, overwriting the oldest element if the buffer is full:
    /// ring.push_overwrite(10);
    /// assert_eq!(Some(10), ring.try_pop());
    /// assert_eq!(None, ring.try_pop());
    ///```
    ///
    pub fn with_capacity(capacity: usize) -> AtomicRingBuffer<T> {
        if capacity > (::std::usize::MAX >> 16) + 1 {
            panic!("too large!");
        }

        let cap = capacity.next_power_of_two();
        let cap_mask = cap - 1;

        /* allocate using a Vec */
        let mut content: Vec<T> = Vec::with_capacity(cap);
        unsafe { content.set_len(cap); }

        /* Zero memory content
        for i in content.iter_mut() {
            unsafe { ptr::write(i, mem::zeroed()); }
        }
        */

        let ptr = Box::into_raw(content.into_boxed_slice());

        AtomicRingBuffer {
            cap_mask,
            ptr,
            read_counters: CounterStore::new(),
            write_counters: CounterStore::new(),
        }
    }

    /// Try to push an object to the atomic ring buffer.
    /// If the buffer has no capacity remaining, the pushed object will be returned to the caller as error.
    #[inline(always)]
    pub fn try_push(&self, content: T) -> Result<(), T> {
        let mut to_write_index;


        // Mark write as in progress
        let mut write_counters = self.write_counters.load(Ordering::Acquire);
        loop {
            let write_in_process_count = write_counters.in_process_count();

            // spin wait on 255 simultanous in progress writes
            if write_in_process_count == 255 {
                spin_loop_hint();
                write_counters = self.write_counters.load(Ordering::Acquire);
                continue;
            }

            let write_idx = write_counters.index();

            to_write_index = write_idx.wrapping_add(write_in_process_count as usize) & self.cap_mask;

            if to_write_index.wrapping_add(1) & self.cap_mask == self.read_counters.load(Ordering::SeqCst).index() {
                return Err(content);
            }

            let new_counters = write_counters.increment_in_process();
            match self.write_counters.compare_and_exchange_weak(write_counters, new_counters, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => {
                    write_counters = new_counters;
                    break;
                }
                Err(n) => write_counters = n
            };
        }

        // write mem
        unsafe {
            ptr::write(&mut (*self.ptr)[to_write_index], content);
        }

        // Mark write as done
        loop {
            let new_counters = write_counters.increment_done(self.cap_mask);

            match self.write_counters.compare_and_exchange_weak(write_counters, new_counters, Ordering::Release, Ordering::Relaxed) {
                Ok(_) => return Ok(()),
                Err(previous) => write_counters = previous
            };
        }
    }

    /// Pushes an object to the atomic ring buffer.
    /// If the buffer is full, another object will be popped to make room for the new object.
    #[inline]
    pub fn push_overwrite(&self, content: T) {
        let mut cont = content;
        loop {
            let option = self.try_push(cont);
            if option.is_ok() {
                return;
            }
            self.remove_if_full();
            cont = option.err().unwrap();
        }
    }

    /// Pop an object from the ring buffer, returns None if the buffer is empty
    #[inline(always)]
    pub fn try_pop(&self) -> Option<T> {
        let mut read_counters = self.read_counters.load(Ordering::Acquire);

        // Mark read as in progress
        let mut to_read_index;
        loop {
            let read_in_process_count = read_counters.in_process_count();
            // spin wait on 255 simultanous in progress reads
            if read_in_process_count == 255 {
                spin_loop_hint();
                read_counters = self.read_counters.load(Ordering::Acquire);
                continue;
            }

            to_read_index = read_counters.index().wrapping_add(read_in_process_count as usize) & self.cap_mask;
            if to_read_index == self.write_counters.load(Ordering::SeqCst).index() {
                return None;
            }

            let new_counters = read_counters.increment_in_process();

            match self.read_counters.compare_and_exchange_weak(read_counters, new_counters, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => {
                    read_counters = new_counters;
                    break;
                }
                Err(n) => read_counters = n
            };
        }

        let popped = unsafe {
            // Read Memory
            ptr::read(&mut (*self.ptr)[to_read_index])
        };

        // Mark read as done
        loop {
            let new_counters = read_counters.increment_done(self.cap_mask);
            match self.read_counters.compare_and_exchange_weak(read_counters, new_counters, Ordering::Release, Ordering::Relaxed) {
                Ok(_) => {
                    break;
                }
                Err(n) => read_counters = n
            };
        }

        Some(popped)
    }

    /// Returns the number of objects stored in the ring buffer that are not in process of being removed.
    #[inline]
    pub fn size(&self) -> usize {
        let read_counters = self.read_counters.load(Ordering::SeqCst);
        let write_counters = self.write_counters.load(Ordering::SeqCst);
        counter_size(read_counters, write_counters, self.cap_mask + 1)
    }


    /// Returns the true if ring buffer is empty. Equivalent to `self.size() == 0`
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.size() == 0
    }

    /// Returns the maximum capacity of the ring buffer
    #[inline]
    pub fn cap(&self) -> usize {
        return self.cap_mask + 1;
    }

    /// Returns the remaining capacity of the ring buffer.
    /// This is equal to `self.cap() - self.size() - pending writes + pending reads`.
    #[inline]
    pub fn remaining_cap(&self) -> usize {
        let read_counters = self.read_counters.load(Ordering::SeqCst);
        let write_counters = self.write_counters.load(Ordering::SeqCst);
        let cap = self.cap_mask + 1;
        let read_index = read_counters.index();
        let write_index = write_counters.index();
        let size = if read_index <= write_index { write_index - read_index } else { write_index + cap - read_index };
        //size is from read_index to write_index, but we have to substract write_in_process_count for a better remaining capacity approximation
        cap - 1 - size - write_counters.in_process_count() as usize
    }

    /// Pop everything from ring buffer and discard it.
    #[inline]
    pub fn clear(&self) {
        while let Some(_) = self.try_pop() {}
    }

    /// Returns the memory usage in bytes of the allocated region of the ring buffer.
    /// This does not include overhead.
    pub fn memory_usage(&self) -> usize {
        unsafe { mem::size_of_val(&(*self.ptr)) }
    }

    /// pop one element, but only if ringbuffer is full, used by push_overwrite
    fn remove_if_full(&self) -> Option<T> {
        let mut read_counters = self.read_counters.load(Ordering::Acquire);

        let mut to_read_index;
        loop {
            let read_in_process_count = read_counters.in_process_count();
            // spin wait on 255 simultanous in progress writes
            if read_in_process_count == 255 {
                spin_loop_hint();
                read_counters = self.read_counters.load(Ordering::Acquire);
                continue;
            }

            if read_in_process_count > 0 {
                return None;
            }

            to_read_index = read_counters.index();
            if to_read_index.wrapping_add(1) & self.cap_mask == self.write_counters.load(Ordering::Acquire).index() {
                return None;
            }

            let new_counters = read_counters.increment_in_process();

            let existing = self.read_counters.compare_and_swap(read_counters, new_counters, Ordering::Acquire);
            if existing == read_counters {
                read_counters = new_counters;
                break;
            }
            read_counters = existing;
        }

        let popped = unsafe {
            // Read Memory
            ptr::read(&mut (*self.ptr)[to_read_index])
        };

        // Mark read as done
        loop {
            let new_counters = read_counters.increment_done(self.cap_mask);

            match self.read_counters.compare_and_exchange_weak(read_counters, new_counters, Ordering::Release, Ordering::Relaxed) {
                Ok(_) => {
                    break;
                }
                Err(n) => read_counters = n
            };
        }

        Some(popped)
    }
}


impl<T> fmt::Debug for AtomicRingBuffer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if f.alternate() {
            let cap = self.cap_mask + 1;
            let read_counters = self.read_counters.load(Ordering::Relaxed);
            let write_counters = self.write_counters.load(Ordering::Relaxed);
            write!(f, "AtomicRingBuffer cap: {} size: {} read_index: {}, read_in_process_count: {}, read_done_count: {}, write_index: {}, write_in_process_count: {}, write_done_count: {}", cap, self.size(),
                   read_counters.index(), read_counters.in_process_count(), read_counters.done_count(),
                   write_counters.index(), write_counters.in_process_count(), write_counters.done_count())
        } else {
            write!(f, "AtomicRingBuffer cap: {} size: {}", self.cap_mask + 1, self.size())
        }
    }
}


#[derive(Eq, PartialEq, Copy, Clone)]
pub struct Counters(usize);

fn counter_size(read_counters: Counters, write_counters: Counters, cap: usize) -> usize {
    let read_index = read_counters.index();
    let write_index = write_counters.index();
    let size = if read_index <= write_index { write_index - read_index } else { write_index + cap - read_index };
//size is from read_index to write_index, but we have to subtract read_in_process_count for a better size approximation
    size - (read_counters.in_process_count() as usize)
}

impl Counters {
    #[inline(always)]
    fn index(&self) -> usize {
        self.0 >> 16
    }

    #[inline(always)]
    fn in_process_count(&self) -> u8 {
        self.0 as u8
    }

    #[inline(always)]
    fn done_count(&self) -> u8 {
        (self.0 >> 8) as u8
    }


    #[inline(always)]
    fn increment_done(&self, cap_mask: usize) -> Counters {
        let in_process_count = self.in_process_count();
        // if the new read_done_count would equal read_in_process_count count
        Counters(if self.done_count() + 1 == in_process_count {
            // preserve write self.0, increment read_index and zero read_in_process_count and read_done_count (= commit)
            ((self.index().wrapping_add(in_process_count as usize) & cap_mask) << 16)
        } else {
            // otherwise we just increment read_done_count
            self.0 + (1 << 8)
        })
    }

    #[inline(always)]
    fn increment_in_process(&self) -> Counters {
        Counters(self.0 + 1)
    }
}


struct CounterStore {
    counters: ::std::sync::atomic::AtomicUsize,

}

impl CounterStore {
    pub fn new() -> CounterStore {
        CounterStore { counters: ::std::sync::atomic::AtomicUsize::new(0) }
    }
    #[inline(always)]
    pub fn load(&self, ordering: Ordering) -> Counters {
        Counters(self.counters.load(ordering))
    }
    #[inline(always)]
    pub fn compare_and_swap(&self, old: Counters, new: Counters, ordering: Ordering) -> Counters {
        Counters(self.counters.compare_and_swap(old.0, new.0, ordering))
    }
    #[inline(always)]
    pub fn compare_and_exchange_weak(&self, old: Counters, new: Counters, success: Ordering, failure: Ordering) -> Result<Counters, Counters> {
        match self.counters.compare_exchange_weak(old.0, new.0, success, failure) {
            Ok(_) => Ok(old),
            Err(previous) => Err(Counters(previous))
        }
    }
}


impl<T> Drop for AtomicRingBuffer<T> {
    fn drop(&mut self) {
        // drop contents
        self.clear();
        // drop memory box without dropping contents
        unsafe { Box::from_raw(self.ptr).into_vec().set_len(0); }
    }
}


#[cfg(test)]
mod tests {
    #[test]
    pub fn test_increments() {
        let mut read_counters = super::Counters(0);
        let mut write_counters = super::Counters(0);
        // 16 elements
        let cap = 16;
        let cap_mask = 0xf;


        for i in 0..8 {
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));

            write_counters = write_counters.increment_in_process();
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 1, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));

            write_counters = write_counters.increment_in_process();
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 2, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counters = write_counters.increment_in_process();
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 3, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counters = write_counters.increment_done(cap_mask);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 3, 1), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counters = write_counters.increment_done(cap_mask);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 3, 2), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counters = write_counters.increment_done(cap_mask);
            assert_eq!((3, (0 + i * 3) % 16, 0, 0, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));

            read_counters = read_counters.increment_in_process();
            assert_eq!((2, (0 + i * 3) % 16, 1, 0, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counters = read_counters.increment_in_process();
            assert_eq!((1, (0 + i * 3) % 16, 2, 0, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counters = read_counters.increment_in_process();
            assert_eq!((0, (0 + i * 3) % 16, 3, 0, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counters = read_counters.increment_done(cap_mask);
            assert_eq!((0, (0 + i * 3) % 16, 3, 1, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counters = read_counters.increment_done(cap_mask);
            assert_eq!((0, (0 + i * 3) % 16, 3, 2, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counters = read_counters.increment_done(cap_mask);
            assert_eq!((0, (3 + i * 3) % 16, 0, 0, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
        }
    }

    #[test]
    pub fn test_pushpop() {
        let ring = super::AtomicRingBuffer::with_capacity(900);
        assert_eq!(None, ring.try_pop());
        ring.push_overwrite(1);
        assert_eq!(Some(1), ring.try_pop());
        assert_eq!(None, ring.try_pop());

        for i in 0..5000 {
            ring.push_overwrite(i);
            assert_eq!(Some(i), ring.try_pop());
            assert_eq!(None, ring.try_pop());
        }


        for i in 0..199999 {
            ring.push_overwrite(i);
        }
        assert_eq!(ring.cap(), ring.size() + 1);
        assert_eq!(199999 - (ring.cap() - 1), ring.try_pop().unwrap());
        assert_eq!(Ok(()), ring.try_push(199999));

        for i in 200000 - (ring.cap() - 1)..200000 {
            assert_eq!(i, ring.try_pop().unwrap());
        }
    }

    #[test]
    pub fn test_pushpop_large() {
        let ring = super::AtomicRingBuffer::with_capacity(65535);


        assert_eq!(None, ring.try_pop());
        ring.push_overwrite(1);
        assert_eq!(Some(1), ring.try_pop());

        for i in 0..200000 {
            ring.push_overwrite(i);
            assert_eq!(Some(i), ring.try_pop());
        }


        for i in 0..200000 {
            ring.push_overwrite(i);
        }
        assert_eq!(ring.cap(), ring.size() + 1);

        for i in 200000 - (ring.cap() - 1)..200000 {
            assert_eq!(i, ring.try_pop().unwrap());
        }
    }

    #[test]
    pub fn test_pushpop_large2() {
        let ring = super::AtomicRingBuffer::with_capacity(65536);


        assert_eq!(None, ring.try_pop());
        ring.push_overwrite(1);
        assert_eq!(Some(1), ring.try_pop());

        for i in 0..200000 {
            ring.push_overwrite(i);
            assert_eq!(Some(i), ring.try_pop());
        }


        for i in 0..200000 {
            ring.push_overwrite(i);
        }
        assert_eq!(ring.cap(), ring.size() + 1);

        for i in 200000 - (ring.cap() - 1)..200000 {
            assert_eq!(i, ring.try_pop().unwrap());
        }
    }


    #[test]
    pub fn test_pushpop_large2_zerotype() {
        #[derive(Eq, PartialEq, Debug)]
        struct ZeroType {}

        let ring = super::AtomicRingBuffer::with_capacity(65536);

        assert_eq!(0, ring.memory_usage());

        assert_eq!(None, ring.try_pop());
        ring.push_overwrite(ZeroType {});
        assert_eq!(Some(ZeroType {}), ring.try_pop());

        for _i in 0..200000 {
            ring.push_overwrite(ZeroType {});
            assert_eq!(Some(ZeroType {}), ring.try_pop());
        }


        for _i in 0..200000 {
            ring.push_overwrite(ZeroType {});
        }
        assert_eq!(ring.cap(), ring.size() + 1);

        for _i in 200000 - (ring.cap() - 1)..200000 {
            assert_eq!(ZeroType {}, ring.try_pop().unwrap());
        }
    }


    #[test]
    pub fn test_threaded() {
        let cap = 65535;

        let buf: super::AtomicRingBuffer<usize> = super::AtomicRingBuffer::with_capacity(cap);
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
                    let a = pop_wait(&buf);
                    let b = pop_wait(&buf);
                    while let Err(_) = buf.try_push(a) {};
                    while let Err(_) = buf.try_push(b) {};
                }
            }));
        }
        for (_idx, handle) in handles.into_iter().enumerate() {
            handle.join().expect("join");
        }

        assert_eq!(arc.size(), cap);

        let mut expected: Vec<usize> = Vec::new();
        let mut actual: Vec<usize> = Vec::new();
        for i in 0..cap {
            expected.push(i);
            actual.push(arc.try_pop().expect("check"));
        }
        actual.sort_by(|&a, b| a.partial_cmp(b).unwrap());
        assert_eq!(actual, expected);
    }

    fn pop_wait(buf: &::std::sync::Arc<super::AtomicRingBuffer<usize>>) -> usize {
        loop {
            match buf.try_pop() {
                None => continue,
                Some(v) => return v,
            }
        }
    }
}

