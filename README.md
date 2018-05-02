 A constant-size lock-free and almost wait-free ring buffer


 Upsides:

 - Writes and reads should be approx. O(1) even during heavy concurrency
 - No allocs

 Downsides:

 - Fixed size, growing/shrinking is not supported
 - No blocking wait/poll
 - only efficient on 64bit architectures
 - maximum capacity of 65536 entries
 - capacity is rounded up to the next power of 2

 Implementation details:

 This implementation uses a 64 Bit atomic to store the state

```
 +--------+--------+----------------+--------+--------+----------------+
 | w_done | w_pend |  write_index   | r_done | r_pend |   read_index   |
 +--------+--------+----------------+--------+--------+----------------+
```

- write_index/read_index are 16bit indizes on the current read/write position of the ring buffer.
- r_pend/w_pend is the number of pending concurrent read/writes
- r_done/w_done is the number of completed read/writes.

 For reading r_pend is incremented first, then the content of the ring buffer is read from memory.
 After reading is done r_done is incremented. read_index is only incremented if r_done is equal to r_pend.

 For writing first w_pend is incremented, then the content of the ring buffer is updated.
 After writing w_done is incremented. If w_done is equal to w_pend then both are set to 0 and write_index is incremented.

 In rare cases this can result in a race where multiple threads increment r_pend and r_done never quite reaches r_pend.
 If r_pend == 255 or w_pend == 255 a spinloop waits it to be <255 to continue.


 # Examples

 ```
 let ring = ::atomicring::AtomicRingBuffer::new(900);

 assert_eq!(None, ring.try_pop());
 ring.push_overwrite(1);
 assert_eq!(Some(1), ring.try_pop());
 assert_eq!(None, ring.try_pop());
 ```


# Usage

To use AtomicRingBuffer, add this to your `Cargo.toml`:

```toml
[dependencies]
atomicring = "0.1.0"
```