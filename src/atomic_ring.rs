use std::fmt;
use std::mem;
use std::ptr;
use std::sync::atomic::{Ordering, spin_loop_hint};
use atomic64::atomic64::AtomicU64;

///A constant-size lock-free and almost wait-free ring buffer
///
///
///Upsides
///
///- Very fast
///- Writes and reads should be approx. O(1) even during heavy concurrency
///- No memory allocation after it is created
///
///Downsides:
///
///- Fixed size, growing/shrinking is not supported
///- No blocking wait/poll
///- only efficient on 64bit architectures
///- maximum capacity of 65536 entries
///- capacity is rounded up to the next power of 2
///
///# Implementation details
///
///This implementation uses a 64 Bit atomic to store the state
///
///```Text
///+64------+56------+48--------------+32------+24------+16-------------0+
///| w_done | w_pend |  write_index   | r_done | r_pend |   read_index   |
///+--------+--------+----------------+--------+--------+----------------+
///```
///
///- write_index/read_index (16bit): current read/write position in the ring buzffer.
///- r_pend/w_pend (8bit): number of pending concurrent read/writes
///- r_done/w_done (8bit): number of completed read/writes.
///
///For reading r_pend is incremented first, then the content of the ring buffer is read from memory.
///After reading is done r_done is incremented. read_index is only incremented if r_done is equal to r_pend.
///
///For writing first w_pend is incremented, then the content of the ring buffer is updated.
///After writing w_done is incremented. If w_done is equal to w_pend then both are set to 0 and write_index is incremented.
///
///In rare cases this can result in a race where multiple threads increment r_pend and r_done never quite reaches r_pend.
///If r_pend == 255 or w_pend == 255 a spinloop waits it to be <255 to continue.
///
///
///
///
///# Dependencies
///
///This package only depends on [atomic64](https://github.com/obourgain/rust-atomic64)
///
///# Usage
///
///To use AtomicRingBuffer, add this to your `Cargo.toml`:
///
///```toml
///[dependencies]
///atomicring = "0.1.0"
///```
///
///
///And something like this to your code
///
///```rust
///
///// create an AtomicRingBuffer with capacity of 1024 elements 
///let ring = ::atomicring::AtomicRingBuffer::new(900);
///
///// try_pop removes an element of the buffer and returns None if the buffer is empty
///assert_eq!(None, ring.try_pop());
///// push_overwrite adds an element to the buffer, overwriting the oldest element if the buffer is full: 
///ring.push_overwrite(1);
///assert_eq!(Some(1), ring.try_pop());
///assert_eq!(None, ring.try_pop());
///```

pub struct AtomicRingBuffer<T: Sized> {
    cap: u32,
    cap_mask: u16,
    memory: Option<Box<[T]>>,
    ptr: *mut T,
    counters: AtomicU64,
}

// Any particular `T` should never accessed concurrently, so T does not need to be Sync
// If T is Send, AtomicRingBuffer is Send + Sync
unsafe impl<T: Send> Send for AtomicRingBuffer<T> {}

unsafe impl<T: Send> Sync for AtomicRingBuffer<T> {}


impl<T: Sized> AtomicRingBuffer<T> {
    /// create a new instance of an AtomicRingBuffer with the given capacity
    /// the capacity is rounded up to the next power of 2
    pub fn new(mut capacity: usize) -> AtomicRingBuffer<T> {
        if capacity > (::std::u16::MAX as usize) + 1 {
            panic!("too large!");
        }

        let mut cap_mask: u16 = 1;

        loop {
            if capacity <= (cap_mask as usize) + 1 {
                break;
            }
            cap_mask = (cap_mask << 1) + 1;
        }
        let cap = cap_mask as u32 + 1;

        /* allocate using a Vec */
        let mut content: Vec<T> = Vec::with_capacity(cap as usize);
        unsafe { content.set_len(cap as usize); }

        /* Zero memory content
        for i in content.iter_mut() {
            unsafe { ptr::write(i, mem::zeroed()); }
        }
        */

        let mut memory = content.into_boxed_slice();
        let ptr = memory.as_mut_ptr();

        AtomicRingBuffer {
            cap,
            cap_mask,
            ptr,
            memory: Some(memory),
            counters: AtomicU64::new(0),
        }
    }

