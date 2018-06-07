# AtomicRingBuffer / AtomicRingQueue
 
[![Build Status](https://travis-ci.org/eun-ice/atomicring.svg?branch=master)](https://travis-ci.org/eun-ice/atomicring)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/eun-ice/atomicring)
[![Cargo](https://img.shields.io/crates/v/atomicring.svg)](https://crates.io/crates/atomicring)
[![Documentation](https://docs.rs/atomicring/badge.svg)](https://docs.rs/atomicring)

A constant-size almost lock-free concurrent ring buffer

Upsides

- fast, try_push and try_pop are O(1)
- scales well even during heavy concurrency
- AtomicRingBuffer has only 4 words of memory overhead (AtomicRingQueue has 6 words of overhead)
- blocking pop supported via AtomicRingQueue 
- no memory allocations after initial creation


Downsides

- growing/shrinking is not supported
- maximum capacity of (usize >> 16) entries
- capacity is rounded up to the next power of 2

This queue should perform similar to [mpmc](https://github.com/brayniac/mpmc) but with a lower memory overhead. 
If memory overhead is not your main concern you should run benchmarks to decide which one to use.

## Implementation details

This implementation uses two atomics to store the read_index/write_index

```Text
 Read index atomic
+63------------------------------------------------16+15-----8+7------0+
|                     read_index                     | r_done | r_pend |
+----------------------------------------------------+--------+--------+
 Write index atomic
+63------------------------------------------------16+15-----8+7------0+
|                     write_index                    | w_done | w_pend |
+----------------------------------------------------+--------+--------+
```

- write_index/read_index (16bit on 32bit arch, 48bits on 64bit arch): current read/write position in the ring buffer (head and tail).
- r_pend/w_pend (8bit): number of pending concurrent read/writes
- r_done/w_done (8bit): number of completed read/writes.

For reading r_pend is incremented first, then the content of the ring buffer is read from memory.
After reading is done r_done is incremented. read_index is only incremented if r_done is equal to r_pend.

For writing first w_pend is incremented, then the content of the ring buffer is updated.
After writing w_done is incremented. If w_done is equal to w_pend then both are set to 0 and write_index is incremented.

In rare cases this can result in a race where multiple threads increment r_pend in turn and r_done never quite reaches r_pend.
If r_pend == 255 or w_pend == 255 a spinloop waits it to be <255 to continue. This rarely happens in practice, that's why this is called almost lock-free.


## Structs

This package provides ```AtomicRingBuffer``` without blocking pop support and ```AtomicRingQueue``` with blocking pop support.


## Dependencies

This package depends on [parking_lot](https://github.com/Amanieu/parking_lot) for blocking support in AtomicRingQueue

## Usage

To use AtomicRingBuffer, add this to your `Cargo.toml`:

```toml
[dependencies]
atomicring = "1.0.0"
```


And something like this to your code

```rust

// create an AtomicRingBuffer with capacity of 1024 elements 
let ring = ::atomicring::AtomicRingBuffer::with_capacity(900);

// try_pop removes an element of the buffer and returns None if the buffer is empty
assert_eq!(None, ring.try_pop());
// push_overwrite adds an element to the buffer, overwriting the oldest element if the buffer is full: 
ring.push_overwrite(1);
assert_eq!(Some(1), ring.try_pop());
assert_eq!(None, ring.try_pop());
```


## License

Licensed under the terms of MIT license and the Apache License (Version 2.0).

See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE) for details.

