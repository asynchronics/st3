# St³ — the Stealing Static Stack

Lock-free, bounded, work-stealing queues with FIFO stealing amd LIFO or FIFO
semantic for the worker thread.

[![Cargo](https://img.shields.io/crates/v/multishot.svg)](https://crates.io/crates/st3)
[![Documentation](https://docs.rs/multishot/badge.svg)](https://docs.rs/st3)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/asynchronics/st3#license)


## Overview

The Go scheduler and the [Tokio] runtime are examples of high-performance
schedulers that rely on fixed-capacity (*bounded*) work-stealing queues to avoid
the allocation and synchronization overhead associated with unbounded queues
such as the Chase-Lev work-stealing deque (used in particular by [Crossbeam
Deque]). This is a natural design choice for schedulers that use a global
injector queue as the latter often can, at nearly no extra cost, buffer the
overflow should a local queue become full.

For such applications, `St3` provides high-performance, fixed-size, lock-free
FIFO and LIFO work-stealing queues.

The FIFO queue is mostly based on the Tokio queue, but provides a somewhat more
convenient and more flexible API. The LIFO queue is a novel design with the same
API and performance profile as its FIFO counterpart and can be considered a
faster, fixed-size alternative to the Chase-Lev deque.

In theory, the LIFO variant can be expected to perform better in pure
data-parallelism applications due to lower cache misses and lower contention
between workers and stealers (items being popped and stolen from opposite ends).
The FIFO variant may prove in turn preferable for applications such as
message-passing executors where processing the most recently queued items is not
necessarily the optimal strategy.

[Tokio]: https://github.com/tokio-rs/tokio
[Crossbeam Deque]: https://github.com/crossbeam-rs/crossbeam/tree/master/crossbeam-deque


## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
st3 = "0.3.1"
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

The queues are low-level primitives and as such their implementation relies on
`unsafe`. The test suite makes extensive use of [Loom] to assess correctness. As
amazing as it is, however, Loom is only a tool: it cannot formally prove the
absence of data races.

Before *St³* sees wider use in the field and receives greater scrutiny, you
should exercise caution before using it in mission-critical software. The LIFO
queue in particular is a new concurrent algorithm and it is therefore possible
that soundness issues will be discovered that weren't caught by the test suite.

[Loom]: https://github.com/tokio-rs/loom


## Performance

The *St³* queues use no atomic fences and very few atomic Read-Modify-Write
(RMW) operations. Similarly to the Tokio queue, they needs no RMW for `push` and
only one for `pop`. Stealing operations require only a single RMW in the LIFO
variant and 2 in the FIFO variant.

The first benchmark measures performance in the single-threaded, no-stealing
case: a series of 64 `push` operations (or 256 in the large-batch case) is
followed by as many pop operations.

Test CPU: i5-7200U.

| benchmark            | queue                            | average time |
|----------------------|----------------------------------|:------------:|
| push_pop-small_batch | St³ FIFO                         |    836 ns    |
| push_pop-small_batch | St³ LIFO                         |    797 ns    |
| push_pop-small_batch | Tokio (FIFO)                     |    835 ns    |
| push_pop-small_batch | Crossbeam Deque (Chase-Lev) FIFO |    837 ns    |
| push_pop-small_batch | Crossbeam Deque (Chase-Lev) LIFO |   1357 ns    |

| benchmark            | queue                            | average time |
|----------------------|----------------------------------|:------------:|
| push_pop-large_batch | St³ FIFO                         |   3368 ns    |
| push_pop-large_batch | St³ LIFO                         |   3310 ns    |
| push_pop-large_batch | Tokio (FIFO)                     |   3370 ns    |
| push_pop-large_batch | Crossbeam Deque (Chase-Lev) FIFO |   5249 ns    |
| push_pop-large_batch | Crossbeam Deque (Chase-Lev) LIFO |   7326 ns    |

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
figures below must be taken with a grain of salt**. In particular, this
benchmark does not model message-passing, which in practice was observed to
often favor FIFO queues.

Test CPU: i5-7200U.

| benchmark | queue                            | average time |
|-----------|----------------------------------|:------------:|
| executor  | St³ FIFO                         |    210 µs    |
| executor  | St³ LIFO                         |    221 µs    |
| executor  | Tokio (FIFO)                     |    248 µs    |
| executor  | Crossbeam Deque (Chase-Lev) FIFO |    318 µs    |
| executor  | Crossbeam Deque (Chase-Lev) LIFO |    304 µs    |


## (Mis)Features

Just like the Tokio queue, the *St³* queues are susceptible to [ABA]. For
instance, in a naive implementation, if a steal operation was preempted at the
wrong moment for exactly the time necessary to pop a number of items equal to
the queue capacity while pushing less items than are popped, once resumed the
stealer could attempt to steal more items than are available. ABA is overcome by
using buffer positions that can index many times the actual buffer capacity so
as to increase the cycle period beyond worst-case preemption.

For this reason, *St³* will by default use 32-bit buffer positions, which should
in practice provide full resilience against ABA. This requires the use of 64-bit
atomics, however, which on 32-bit targets may not be supported (e.g. MIPS) or
may be slower (e.g. ARMv7). If you like to live dangerously, you can still elect
to disable the default `long_counter` feature and use 16-bit buffer positions
instead:

```toml
[dependencies]
st3 = { version = "0.3.1", default-features = false }
```

Note that disabling this feature has no effect on 64-bit targets: those will
still use 32-bit buffer positions.

[ABA]: https://en.wikipedia.org/wiki/ABA_problem


## Acknowledgements

Although the LIFO implementation ended up quite different, the Tokio FIFO queue
was an inspiration which also helped set the goal in terms of performance.

Tokio's queue is itself a modified version of Go's work-stealing queue. Go uses
something akin to a Seqlock pattern where stealers optimistically read all items
marked for stealing and later discard them if they have been concurrently
evicted from the queue. Because of its stricter aliasing rules, Rust makes this
pattern hard to implement so the Tokio queue was designed with the ability to
"book" the items beforehand, an idea which *St³*'s LIFO queue borrowed.


## License

This software is licensed under the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option.

Some assets of the test suite and benchmark may be licensed under different
terms, which are explicitly outlined within those assets.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