    /// tries to write an object to the atomic ring buffer.
    /// If the buffer is full, the object will be returned to the caller as error
    #[inline]
    pub fn try_push(&self, content: T) -> Result<(), T> {
        let mut to_write_index;


        // increment write_in_progress
        let mut counters = self.counters.load(Ordering::Acquire) as u64;
        loop {
            // spin wait on 255 simultanous in progress writes
            if write_in_process_count_full(counters) {
                spin_loop_hint();
                counters = self.counters.load(Ordering::Acquire) as u64;
                continue;
            }
            let write_in_progress_count = write_in_process_count(counters) as u16;
            let write_idx = write_index(counters);

            to_write_index = write_idx.wrapping_add(write_in_progress_count) & self.cap_mask;

            if to_write_index.wrapping_add(1) & self.cap_mask == read_index(counters) {
                // spin wait if we want to add to full atomic ring and reads are in progress
                if read_in_process_count(counters) > 0 {
                    spin_loop_hint();
                    counters = self.counters.load(Ordering::Acquire) as u64;
                    continue;
                }
                return Err(content);
            }

            let new_counters = increment_write_in_process(counters);

            let existing = self.counters.compare_and_swap(counters, new_counters, Ordering::Acquire) as u64;

            if existing == counters {
                counters = new_counters;
                break;
            }
            counters = existing;
        }

        // write mem
        unsafe {
            // we do not need write_volatile here?
            // ptr::write_volatile(self.ptr.offset(to_write_index as isize), content);
            ptr::write(self.ptr.offset(to_write_index as isize), content);
        }

        // increment write_done
        loop {
            let new_counters = increment_write_done(counters, self.cap_mask);

            let existing = self.counters.compare_and_swap(counters, new_counters, Ordering::Release) as u64;

            if existing == counters {
                return Ok(());
            }
            counters = existing;
        }
    }

    /// tries to write an object to the atomic ring buffer.
    /// If the buffer is full, another object will be popped and overwritten
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
    #[inline]
    pub fn try_pop(&self) -> Option<T> {
        let mut counters = self.counters.load(Ordering::Acquire) as u64;

        let mut to_read_index;
        loop {
            // spin wait on 255 simultanous in progress reads
            if read_in_process_count_full(counters) {
                spin_loop_hint();
                counters = self.counters.load(Ordering::Acquire) as u64;
                continue;
            }

            to_read_index = read_index(counters).wrapping_add(read_in_process_count(counters) as u16) & self.cap_mask;
            if to_read_index == write_index(counters) {
                return None;
            }

            let new_counters = increment_read_in_process(counters);

            let existing = self.counters.compare_and_swap(counters, new_counters, Ordering::Acquire) as u64;
            if existing == counters {
                counters = new_counters;
                break;
            }
            counters = existing;
        }

        let popped = unsafe {
            // Read Memory
            // we do not need read_volatile here?
            // ptr::read_volatile(self.ptr.offset(to_read_index as isize))
            ptr::read(self.ptr.offset(to_read_index as isize))
        };

        loop {
            let new_counters = increment_read_done(counters, self.cap_mask);

            let existing = self.counters.compare_and_swap(counters, new_counters, Ordering::Release) as u64;
            if existing == counters {
                break;
            }
            counters = existing;
        }

        Some(popped)
    }

    /// return the number of objects remaining in the ring buffer
    #[inline]
    pub fn size(&self) -> usize {
        size(self.counters.load(Ordering::Relaxed) as u64, self.cap)
    }


