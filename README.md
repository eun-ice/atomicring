
[![Build Status](https://travis-ci.org/eun-ice/atomicring.svg?branch=master)](https://travis-ci.org/obourgain/rust-atomic64)

 A constant-size lock-free and almost wait-free ring buffer


 Upsides

 - Very fast, push and pop should be approx. O(1) even during heavy concurrency
 - Only 4 words of memory overhead
 - no memory allocations after initial creation
 
 
 Downsides

 - Fixed size, growing/shrinking is not supported
 - No method for blocking poll
 - Only efficient on 64bit architectures, due to 
 - maximum capacity of 65536 entries
 - capacity is rounded up to the next power of 2

 # Implementation details

 This implementation uses a 64 Bit atomic to store the state

```Text
 +64------+56------+48--------------+32------+24------+16-------------0+
 | w_done | w_pend |  write_index   | r_done | r_pend |   read_index   |
 +--------+--------+----------------+--------+--------+----------------+
```

- write_index/read_index (16bit): current read/write position in the ring buzffer.
- r_pend/w_pend (8bit): number of pending concurrent read/writes
- r_done/w_done (8bit): number of completed read/writes.

 For reading r_pend is incremented first, then the content of the ring buffer is read from memory.
 After reading is done r_done is incremented. read_index is only incremented if r_done is equal to r_pend.

 For writing first w_pend is incremented, then the content of the ring buffer is updated.
 After writing w_done is incremented. If w_done is equal to w_pend then both are set to 0 and write_index is incremented.

 In rare cases this can result in a race where multiple threads increment r_pend and r_done never quite reaches r_pend.
 If r_pend == 255 or w_pend == 255 a spinloop waits it to be <255 to continue.




# Dependencies

This package only depends on [atomic64](https://github.com/obourgain/rust-atomic64)

# Usage

AtomicRingBuffer is on [crates.io](https://crates.io/crates/atomicring) and on [docs.rs](https://docs.rs/atomicring/)

To use AtomicRingBuffer, add this to your `Cargo.toml`:

```toml
[dependencies]
atomicring = "0.1.0"
```


And something like this to your code

 ```rust
 
 // create an AtomicRingBuffer with capacity of 1024 elements 
 let ring = ::atomicring::AtomicRingBuffer::new(900);

// try_pop removes an element of the buffer and returns None if the buffer is empty
 assert_eq!(None, ring.try_pop());
 // push_overwrite adds an element to the buffer, overwriting the oldest element if the buffer is full: 
 ring.push_overwrite(1);
 assert_eq!(Some(1), ring.try_pop());
 assert_eq!(None, ring.try_pop());
 ```
