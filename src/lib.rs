//! # St³ — Stealing Static Stack
//!
//! Very fast lock-free, bounded, work-stealing queue with FIFO stealing and
//! LIFO or FIFO semantic for the worker thread.
//!
//! The `Worker` handle enables push and pop operations from a single thread,
//! while `Stealer` handles can be shared between threads to perform FIFO
//! batch-stealing operations.
//!
//! `St³` is effectively a faster, fixed-size alternative to the Chase-Lev
//! double-ended queue. It uses no atomic fences, much fewer atomic loads and
//! stores, and fewer Read-Modify-Write operations: none for `push`, one for
//! `pop` and one (LIFO) or two (FIFO) for `steal`.
//!
//! ## Example
//!
//! ```
//! use std::thread;
//! use st3::lifo::Worker;
//!
//! // Push 4 items into a queue of capacity 256.
//! let worker = Worker::new(256);
//! worker.push("a").unwrap();
//! worker.push("b").unwrap();
//! worker.push("c").unwrap();
//! worker.push("d").unwrap();
//!
//! // Steal items concurrently.
//! let stealer = worker.stealer();
//! let th = thread::spawn(move || {
//!     let other_worker = Worker::new(256);
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
#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

use core::fmt;
use core::mem::MaybeUninit;

use config::{UnsignedLong, UnsignedShort};

use crate::loom_exports::cell::UnsafeCell;

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

fn allocate_buffer<T>(len: usize) -> Box<[UnsafeCell<MaybeUninit<T>>]> {
    let mut buffer = Vec::with_capacity(len);

    // Note: resizing the vector would normally be an O(N) operation due to
    // initialization, but initialization is optimized out in release mode since
    // an `UnsafeCell<MaybeUninit>` does not actually need to be initialized as
    // `UnsafeCell` is `repr(transparent)`.
    buffer.resize_with(len, || UnsafeCell::new(MaybeUninit::uninit()));

    buffer.into_boxed_slice()
}
