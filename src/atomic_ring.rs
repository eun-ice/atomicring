use std::fmt;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering, spin_loop_hint};


///A constant-size almost lock-free concurrent ring buffer
///
///Upsides
///
///- fast, try_push and pop are O(1)
///- scales well even during heavy concurrency
///- only 4 words of memory overhead
///- no memory allocations after initial creation
///
///
///Downsides
///
///- growing/shrinking is not supported
///- no blocking poll support (see AtomicRingQueue for blocking poll support)
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
///
///## Usage
///
///To use AtomicRingBuffer, add this to your `Cargo.toml`:
///
///```toml
///[dependencies]
///atomicring = "0.5.1"
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
    #[cfg(not(feature = "index_access"))]
    ptr: *mut T,
    mem: *mut [T],
}

/// If T is Send, AtomicRingBuffer is Send + Sync
unsafe impl<T: Send> Send for AtomicRingBuffer<T> {}

/// Any particular `T` should never accessed concurrently, so T does not need to be Sync.
/// If T is Send, AtomicRingBuffer is Send + Sync
unsafe impl<T: Send> Sync for AtomicRingBuffer<T> {}

const MAXIMUM_IN_PROGRESS: u8 = 16;

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

        /* allocate using a Vec */
        let mut content: Vec<T> = Vec::with_capacity(cap);
        unsafe { content.set_len(cap); }

        /* Zero memory content
        for i in content.iter_mut() {
            unsafe { ptr::write(i, mem::zeroed()); }
        }
        */
        let mem = Box::into_raw(content.into_boxed_slice());

        AtomicRingBuffer {
            #[cfg(not(feature = "index_access"))]
            ptr: unsafe { ::std::mem::transmute(&mut (*mem)[0]) },
            mem,
            read_counters: CounterStore::new(),
            write_counters: CounterStore::new(),

        }
    }

    /// Returns a *mut T pointer to an indexed cell
    #[cfg(feature = "index_access")]
    unsafe fn cell(&self, index: usize) -> *mut T {
        (&mut (*self.mem)[index])
    }


    /// Returns a *mut T pointer to an indexed cell
    #[cfg(not(feature = "index_access"))]
    unsafe fn cell(&self, index: usize) -> *mut T {
        self.ptr.offset(index as isize)
    }

    /// Returns the capacity mask
    #[inline(always)]
    fn cap_mask(&self) -> usize {
        self.cap() - 1
    }

    /// Try to push an object to the atomic ring buffer.
    /// If the buffer has no capacity remaining, the pushed object will be returned to the caller as error.
    #[inline(always)]
    pub fn try_push(&self, content: T) -> Result<(), T> {
        let cap_mask = self.cap_mask();
        let recheck = |to_write_index: usize, _: u8| { to_write_index.wrapping_add(1) & cap_mask == self.read_counters.load(Ordering::SeqCst).index() };

        if let Ok((write_counters, to_write_index)) = self.write_counters.increment_in_progress(recheck, cap_mask) {

            // write mem
            unsafe {
                ptr::write_unaligned(self.cell(to_write_index), content);
            }

            // Mark write as done
            self.write_counters.increment_done(write_counters, to_write_index, cap_mask);
            Ok(())
        } else {
            Err(content)
        }
    }

    /// Pushes an object to the atomic ring buffer.
    /// If the buffer is full, another object will be popped to make room for the new object.
    #[inline(always)]
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
    #[inline]
    pub fn try_pop(&self) -> Option<T> {
        let cap_mask = self.cap_mask();


        let recheck = |to_read_index: usize, _: u8| { to_read_index == self.write_counters.load(Ordering::SeqCst).index() };


        if let Ok((read_counters, to_read_index)) = self.read_counters.increment_in_progress(recheck, cap_mask) {
            let popped = unsafe {
                // Read Memory
                ptr::read_unaligned(self.cell(to_read_index))
            };

            self.read_counters.increment_done(read_counters, to_read_index, cap_mask);
            Some(popped)
        } else {
            None
        }
    }

    /// Returns the number of objects stored in the ring buffer that are not in process of being removed.
    #[inline]
    pub fn size(&self) -> usize {
        let read_counters = self.read_counters.load(Ordering::SeqCst);
        let write_counters = self.write_counters.load(Ordering::SeqCst);
        counter_size(read_counters, write_counters, self.cap())
    }


    /// Returns the true if ring buffer is empty. Equivalent to `self.size() == 0`
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.size() == 0
    }

    /// Returns the maximum capacity of the ring buffer
    #[inline(always)]
    pub fn cap(&self) -> usize {
        return unsafe { (*self.mem).len() };
    }

    /// Returns the remaining capacity of the ring buffer.
    /// This is equal to `self.cap() - self.size() - pending writes + pending reads`.
    #[inline]
    pub fn remaining_cap(&self) -> usize {
        let read_counters = self.read_counters.load(Ordering::SeqCst);
        let write_counters = self.write_counters.load(Ordering::SeqCst);
        let cap = self.cap();
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
        unsafe { mem::size_of_val(&(*self.mem)) }
    }

    /// pop one element, but only if ringbuffer is full, used by push_overwrite
    fn remove_if_full(&self) -> Option<T> {
        let cap_mask = self.cap_mask();


        let recheck = |to_read_index: usize, read_in_process_count: u8| { read_in_process_count > 0 || to_read_index.wrapping_add(1) & cap_mask == self.write_counters.load(Ordering::Acquire).index() };


        if let Ok((read_counters, to_read_index)) = self.read_counters.increment_in_progress(recheck, cap_mask) {
            let popped = unsafe {
                // Read Memory
                ptr::read_unaligned(self.cell(to_read_index))
            };

            self.read_counters.increment_done(read_counters, to_read_index, cap_mask);

            Some(popped)
        } else {
            None
        }
    }
}


