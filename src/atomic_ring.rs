use std::fmt;
use std::marker::PhantomData;
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
///atomicring = "1.2.1"
///```
///
///
///And something like this to your code
///
///```rust
///
///// create an AtomicRingBuffer with capacity of 1023 elements (next power of two -1)
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
    mem: *mut [T],
    _marker: PhantomData<T>,
}

/// If T is Send, AtomicRingBuffer is Send + Sync
unsafe impl<T: Send> Send for AtomicRingBuffer<T> {}

/// Any particular `T` should never accessed concurrently, so T does not need to be Sync.
/// If T is Send, AtomicRingBuffer is Send + Sync
unsafe impl<T: Send> Sync for AtomicRingBuffer<T> {}

const MAXIMUM_IN_PROGRESS: u8 = 16;

impl<T: Default> AtomicRingBuffer<T> {
    /// Write an object from the ring buffer, passing an &mut pointer to a given function to write to during transaction. The cell will be initialized with Default::default()
    #[inline(always)]
    pub fn try_write<F: FnOnce(&mut T)>(&self, writer: F) -> Result<(), ()> {
        self.try_unsafe_write(|cell| unsafe {
            ptr::write(cell, Default::default());
            writer(&mut (*cell));
        })
    }
}

impl<T: Sized> AtomicRingBuffer<T> {
    /// Constructs a new empty AtomicRingBuffer<T> with the specified capacity
    /// the capacity is rounded up to the next power of 2 -1
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 1023 elements (next power of 2 from the given capacity)
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(900);
    ///
    /// assert_eq!(1023, ring.remaining_cap());
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
            mem,
            read_counters: CounterStore::new(),
            write_counters: CounterStore::new(),
            _marker: PhantomData,
        }
    }


    /// Try to push an object to the atomic ring buffer.
    /// If the buffer has no capacity remaining, the pushed object will be returned to the caller as error.
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 3 elements
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(3);
    /// assert_eq!(3, ring.remaining_cap());
    ///
    /// // try_push adds an element to the buffer, if there is room
    /// assert_eq!(Ok(()), ring.try_push(10));
    /// assert_eq!(Some(10), ring.try_pop());
    /// assert_eq!(None, ring.try_pop());
    ///
    /// for i in 0..3 {
    ///     assert_eq!(Ok(()), ring.try_push(i));
    /// }
    ///
    /// // try_push returns the element to the caller if the buffer is full
    /// assert_eq!(Err(3), ring.try_push(3));
    ///
    /// // existing values remain in the ring buffer
    /// for i in 0..3 {
    ///     assert_eq!(Some(i), ring.try_pop());
    /// }
    ///```
    #[inline(always)]
    pub fn try_push(&self, content: T) -> Result<(), T> {
        self.try_unsafe_write_or(content, |cell, content| { unsafe { ptr::write(cell, content); } }, |content| { content })
    }

    /// Write an object from the ring buffer, passing an uninitialized *mut pointer to a given fuction to write to during transaction.
    ///
    /// The content of the cell will *NOT* be initialized and has to be overwritten using ptr::write.
    ///
    /// The writer function is called once if there is room in the buffer
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 7 elements  (next power of 2 -1 from the given capacity)
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(5);
    /// assert_eq!(7, ring.remaining_cap());
    ///
    /// // try_unsafe_write adds an element to the buffer, if there is room
    /// assert_eq!(Ok(()), ring.try_unsafe_write(|cell| unsafe { ::std::ptr::write(cell, 10) }));
    /// assert_eq!(Some(10), ring.try_pop());
    /// assert_eq!(None, ring.try_pop());
    ///
    /// for i in 0..7 {
    ///     assert_eq!(Ok(()), ring.try_unsafe_write(|cell| unsafe { ::std::ptr::write(cell, i) }));
    /// }
    ///
    /// // try_unsafe_write returns an error to the caller if the buffer is full
    /// assert_eq!(Err(()), ring.try_unsafe_write(|cell| unsafe { ::std::ptr::write(cell, 7) }));
    ///
    /// // existing values remain in the ring buffer
    /// for i in 0..7 {
    ///     assert_eq!(Some(i), ring.try_pop());
    /// }
    ///```
    #[inline(always)]
    pub fn try_unsafe_write<F: FnOnce(*mut T)>(&self, writer: F) -> Result<(), ()> {
        self.try_unsafe_write_or((), |dst, _| { writer(dst) }, |_| { })
    }


    /// Write an object from the ring buffer, passing an uninitialized *mut pointer and an arbitrary content parameter to a given fuction to write to during transaction.
    ///
    /// The content of the cell will *NOT* be initialized and has to be overwritten using ptr::write.
    ///
    /// The writer function is called once if there is room in the buffer
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 7 elements  (next power of 2 -1 from the given capacity)
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(5);
    /// assert_eq!(7, ring.remaining_cap());
    ///
    /// // try_unsafe_write adds an element to the buffer, if there is room
    /// let writer = |cell, content| unsafe { ::std::ptr::write(cell, content) };
    /// let error = |content| {content};
    ///
    /// assert_eq!(Ok(()), ring.try_unsafe_write_or(10, writer, error));
    /// assert_eq!(Some(10), ring.try_pop());
    /// assert_eq!(None, ring.try_pop());
    ///
    /// for i in 0..7 {
    ///     assert_eq!(Ok(()), ring.try_unsafe_write_or(i, writer, error));
    /// }
    ///
    /// assert_eq!(0, ring.remaining_cap());
    ///
    /// // try_unsafe_write returns an error to the caller if the buffer is full
    /// assert_eq!(Err(7), ring.try_unsafe_write_or(7, writer, error));
    ///
    /// // existing values remain in the ring buffer
    /// for i in 0..7 {
    ///     assert_eq!(Some(i), ring.try_pop());
    /// }
    ///
    ///```
    #[inline(always)]
    pub fn try_unsafe_write_or<CNT, OK, ERR, W: FnOnce(*mut T, CNT) -> OK, E: FnOnce(CNT) -> ERR>(&self, content: CNT, writer: W, err: E) -> Result<OK, ERR> {
        let cap_mask = self.cap_mask();
        let error_condition = |to_write_index: usize, _: u8| { to_write_index.wrapping_add(1) & cap_mask == self.read_counters.load(Ordering::SeqCst).index() };

        if let Ok((write_counters, to_write_index)) = self.write_counters.increment_in_progress(error_condition, cap_mask) {

            // write mem
            let ok = unsafe {
                let cell = self.cell(to_write_index);
                writer(cell, content)
            };

            // Mark write as done
            self.write_counters.increment_done(write_counters, to_write_index, cap_mask);
            Ok(ok)
        } else {
            Err(err(content))
        }
    }


    /// Pushes an object to the atomic ring buffer.
    /// If the buffer is full, another object will be popped to make room for the new object.
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 3 elements
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(3);
    ///
    /// assert_eq!(3, ring.remaining_cap());
    ///
    /// // push_overwrite adds an element to the buffer, overwriting an older element
    ///
    /// ring.push_overwrite(10);
    /// assert_eq!(Some(10), ring.try_pop());
    /// assert_eq!(None, ring.try_pop());
    ///
    /// for i in 0..3 {
    ///     ring.push_overwrite(i);
    /// }
    /// // push_overwrite overwrites t first element
    /// ring.push_overwrite(3);
    ///
    /// // first value (0) was removed
    /// for i in 1..4 {
    ///     assert_eq!(Some(i), ring.try_pop());
    /// }
    ///
    ///```
    #[inline(always)]
    pub fn push_overwrite(&self, content: T) {
        let mut cont = content;
        loop {
            match self.try_push(cont) {
                Ok(_) => return,
                Err(ret) => {
                    self.remove_if_full();
                    cont = ret;
                }
            }
        }
    }

    /// Tries to pop an object from the ring buffer.
    /// If the buffer is empty this returns None, otherwise this returns Some(val)
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 3 elements
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(3);
    ///
    /// assert_eq!(Ok(()), ring.try_push(10));
    /// assert_eq!(Some(10), ring.try_pop());
    /// assert_eq!(None, ring.try_pop());
    ///
    ///```
    #[inline]
    pub fn try_pop(&self) -> Option<T> {
        self.try_read_nodrop(|cell| unsafe { ptr::read(cell) })
    }

    /// Read an object from the ring buffer, passing an &mut pointer to a given function to read during transaction.
    ///
    /// The given function is called with a mutable reference to the cell, and then the content in the cell is dropped
    ///
    /// If you do not want the content to be dropped use try_read_nodrop(..) or just use try_pop()
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 3 elements
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(3);
    ///
    /// assert_eq!(Ok(()), ring.try_push(10));
    /// assert_eq!(Some(10), ring.try_read(|cell| unsafe { *cell }));
    /// assert_eq!(None, ring.try_read(|cell| unsafe { *cell }));
    ///
    ///```
    #[inline]
    pub fn try_read<U, F: FnOnce(&mut T) -> U>(&self, reader: F) -> Option<U> {
        self.try_read_nodrop(|cell| unsafe {
            let result = reader(&mut (*cell));
            ptr::drop_in_place(cell);
            result
        })
    }

    /// Read an object from the ring buffer, passing an &mut pointer to a given function to read during transaction.
    ///
    /// The given function is called with a mutable reference to the cell. The content in the cell is not dropped after reading, so the given function must take ownership of the content ideally using ptr::read(cell)
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 3 elements
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(3);
    ///
    /// assert_eq!(Ok(()), ring.try_push(10));
    /// assert_eq!(Some(10), ring.try_read_nodrop(|cell| unsafe { *cell }));
    /// assert_eq!(None, ring.try_read_nodrop(|cell| unsafe { *cell }));
    ///
    ///```
    #[inline]
    pub fn try_read_nodrop<U, F: FnOnce(&mut T) -> U>(&self, reader: F) -> Option<U> {
        let cap_mask = self.cap_mask();

        let error_condition = |to_read_index: usize, _: u8| { to_read_index == self.write_counters.load(Ordering::SeqCst).index() };


        if let Ok((read_counters, to_read_index)) = self.read_counters.increment_in_progress(error_condition, cap_mask) {
            let popped = unsafe {
                let cell = self.cell(to_read_index);
                reader(&mut (*cell))
            };
            self.read_counters.increment_done(read_counters, to_read_index, cap_mask);
            Some(popped)
        } else {
            None
        }
    }


    /// Returns the number of objects stored in the ring buffer that are not in process of being removed.
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 3 elements
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(3);
    ///
    /// assert_eq!(0, ring.len());
    /// assert_eq!(Ok(()), ring.try_push(10));
    /// assert_eq!(1, ring.len());
    /// assert_eq!(Some(10), ring.try_pop());
    /// assert_eq!(None, ring.try_pop());
    /// assert_eq!(0, ring.len());
    ///
    ///```
    #[inline]
    pub fn len(&self) -> usize {
        let read_counters = self.read_counters.load(Ordering::SeqCst);
        let write_counters = self.write_counters.load(Ordering::SeqCst);
        counter_len(read_counters, write_counters, self.capacity())
    }


    /// Returns the true if ring buffer is empty. Equivalent to `self.len() == 0`
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 3 elements
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(3);
    ///
    /// assert_eq!(true, ring.is_empty());
    /// assert_eq!(Ok(()), ring.try_push(10));
    /// assert_eq!(false, ring.is_empty());
    /// assert_eq!(Some(10), ring.try_pop());
    /// assert_eq!(None, ring.try_pop());
    /// assert_eq!(true, ring.is_empty());
    ///
    ///```
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the maximum capacity of the ring buffer
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 3 elements
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(3);
    ///
    /// assert_eq!(true, ring.is_empty());
    /// assert_eq!(Ok(()), ring.try_push(10));
    /// assert_eq!(false, ring.is_empty());
    /// assert_eq!(Some(10), ring.try_pop());
    /// assert_eq!(None, ring.try_pop());
    /// assert_eq!(true, ring.is_empty());
    ///
    ///```
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        unsafe { (*self.mem).len() }
    }

    /// Returns the remaining capacity of the ring buffer.
    /// This is equal to `self.cap() - self.len() - pending writes + pending reads`.
    ///
    /// # Examples
    ///
    ///```
    /// // create an AtomicRingBuffer with capacity of 3 elements
    /// let ring = ::atomicring::AtomicRingBuffer::with_capacity(3);
    ///
    /// assert_eq!(3, ring.remaining_cap());
    /// assert_eq!(Ok(()), ring.try_push(10));
    /// assert_eq!(2, ring.remaining_cap());
    /// assert_eq!(Some(10), ring.try_pop());
    /// assert_eq!(3, ring.remaining_cap());
    ///
    ///```
    #[inline]
    pub fn remaining_cap(&self) -> usize {
        let read_counters = self.read_counters.load(Ordering::SeqCst);
        let write_counters = self.write_counters.load(Ordering::SeqCst);
        let cap = self.capacity();
        let read_index = read_counters.index();
        let write_index = write_counters.index();
        let len = if read_index <= write_index { write_index - read_index } else { write_index + cap - read_index };
        //len is from read_index to write_index, but we have to substract write_in_process_count for a better remaining capacity approximation
        cap - 1 - len - write_counters.in_process_count() as usize
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

    /// Returns a *mut T pointer to an indexed cell
    #[inline(always)]
    unsafe fn cell(&self, index: usize) -> *mut T {
        (&mut (*self.mem)[index])
    }

    /// Returns the capacity mask
    #[inline(always)]
    fn cap_mask(&self) -> usize {
        self.capacity() - 1
    }


    /// pop one element, but only if ringbuffer is full, used by push_overwrite
    fn remove_if_full(&self) -> Option<T> {
        let cap_mask = self.cap_mask();


        let error_condition = |to_read_index: usize, read_in_process_count: u8| { read_in_process_count > 0 || to_read_index.wrapping_add(1) & cap_mask == self.write_counters.load(Ordering::Acquire).index() };


        if let Ok((read_counters, to_read_index)) = self.read_counters.increment_in_progress(error_condition, cap_mask) {
            let popped = unsafe {
                // Read Memory
                ptr::read(self.cell(to_read_index))
            };

            self.read_counters.increment_done(read_counters, to_read_index, cap_mask);

            Some(popped)
        } else {
            None
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


impl<T> fmt::Debug for AtomicRingBuffer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if f.alternate() {
            let cap = self.capacity();
            let read_counters = self.read_counters.load(Ordering::Relaxed);
            let write_counters = self.write_counters.load(Ordering::Relaxed);
            write!(f, "AtomicRingBuffer cap: {} len: {} read_index: {}, read_in_process_count: {}, read_done_count: {}, write_index: {}, write_in_process_count: {}, write_done_count: {}", cap, self.len(),
                   read_counters.index(), read_counters.in_process_count(), read_counters.done_count(),
                   write_counters.index(), write_counters.in_process_count(), write_counters.done_count())
        } else {
            write!(f, "AtomicRingBuffer cap: {} len: {}", self.capacity(), self.len())
        }
    }
}


#[derive(Eq, PartialEq, Copy, Clone)]
pub struct Counters(usize);

impl Counters {
    #[inline(always)]
    const fn index(self) -> usize {
        self.0 >> 16
    }

    #[inline(always)]
    const fn in_process_count(self) -> u8 {
        self.0 as u8
    }

    #[inline(always)]
    const fn done_count(self) -> u8 {
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

fn counter_len(read_counters: Counters, write_counters: Counters, cap: usize) -> usize {
    let read_index = read_counters.index();
    let write_index = write_counters.index();
    let len = if read_index <= write_index { write_index - read_index } else { write_index + cap - read_index };
//len is from read_index to write_index, but we have to subtract read_in_process_count for a better approximation
    len - (read_counters.in_process_count() as usize)
}


struct CounterStore {
    counters: AtomicUsize,

}

impl CounterStore {
    pub const fn new() -> CounterStore {
        CounterStore { counters: AtomicUsize::new(0) }
    }
    #[inline(always)]
    pub fn load(&self, ordering: Ordering) -> Counters {
        Counters(self.counters.load(ordering))
    }
    #[inline(always)]
    pub fn increment_in_progress<F>(&self, error_condition: F, cap_mask: usize) -> Result<(Counters, usize), ()>
        where F: Fn(usize, u8) -> bool {

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


            let index = counters.index().wrapping_add(in_progress_count as usize) & cap_mask;

            if error_condition(index, in_progress_count) {
                return Err(());
            }

            let new_counters = Counters(counters.0.wrapping_add(1));
            match self.counters.compare_exchange_weak(counters.0, new_counters.0, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => return Ok((new_counters, index)),
                Err(updated) => counters = Counters(updated)
            };
        }
    }
    #[inline(always)]
    pub fn increment_done(&self, mut counters: Counters, index: usize, cap_mask: usize) {
        // ultra fast path: if we are first pending operation in line and nothing is done yet,
        // we can just increment index, decrement in_progress_count, preserve done_count without checking
        // if counters.index()==index && counters.done_count()==0
        if (counters.0 & 0x00FF_FFFF_FFFF_FF00 == (index << 16)) && (index < cap_mask) {
            // even if other operations are in progress
            self.counters.fetch_add((1 << 16) - 1, Ordering::Release);
            return;
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


#[cfg(test)]
mod tests {
    #[test]
    pub fn test_increments() {
        let read_counter_store = super::CounterStore::new();
        let write_counter_store = super::CounterStore::new();
        // 16 elements
        let cap = 16;
        let cap_mask = 0xf;


        let error_condition = |_, _| { false };
        for i in 0..8 {
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 0, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));

            write_counter_store.increment_in_progress(error_condition, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 1, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));

            write_counter_store.increment_in_progress(error_condition, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 2, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counter_store.increment_in_progress(error_condition, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 3, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counter_store.increment_done(write_counters, (0 + i * 3) % 16, cap_mask);
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((1, (0 + i * 3) % 16, 0, 0, (1 + i * 3) % 16, 2, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counter_store.increment_done(write_counters, (2 + i * 3) % 16, cap_mask);
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((1, (0 + i * 3) % 16, 0, 0, (1 + i * 3) % 16, 2, 1), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            write_counter_store.increment_done(write_counters, (1 + i * 3) % 16, cap_mask);
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((3, (0 + i * 3) % 16, 0, 0, (3 + i * 3) % 16, 0, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));

            read_counter_store.increment_in_progress(error_condition, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((2, (0 + i * 3) % 16, 1, 0, (3 + i * 3) % 16, 0, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counter_store.increment_in_progress(error_condition, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((1, (0 + i * 3) % 16, 2, 0, (3 + i * 3) % 16, 0, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counter_store.increment_in_progress(error_condition, cap_mask).expect("..");
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 3, 0, (3 + i * 3) % 16, 0, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counter_store.increment_done(read_counters, (1 + i * 3) % 16, cap_mask);

            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (0 + i * 3) % 16, 3, 1, (3 + i * 3) % 16, 0, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counter_store.increment_done(read_counters, (0 + i * 3) % 16, cap_mask);
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (1 + i * 3) % 16, 2, 1, (3 + i * 3) % 16, 0, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
            read_counter_store.increment_done(read_counters, (2 + i * 3) % 16, cap_mask);
            let read_counters = read_counter_store.load(super::Ordering::Relaxed);
            let write_counters = write_counter_store.load(super::Ordering::Relaxed);
            assert_eq!((0, (3 + i * 3) % 16, 0, 0, (3 + i * 3) % 16, 0, 0), (super::counter_len(read_counters, write_counters, cap), read_counters.index(), read_counters.in_process_count(), read_counters.done_count(), write_counters.index(), write_counters.in_process_count(), write_counters.done_count()));
        }
    }

    #[test]
    pub fn test_pushpop() {
        let ring = super::AtomicRingBuffer::with_capacity(900);
        assert_eq!(1024, ring.capacity());
        assert_eq!(None, ring.try_pop());
        ring.push_overwrite(1);
        assert_eq!(Some(1), ring.try_pop());
        assert_eq!(None, ring.try_pop());

        // Test push in an empty buffer
        for i in 0..5000 {
            ring.push_overwrite(i);
            assert_eq!(Some(i), ring.try_pop());
            assert_eq!(None, ring.try_pop());
        }


        // Test push in a full buffer
        for i in 0..199999 {
            ring.push_overwrite(i);
        }
        assert_eq!(ring.capacity(), ring.len() + 1);
        assert_eq!(199999 - (ring.capacity() - 1), ring.try_pop().unwrap());
        assert_eq!(Ok(()), ring.try_push(199999));

        for i in 200000 - (ring.capacity() - 1)..200000 {
            assert_eq!(i, ring.try_pop().unwrap());
        }

        // Test push in an almost full buffer
        for i in 0..1023 {
            ring.try_push(i).expect("push")
        }
        assert_eq!(1024, ring.capacity());
        assert_eq!(1023, ring.len());
        for i in 0..1023 {
            assert_eq!(ring.try_pop(), Some(i));
            ring.try_push(i).expect("push")
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
        assert_eq!(ring.capacity(), ring.len() + 1);

        for i in 200000 - (ring.capacity() - 1)..200000 {
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
        assert_eq!(ring.capacity(), ring.len() + 1);

        for i in 200000 - (ring.capacity() - 1)..200000 {
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
        assert_eq!(ring.capacity(), ring.len() + 1);

        for _i in 200000 - (ring.capacity() - 1)..200000 {
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
                    //buf.try_push(a).expect("push");
                    //buf.try_push(b).expect("push");
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
            actual.push(arc.try_pop().expect("check"));
        }
        actual.sort_by(|&a, b| a.partial_cmp(b).unwrap());
        assert_eq!(actual, expected);
    }

    #[test]
    pub fn test_push_overwrite() {
        // create an AtomicRingBuffer with capacity of 8 elements (next power of 2 from the given capacity)
        let ring = super::AtomicRingBuffer::with_capacity(3);

        // push_overwrite adds an element to the buffer, overwriting an older element

        ring.push_overwrite(10);
        assert_eq!(Some(10), ring.try_pop());
        assert_eq!(None, ring.try_pop());

        for i in 0..3 {
            assert_eq!(Ok(()), ring.try_push(i));
        }
        assert_eq!(3, ring.len());

        ring.push_overwrite(3);

        assert_eq!(Some(1), ring.try_pop());
        assert_eq!(Some(2), ring.try_pop());
        assert_eq!(Some(3), ring.try_pop());
    }

    static DROP_COUNT: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::AtomicUsize::new(0);

    #[allow(dead_code)]
    #[derive(Debug, Default)]
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

    #[test]
    pub fn test_inline_dropcount() {
        DROP_COUNT.store(0, ::std::sync::atomic::Ordering::Relaxed);
        {
            let buf: super::AtomicRingBuffer<TestType> = super::AtomicRingBuffer::with_capacity(1024);
            buf.try_write(|w| { w.some = 0 }).expect("push");
            buf.try_write(|w| { w.some = 0 }).expect("push");

            assert_eq!(0, DROP_COUNT.load(::std::sync::atomic::Ordering::Relaxed));
            buf.try_read(|_| {});
            assert_eq!(1, DROP_COUNT.load(::std::sync::atomic::Ordering::Relaxed));
        }
        assert_eq!(2, DROP_COUNT.load(::std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    pub fn test_unsafe_dropcount() {
        DROP_COUNT.store(0, ::std::sync::atomic::Ordering::Relaxed);
        {
            let buf: super::AtomicRingBuffer<TestType> = super::AtomicRingBuffer::with_capacity(1024);
            buf.try_unsafe_write(|w| unsafe { ::std::ptr::write(w, TestType { some: 0 }) }).expect("push");
            buf.try_unsafe_write(|w| unsafe { ::std::ptr::write(w, TestType { some: 0 }) }).expect("push");

            assert_eq!(0, DROP_COUNT.load(::std::sync::atomic::Ordering::Relaxed));
            buf.try_read(|_| {});
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

