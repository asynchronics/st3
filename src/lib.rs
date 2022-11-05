//! # St³ — Stealing Static Stack
//!
//! A very fast lock-free, bounded, work-stealing queue with stack-like (LIFO)
//! semantic for the worker thread and FIFO stealing.
//!
//! The [`Worker`] handle enables LIFO push and pop operations from a single
//! thread, while [`Stealer`] handles can be shared between threads to perform
//! FIFO batch-stealing operations.
//!
//! `St³` is effectively a faster, fixed-size alternative to the Chase-Lev
//! double-ended queue. It uses no atomic fences, much fewer atomic loads and
//! stores, and fewer Read-Modify-Write operations: none for
//! [`push`](Worker::push) and only one for [`pop`](Worker::pop) and
//! [`steal`](Stealer::steal).
//!
//! ## Example
//!
//! ```
//! use std::thread;
//! use st3::B256;
//! use st3::lifo::Worker;
//!
//! // Push 4 items into a queue of capacity 256.
//! let worker = Worker::<_, B256>::new();
//! worker.push("a").unwrap();
//! worker.push("b").unwrap();
//! worker.push("c").unwrap();
//! worker.push("d").unwrap();
//!
//! // Steal items concurrently.
//! let stealer = worker.stealer();
//! let th = thread::spawn(move || {
//!     let other_worker = Worker::<_, B256>::new();
//!
//!     // Try to steal half the items and return the actual count of stolen items.
//!     match stealer.steal(&other_worker, |n| n/2) {
//!         Ok(actual) => actual,
//!         Err(_) => 0,
//!     }
//! });
//!
//! // Pop items concurrently.
//! let mut pop_count = 0;
//! while worker.pop().is_some() {
//!     pop_count += 1;
//! }
//!
//! // Does it add up?
//! let steal_count = th.join().unwrap();
//! assert_eq!(pop_count + steal_count, 4);
//! ```

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]

use std::fmt;

use config::{UnsignedLong, UnsignedShort};

pub mod buffers;
pub use buffers::*;
mod config;
pub mod fifo;
pub mod lifo;
mod loom_exports;

/// Error returned when stealing is unsuccessful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StealError {
    /// No item was stolen.
    Empty,
    /// Another concurrent stealing operation is ongoing.
    Busy,
}

impl fmt::Display for StealError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            StealError::Empty => write!(f, "cannot steal from empty queue"),
            StealError::Busy => write!(f, "a concurrent steal operation is ongoing"),
        }
    }
}

#[inline]
/// Pack two short integers into a long one.
fn pack(value1: UnsignedShort, value2: UnsignedShort) -> UnsignedLong {
    ((value1 as UnsignedLong) << UnsignedShort::BITS) | value2 as UnsignedLong
}
#[inline]
/// Unpack a long integer into 2 short ones.
fn unpack(value: UnsignedLong) -> (UnsignedShort, UnsignedShort) {
    (
        (value >> UnsignedShort::BITS) as UnsignedShort,
        value as UnsignedShort,
    )
}
