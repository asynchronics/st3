//! # St³ — Stealing Static Stack
//!
//! Very fast lock-free, bounded, work-stealing queue with FIFO stealing and
//! LIFO of FIFO semantic for the worker thread.
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
use std::iter::FusedIterator;

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

/// Handle for single-threaded FIFO push and pop operations.
#[derive(Debug)]
pub enum Worker<T, B: Buffer<T>> {
    Fifo(fifo::Worker<T, B>),
    Lifo(lifo::Worker<T, B>),
}

impl<T, B: Buffer<T>> Worker<T, B> {
    pub fn new_fifo() -> Self {
        Self::Fifo(fifo::Worker::new())
    }

    pub fn new_lifo() -> Self {
        Self::Lifo(lifo::Worker::new())
    }

    pub fn stealer(&self) -> Stealer<T, B> {
        match self {
            Self::Fifo(worker) => Stealer::Fifo(worker.stealer()),
            Self::Lifo(worker) => Stealer::Lifo(worker.stealer()),
        }
    }

    pub fn spare_capacity(&self) -> usize {
        match self {
            Self::Fifo(worker) => worker.spare_capacity(),
            Self::Lifo(worker) => worker.spare_capacity(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Fifo(worker) => worker.is_empty(),
            Self::Lifo(worker) => worker.is_empty(),
        }
    }

    pub fn push(&self, item: T) -> Result<(), T> {
        match self {
            Self::Fifo(worker) => worker.push(item),
            Self::Lifo(worker) => worker.push(item),
        }
    }

    pub fn extend<I: IntoIterator<Item = T>>(&self, iter: I) {
        match self {
            Self::Fifo(worker) => worker.extend(iter),
            Self::Lifo(worker) => worker.extend(iter),
        }
    }

    pub fn pop(&self) -> Option<T> {
        match self {
            Self::Fifo(worker) => worker.pop(),
            Self::Lifo(worker) => worker.pop(),
        }
    }

    pub fn drain<C>(&self, count_fn: C) -> Result<Drain<'_, T, B>, StealError>
    where
        C: FnMut(usize) -> usize,
    {
        match self {
            Self::Fifo(worker) => worker.drain(count_fn).map(|drain| Drain::Fifo(drain)),
            Self::Lifo(worker) => worker.drain(count_fn).map(|drain| Drain::Lifo(drain)),
        }
    }
}

/// A draining iterator for [`Worker<T, B>`].
///
/// This iterator is created by [`Worker::drain`]. See its documentation for
/// more.
#[derive(Debug)]
pub enum Drain<'a, T, B: Buffer<T>> {
    Fifo(fifo::Drain<'a, T, B>),
    Lifo(lifo::Drain<'a, T, B>),
}

impl<'a, T, B: Buffer<T>> Iterator for Drain<'a, T, B> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        match self {
            Self::Fifo(drain) => drain.next(),
            Self::Lifo(drain) => drain.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Fifo(drain) => drain.size_hint(),
            Self::Lifo(drain) => drain.size_hint(),
        }
    }
}

impl<'a, T, B: Buffer<T>> ExactSizeIterator for Drain<'a, T, B> {}
impl<'a, T, B: Buffer<T>> FusedIterator for Drain<'a, T, B> {}

/// Handle for multi-threaded stealing operations.
#[derive(Debug)]
pub enum Stealer<T, B: Buffer<T>> {
    Fifo(fifo::Stealer<T, B>),
    Lifo(lifo::Stealer<T, B>),
}

impl<T, B: Buffer<T>> Stealer<T, B> {
    pub fn steal<C, BDest>(&self, dest: &Worker<T, BDest>, count_fn: C) -> Result<usize, StealError>
    where
        C: FnMut(usize) -> usize,
        BDest: Buffer<T>,
    {
        match (self, dest) {
            (Self::Fifo(stealer), Worker::Fifo(worker)) => stealer.steal(worker, count_fn),
            (Self::Lifo(stealer), Worker::Lifo(worker)) => stealer.steal(worker, count_fn),
            _ => panic!("The type of the stealer must match that of the worker"),
        }
    }

    pub fn steal_and_pop<C, BDest>(
        &self,
        dest: &Worker<T, BDest>,
        count_fn: C,
    ) -> Result<(T, usize), StealError>
    where
        C: FnMut(usize) -> usize,
        BDest: Buffer<T>,
    {
        match (self, dest) {
            (Self::Fifo(stealer), Worker::Fifo(worker)) => stealer.steal_and_pop(worker, count_fn),
            (Self::Lifo(stealer), Worker::Lifo(worker)) => stealer.steal_and_pop(worker, count_fn),
            _ => panic!("The type of the stealer must match that of the worker"),
        }
    }
}

impl<T, B: Buffer<T>> Clone for Stealer<T, B> {
    fn clone(&self) -> Self {
        match self {
            Self::Fifo(stealer) => Self::Fifo(stealer.clone()),
            Self::Lifo(stealer) => Self::Lifo(stealer.clone()),
        }
    }
}
