# St³ — the Stealing Static Stack

A very fast lock-free, bounded, work-stealing queue with stack-like (LIFO)
semantic for the worker thread and FIFO stealing.

[![Cargo](https://img.shields.io/crates/v/multishot.svg)](https://crates.io/crates/st3)
[![Documentation](https://docs.rs/multishot/badge.svg)](https://docs.rs/st3)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/asynchronics/st3#license)


## Overview

The Go scheduler and the [Tokio] runtime are examples of high-performance
schedulers that rely on fixed-capacity (*bounded*) work-stealing queues to avoid
the allocation and synchronization overhead associated with unbounded queues
such as the Chase-Lev work-stealing deque (used in particular by
[Crossbeam Deque]). This is a natural design choice for schedulers that use a
global injector queue as the latter often can, at nearly no extra cost, buffer
the overflow should a local queue become full.

The Go and Tokio schedulers use FIFO local queues to ensure fairness. For
schedulers that do not require fairness, however, LIFO queues can provide better
performance since dequeued tasks are more likely to have their associated data
still available in the CPU cache[^1]. Moreover, contention between workers and
stealers is reduced since items are popped and stolen from opposite ends.

For such use-cases, *St³* constitutes a faster, fixed-size alternative to the
Chase-Lev deque with even slightly better performance than the Tokio
work-stealing queue.

[^1]: Go and Tokio actually both use a single-task LIFO slot that bypasses the
    FIFO queue for this very reason.

[Tokio]: https://github.com/tokio-rs/tokio
[Crossbeam Deque]: https://github.com/crossbeam-rs/crossbeam/tree/master/crossbeam-deque


## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
st3 = "0.1.1"
```


## Example

```rust
use std::thread;
use st3::{Worker, B256};

// Push 4 items into a queue of capacity 256.
let worker = Worker::<_, B256>::new();
worker.push("a").unwrap();
worker.push("b").unwrap();
worker.push("c").unwrap();
worker.push("d").unwrap();

// Steal items concurrently.
let stealer = worker.stealer();
let th = thread::spawn(move || {
    let other_worker = Worker::<_, B256>::new();

    // Try to steal half the items and return the actual count of stolen items.
    match stealer.steal(&other_worker, |n| n/2) {
        Ok(actual) => actual,
        Err(_) => 0,
    }
});

// Pop items concurrently.
let mut pop_count = 0;
while worker.pop().is_some() {
    pop_count += 1;
}

// Does it add up?
let steal_count = th.join().unwrap();
assert_eq!(pop_count + steal_count, 4);
```


## Safety — a word of caution

This is a low-level primitive and as such its implementation relies on `unsafe`.
The test suite makes extensive use of [Loom] to assess its correctness. As
amazing as it is, however, Loom is only a tool: it cannot formally prove the
absence of data races.

Before *St³* sees wider use in the field and receives greater scrutiny, you
should exercise caution before using it in mission-critical software. It is a
new concurrent algorithm and it is therefore possible that soundness issues will
be discovered that weren't caught by the test suite.

[Loom]: https://github.com/tokio-rs/loom


## Performance

*St³* uses no atomic fences and very few atomic Read-Modify-Write (RMW)
operations. Similarly to the Tokio queue, it needs no RMW for `push` and only
one for `pop`. Stealing operations require only a single RMW (one less than the
Tokio queue).

The first benchmark measures performance in the single-threaded, no-stealing
case: a series of 64 `push` operations (or 256 in the large-batch case) is
followed by as many pop operations.

Test CPU: i5-7200U.

| benchmark            | queue                            | average time |
|----------------------|----------------------------------|:------------:|
| push_pop-small_batch | St³ (LIFO)                       |    784 ns    |
| push_pop-small_batch | Tokio (FIFO)                     |    820 ns    |
| push_pop-small_batch | Crossbeam Deque (Chase-Lev) FIFO |    825 ns    |
| push_pop-small_batch | Crossbeam Deque (Chase-Lev) LIFO |   1314 ns    |

| benchmark            | queue                            | average time |
|----------------------|----------------------------------|:------------:|
| push_pop-large_batch | St³ (LIFO)                       |   3201 ns    |
| push_pop-large_batch | Tokio (FIFO)                     |   3298 ns    |
| push_pop-large_batch | Crossbeam Deque (Chase-Lev) FIFO |   5083 ns    |
| push_pop-large_batch | Crossbeam Deque (Chase-Lev) LIFO |   7188 ns    |

The second benchmark is a synthetic test that aims at characterizing
multi-threaded performance with concurrent stealing. It uses a toy work-stealing
executor which schedules on each of 4 workers an arbitrary number of tasks (from
1 to 256), each task being repeated by re-injection onto its worker an arbitrary
number of times (from 1 to 100). The number of tasks initially assigned to each
workers and the number of times each task is to be repeated are
deterministically pre-determined with a pseudo-RNG, meaning that the workload is
the same for all benchmarked queues. All queues use the Crossbeam Dequeue
work-stealing strategy: half of the tasks are stolen, up to a maximum of 32
tasks. Nevertheless, the re-distribution of tasks via work-stealing is
ultimately non-deterministic as it is affected by thread timing.

Given the somewhat simplistic and subjective design of the benchmark, **the
figures below must be taken with a grain of salt**. That being said, the
relative performance of the different queues appear to be qualitatively
unaffected by the specific values of the benchmark parameters.

Test CPU: i5-7200U.

| benchmark | queue                            | average time |
|-----------|----------------------------------|:------------:|
| executor  | St³ (LIFO)                       |    225 µs    |
| executor  | Tokio (FIFO)                     |    253 µs    |
| executor  | Crossbeam Deque (Chase-Lev) FIFO |    330 µs    |
| executor  | Crossbeam Deque (Chase-Lev) LIFO |    292 µs    |


## (Mis)Features

Just like the Tokio queue, *St³* is susceptible to [ABA]. For instance, in a
naive implementation, if a steal operation was preempted at the wrong moment for
exactly the time necessary to pop a number of items equal to the queue capacity
while pushing less items than are popped, once resumed the stealer could attempt
to steal more items than are available. ABA is overcome by using buffer
positions that can index many times the actual buffer capacity so as to increase
the cycle period beyond worst-case preemption.

Unlike Tokio, *St³* will by default use 32-bit rather than 16-bit buffer
positions. Why? In my opinion, relying on 16 bits to prevent ABA is risky
whereas 32 bits should in practice provides full resilience. This requires the
use of 64-bit atomics, however, which on 32-bit targets may not be supported
(e.g. MIPS) or are slower (e.g. ARMv7). The less paranoid among us can still
disable the default `long_counter` feature to use 32-bit atomics and 16-bit
buffer positions on those targets:

```toml
[dependencies]
st3 = { version = "0.1.1", default-features = false }
```

Note that disabling this feature has no effect on 64-bit targets: those will
still use 32-bit buffer positions to prevent ABA.

[ABA]: https://en.wikipedia.org/wiki/ABA_problem


## Acknowledgements

Although the implementation ended up quite different, the Tokio queue was an
inspiration which also helped set the goal in terms of performance.

Tokio's queue is itself a modified version of Go's work-stealing queue. Go uses
something akin to a Seqlock pattern where stealers optimistically read all items
marked for stealing and later discard them if they have been concurrently
evicted from the queue. Because of its stricter aliasing rules, Rust makes this
pattern hard to implement so the Tokio queue was designed with the ability to
"book" the items beforehand, an idea which *St³* borrowed.


## License

This software is licensed under the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option.

Some assets of the test suite and benchmark may be licensed under different
terms, which are explicitly outlined within those assets.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