    /// check if ring buffer is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        size(self.counters.load(Ordering::Relaxed) as u64, self.cap) == 0
    }

    /// return the capacity of the ring buffer
    #[inline]
    pub fn cap(&self) -> usize {
        return self.cap as usize;
    }

    /// return the remaining capacity of the ring buffer, corresponds to cap()-size()-pending writes+pending reads
    #[inline]
    pub fn remaining_cap(&self) -> usize {
        remaining_cap(self.counters.load(Ordering::Relaxed) as u64, self.cap)
    }

    /// pop everything from ring buffer
    #[inline]
    pub fn clear(&self) {
        while let Some(_) = self.try_pop() {}
    }

    /// return memory usage in bytes of the allocated region of the ring buffer
    pub fn memory_usage(&self) -> usize {
        (self.cap as usize) * mem::size_of_val(&self.memory.as_ref().unwrap()[0])
    }

    /// pop one element, but only if ringbuffer is full, used by push_overwrite
    fn remove_if_full(&self) -> Option<T> {
        let mut counters = self.counters.load(Ordering::Acquire) as u64;

        let mut to_read_index;
        loop {
            // spin wait on 255 simultanous in progress writes
            if read_in_process_count_full(counters) {
                spin_loop_hint();
                counters = self.counters.load(Ordering::Acquire) as u64;
                continue;
            }

            if read_in_process_count(counters) > 0 {
                return None;
            }

            to_read_index = read_index(counters);
            if to_read_index.wrapping_add(1) & self.cap_mask == write_index(counters) {
                return None;
            }

            let new_counters = increment_read_in_process(counters);

            let existing = self.counters.compare_and_swap(counters, new_counters, Ordering::Acquire) as u64;
            if existing == counters {
                counters = new_counters;
                break;
            }
            counters = existing;
        }

        let popped = unsafe {
            // Read Memory
            // wo do not need read_volatile here?
            // ptr::read_volatile(self.ptr.offset(to_read_index as isize))
            ptr::read(self.ptr.offset(to_read_index as isize))
        };

        loop {
            let new_counters = increment_read_done(counters, self.cap_mask);

            let existing = self.counters.compare_and_swap(counters, new_counters, Ordering::Release) as u64;
            if existing == counters {
                break;
            }
            counters = existing;
        }

        Some(popped)
    }
}


impl<T> fmt::Debug for AtomicRingBuffer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if f.alternate() {
            let cap = self.cap;
            let counters = self.counters.load(Ordering::Relaxed) as u64;
            write!(f, "AtomicRingBuffer cap: {} size: {} read_index: {}, read_in_process_count: {}, read_done_count: {}, write_index: {}, write_in_process_count: {}, write_done_count: {}", cap, size(counters, cap), read_index(counters), read_in_process_count(counters), read_done_count(counters), write_index(counters), write_in_process_count(counters), write_done_count(counters))
        } else {
            write!(f, "AtomicRingBuffer cap: {} size: {}", self.cap, self.size())
        }
    }
}


#[inline(always)]
fn read_index(counters: u64) -> u16 {
    (counters & 0xFFFF) as u16
}

#[inline(always)]
fn read_in_process_count(counters: u64) -> u8 {
    ((counters >> 16) & 0xFF) as u8
}

#[inline(always)]
fn read_in_process_count_full(counters: u64) -> bool {
    counters & (0xFF << 16) == (0xFF << 16)
}

#[inline(always)]
fn read_done_count(counters: u64) -> u8 {
    ((counters >> 24) & 0xFF) as u8
}


#[inline(always)]
fn write_index(counters: u64) -> u16 {
    ((counters >> 32) & 0xFFFF) as u16
}

#[inline(always)]
fn write_in_process_count(counters: u64) -> u8 {
    ((counters >> 48) & 0xFF) as u8
}

#[inline(always)]
fn write_in_process_count_full(counters: u64) -> bool {
    counters & (0xFF << 48) == (0xFF << 48)
}

#[inline(always)]
fn write_done_count(counters: u64) -> u8 {
    ((counters >> 56) & 0xFF) as u8
}

#[inline(always)]
fn increment_read_done(counters: u64, cap_mask: u16) -> u64 {
    let read_in_process_count = read_in_process_count(counters);
    // if the new read_done_count would equal read_in_process_count count
    if read_done_count(counters) + 1 == read_in_process_count {
        // preserve write counters, increment read_index and zero read_in_process_count and read_done_count (= commit)
        (counters & 0xFFFFFFFF00000000) |
            ((read_index(counters).wrapping_add(read_in_process_count as u16) & cap_mask) as u64)
    } else {
        // otherwise we just increment read_done_count
        counters + (1 << 24)
    }
}

#[inline(always)]
fn increment_read_in_process(counters: u64) -> u64 {
    counters + (1 << 16)
}

#[inline(always)]
fn increment_write_done(counters: u64, cap_mask: u16) -> u64 {
    let write_in_process_count = write_in_process_count(counters);

    // if the new write_done_count would equal write_in_process_count
    if write_done_count(counters) + 1 == write_in_process_count {
        // preserve read counters, increment write_index and zero write_in_process_count and write_done_count
        ((((write_index(counters).wrapping_add(write_in_process_count as u16)) & cap_mask) as u64) << 32)
            | counters & 0xFFFFFFFF
    } else {
        // otherwise we just increment write_done_count
        counters + (1 << 56)
    }
}