impl<T> fmt::Debug for AtomicRingBuffer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if f.alternate() {
            let cap = self.cap();
            let read_counters = self.read_counters.load(Ordering::Relaxed);
            let write_counters = self.write_counters.load(Ordering::Relaxed);
            write!(f, "AtomicRingBuffer cap: {} size: {} read_index: {}, read_in_process_count: {}, read_done_count: {}, write_index: {}, write_in_process_count: {}, write_done_count: {}", cap, self.size(),
                   read_counters.index(), read_counters.in_process_count(), read_counters.done_count(),
                   write_counters.index(), write_counters.in_process_count(), write_counters.done_count())
        } else {
            write!(f, "AtomicRingBuffer cap: {} size: {}", self.cap(), self.size())
        }
    }
}


#[derive(Eq, PartialEq, Copy, Clone)]
pub struct Counters(usize);

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
}

impl From<usize> for Counters {
    fn from(val: usize) -> Counters {
        Counters(val)
    }
}

impl From<Counters> for usize {
    fn from(val: Counters) -> usize {
        val.0
    }
}

fn counter_size(read_counters: Counters, write_counters: Counters, cap: usize) -> usize {
    let read_index = read_counters.index();
    let write_index = write_counters.index();
    let size = if read_index <= write_index { write_index - read_index } else { write_index + cap - read_index };
//size is from read_index to write_index, but we have to subtract read_in_process_count for a better size approximation
    size - (read_counters.in_process_count() as usize)
}


struct CounterStore {
    counters: AtomicUsize,

}

impl CounterStore {
    pub fn new() -> CounterStore {
        CounterStore { counters: AtomicUsize::new(0) }
    }
    #[inline(always)]
    pub fn load(&self, ordering: Ordering) -> Counters {
        Counters(self.counters.load(ordering))
    }
    #[inline(always)]
    pub fn increment_in_progress<F>(&self, recheck: F, cap_mask: usize) -> Result<(Counters, usize), ()>
        where F: Fn(usize, u8) -> bool {
        /*        let counters = Counters(self.counters.fetch_add(1, Ordering::Acquire).wrapping_add(1));
                return Ok((counters,counters.index() & cap_mask));
        */
        let mut index;

        // Mark write as in progress
        let mut counters = self.load(Ordering::Acquire);
        loop {
            let in_progress_count = counters.in_process_count();
            // spin wait on 255 simultanous in progress writes
            if in_progress_count == MAXIMUM_IN_PROGRESS {
                spin_loop_hint();
                counters = self.load(Ordering::Acquire);
                continue;
            }


            index = counters.index().wrapping_add(in_progress_count as usize) & cap_mask;

            if recheck(index, in_progress_count) {
                return Err(());
            }

            let new_counters = Counters(counters.0.wrapping_add(1));
            match self.counters.compare_exchange_weak(counters.0, new_counters.0, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => {
                    return Ok((new_counters, index));
                }
                Err(updated) => counters = Counters(updated)
            };
        }
    }
    #[inline(always)]
    pub fn increment_done(&self, mut counters: Counters, index: usize, cap_mask: usize) {
        // ultra fast path: if we are first pending operation in line and nothing is done yet,
        // we can just increment index, decrement in_progress_count, preserve done_count without checking
        if counters.index() == index && counters.done_count() == 0 {
            if index < cap_mask {
                // even if other operations are in progress
                self.counters.fetch_add((1 << 16) - 1, Ordering::Release);
                return;
            }
        }

        loop {
            let in_process_count = counters.in_process_count();
            let new_counters = Counters(if counters.done_count().wrapping_add(1) == in_process_count {
                // if the new done_count equals in_process_count count commit:
                // increment read_index and zero read_in_process_count and read_done_count
                (counters.index().wrapping_add(in_process_count as usize) & cap_mask) << 16
            } else if counters.index() == index {
                // fast path: if we are first pending operation in line, increment index, decrement in_progress_count, preserve done_count, even if other operations are pending
                counters.0.wrapping_add((1 << 16) - 1) & ((cap_mask << 16) | 0xFFFF)
            } else {
                // otherwise we just increment read_done_count
                counters.0.wrapping_add(1 << 8)
            });


            match self.counters.compare_exchange_weak(counters.0, new_counters.0, Ordering::Release, Ordering::Relaxed) {
                Ok(_) => return,
                Err(updated) => counters = Counters(updated)
            };
        }
    }

}


impl<T> Drop for AtomicRingBuffer<T> {
    fn drop(&mut self) {
        // drop contents
        self.clear();
        // drop memory box without dropping contents
        unsafe { Box::from_raw(self.mem).into_vec().set_len(0); }
    }
}


#[cfg(test)]
mod tests {
    #[test]
    pub fn test_increments() {
        let read_counter_store = super::CounterStore::new();
        let write_counter_store = super::CounterStore::new();
        // 16 elements
        let cap = 16;
        let cap_mask = 0xf;


        let recheck = |_, _| { false };
        for i in 0..8 {
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));

            write_counter_store.increment_in_progress(recheck, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 1, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));

            write_counter_store.increment_in_progress(recheck, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 2, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counter_store.increment_in_progress(recheck, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 3, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counter_store.increment_done(write_counters, (0 + i * 3) % 16, cap_mask);
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((1, (0 + i * 3) % 16, 0, 0, (1 + i * 3) % 16, 2, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counter_store.increment_done(write_counters, (2 + i * 3) % 16, cap_mask);
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((1, (0 + i * 3) % 16, 0, 0, (1 + i * 3) % 16, 2, 1), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counter_store.increment_done(write_counters, (1 + i * 3) % 16, cap_mask);
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((3, (0 + i * 3) % 16, 0, 0, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));

            read_counter_store.increment_in_progress(recheck, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((2, (0 + i * 3) % 16, 1, 0, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counter_store.increment_in_progress(recheck, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((1, (0 + i * 3) % 16, 2, 0, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counter_store.increment_in_progress(recheck, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 3, 0, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counter_store.increment_done(read_counters, (1 + i * 3) % 16, cap_mask);

            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 3, 1, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counter_store.increment_done(read_counters, (0 + i * 3) % 16, cap_mask);
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (1 + i * 3) % 16, 2, 1, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counter_store.increment_done(read_counters, (2 + i * 3) % 16, cap_mask);
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (3 + i * 3) % 16, 0, 0, (3 + i * 3) % 16, 0, 0), (super::counter_size(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
        }
    }

    #[test]
    pub fn test_pushpop() {
        let ring = super::AtomicRingBuffer::with_capacity(900);
        assert_eq!(1024, ring.cap());
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

    static DROP_COUNT: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::ATOMIC_USIZE_INIT;

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
            let buf: super::AtomicRingBuffer<TestType> = super::AtomicRingBuffer::with_capacity(1024);
            buf.try_push(TestType { some: 0 }).expect("push");
            buf.try_push(TestType { some: 0 }).expect("push");

            assert_eq!(0, DROP_COUNT.load(::std::sync::atomic::Ordering::Relaxed));
            buf.try_pop();
            assert_eq!(1, DROP_COUNT.load(::std::sync::atomic::Ordering::Relaxed));
        }
        assert_eq!(2, DROP_COUNT.load(::std::sync::atomic::Ordering::Relaxed));
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