#[inline(always)]
fn increment_write_in_process(counters: u64) -> u64 {
    counters as u64 + (1 << 48)
}

#[inline(always)]
fn size(counters: u64, cap: u32) -> usize {
    let read_index = read_index(counters);
    let write_index = write_index(counters);
    let size = if read_index <= write_index { write_index as usize - read_index as usize } else { write_index as usize + cap as usize - read_index as usize };
    //size is from read_index to write_index, but we have to substract read_in_process_count for a better size approximation
    size - read_in_process_count(counters) as usize
}


#[inline(always)]
fn remaining_cap(counters: u64, cap: u32) -> usize {
    let read_index = read_index(counters);
    let write_index = write_index(counters);
    let size = if read_index <= write_index { write_index as usize - read_index as usize } else { write_index as usize + cap as usize - read_index as usize };
    //size is from read_index to write_index, but we have to substract write_in_process_count for a better remaining capacity approximation
    cap as usize - 1 - size - write_in_process_count(counters) as usize
}


impl<T> Drop for AtomicRingBuffer<T> {
    fn drop(&mut self) {
        self.clear();
        // forget memory to prevent multiple calls to drop
        mem::forget(self.memory.take());
    }
}


#[cfg(test)]
mod tests {
    #[test]
    pub fn test_increments() {
        let mut counters = 0 as u64;
// 16 elements
        let cap = 16;
        let cap_mask = 0xf;


        for i in 0..8 {
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 0, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));

            counters = super::increment_write_in_process(counters);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 1, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));

            counters = super::increment_write_in_process(counters);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 2, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));
            counters = super::increment_write_in_process(counters);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 3, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));
            counters = super::increment_write_done(counters, cap_mask);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 3, 1), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));
            counters = super::increment_write_done(counters, cap_mask);
            assert_eq!((0, (0 + i * 3) % 16, 0, 0, (0 + i * 3) % 16, 3, 2), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));
            counters = super::increment_write_done(counters, cap_mask);
            assert_eq!((3, (0 + i * 3) % 16, 0, 0, (3 + i * 3) % 16, 0, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));

            counters = super::increment_read_in_process(counters);
            assert_eq!((2, (0 + i * 3) % 16, 1, 0, (3 + i * 3) % 16, 0, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));
            counters = super::increment_read_in_process(counters);
            assert_eq!((1, (0 + i * 3) % 16, 2, 0, (3 + i * 3) % 16, 0, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));
            counters = super::increment_read_in_process(counters);
            assert_eq!((0, (0 + i * 3) % 16, 3, 0, (3 + i * 3) % 16, 0, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));
            counters = super::increment_read_done(counters, cap_mask);
            assert_eq!((0, (0 + i * 3) % 16, 3, 1, (3 + i * 3) % 16, 0, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));
            counters = super::increment_read_done(counters, cap_mask);
            assert_eq!((0, (0 + i * 3) % 16, 3, 2, (3 + i * 3) % 16, 0, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));
            counters = super::increment_read_done(counters, cap_mask);
            assert_eq!((0, (3 + i * 3) % 16, 0, 0, (3 + i * 3) % 16, 0, 0), (super::size(counters, cap), super::read_index(counters), super::read_in_process_count(counters), super::read_done_count(counters), super::write_index(counters), super::write_in_process_count(counters), super::write_done_count(counters)));
        }
    }

    #[test]
    pub fn test_pool() {
        let ring = super::AtomicRingBuffer::new(900);

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
    pub fn test_pool_large() {
        let ring = super::AtomicRingBuffer::new(65535);


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
    pub fn test_pool_large2() {
        let ring = super::AtomicRingBuffer::new(65536);


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
    pub fn test_threaded() {
        let cap = 65535;

        let buf: super::AtomicRingBuffer<usize> = super::AtomicRingBuffer::new(cap);


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
                    let a = buf.try_pop().expect("Pop a");
                    let b = buf.try_pop().expect("Pop b");
                    buf.try_push(a).expect("push a");
                    buf.try_push(b).expect("push b");
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
        actual.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(actual, expected);
    }
}

