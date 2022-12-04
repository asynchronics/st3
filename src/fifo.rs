//! # FIFO, bounded, work-stealing queue.
//!
//! ## Example
//!
//! ```
//! use std::thread;
//! use st3::fifo::Worker;
//!
//! // Push 4 items into a FIFO queue of capacity 256.
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
use alloc::boxed::Box;
use alloc::sync::Arc;

use core::iter::FusedIterator;
use core::mem::{drop, MaybeUninit};
use core::panic::{RefUnwindSafe, UnwindSafe};
use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};

use cache_padded::CachePadded;

use crate::config::{AtomicUnsignedLong, AtomicUnsignedShort, UnsignedShort};
use crate::loom_exports::cell::UnsafeCell;
use crate::loom_exports::{debug_or_loom_assert, debug_or_loom_assert_eq};
use crate::{allocate_buffer, pack, unpack, StealError};

/// A double-ended FIFO work-stealing queue.
///
/// The general operation of the queue is based on tokio's worker queue, itself
/// based on the Go scheduler's worker queue.
///
/// The queue tracks its tail and head position within a ring buffer with
/// wrap-around integers, where the least significant bits specify the actual
/// buffer index. All positions have bit widths that are intentionally larger
/// than necessary for buffer indexing because:
/// - an extra bit is needed to disambiguate between empty and full buffers when
///   the start and end position of the buffer are equal,
/// - the worker head is also used as long-cycle counter to mitigate the risk of
///   ABA.
///
#[derive(Debug)]
struct Queue<T> {
    /// Positions of the head as seen by the worker (most significant bits) and
    /// as seen by a stealer (least significant bits).
    heads: CachePadded<AtomicUnsignedLong>,

    /// Position of the tail.
    tail: CachePadded<AtomicUnsignedShort>,

    /// Queue items.
    buffer: Box<[UnsafeCell<MaybeUninit<T>>]>,

    /// Mask for the buffer index.
    mask: UnsignedShort,
}

impl<T> Queue<T> {
    /// Read an item at the given position.
    ///
    /// The position is automatically mapped to a valid buffer index using a
    /// modulo operation.
    ///
    /// # Safety
    ///
    /// The item at the given position must have been initialized before and
    /// cannot have been moved out.
    ///
    /// The caller must guarantee that the item at this position cannot be
    /// written to or moved out concurrently.
    #[inline]
    unsafe fn read_at(&self, position: UnsignedShort) -> T {
        let index = (position & self.mask) as usize;
        (*self.buffer).as_ref()[index].with(|slot| slot.read().assume_init())
    }

    /// Write an item at the given position.
    ///
    /// The position is automatically mapped to a valid buffer index using a
    /// modulo operation.
    ///
    /// # Note
    ///
    /// If an item is already initialized but was not moved out yet, it will be
    /// leaked.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the item at this position cannot be read
    /// or written to concurrently.
    #[inline]
    unsafe fn write_at(&self, position: UnsignedShort, item: T) {
        let index = (position & self.mask) as usize;
        (*self.buffer).as_ref()[index].with_mut(|slot| slot.write(MaybeUninit::new(item)));
    }

    /// Attempt to book `N` items for stealing where `N` is specified by a
    /// closure which takes as argument the total count of available items.
    ///
    /// In case of success, the returned tuple contains the stealer head and an
    /// item count at least equal to 1, in this order.
    ///
    /// # Errors
    ///
    /// An error is returned in the following cases:
    /// 1) no item could be stolen, either because the queue is empty or because
    ///    `N` is 0,
    /// 2) a concurrent stealing operation is ongoing.
    ///
    /// # Safety
    ///
    /// This function is not strictly unsafe, but because it initiates the
    /// stealing operation by modifying the worker head without ever updating
    /// the stealer head, its misuse can result in permanently blocking
    /// subsequent stealing operations.
    fn book_items<C>(
        &self,
        mut count_fn: C,
        max_count: UnsignedShort,
    ) -> Result<(UnsignedShort, UnsignedShort), StealError>
    where
        C: FnMut(usize) -> usize,
    {
        let mut heads = self.heads.load(Acquire);

        loop {
            let (worker_head, stealer_head) = unpack(heads);

            // Bail out if both heads differ because it means another stealing
            // operation is concurrently ongoing.
            if stealer_head != worker_head {
                return Err(StealError::Busy);
            }

            let tail = self.tail.load(Acquire);
            let item_count = tail.wrapping_sub(worker_head);

            // `item_count` is tested now because `count_fn` may expect
            // `item_count>0`.
            if item_count == 0 {
                return Err(StealError::Empty);
            }

            // Unwind safety: it is OK if `count_fn` panics because no state has
            // been modified yet.
            let count = (count_fn(item_count as usize).min(max_count as usize) as UnsignedShort)
                .min(item_count);

            // The special case `count_fn() == 0` must be tested specifically,
            // because if the compare-exchange succeeds with `count=0`, the new
            // worker head will be the same as the old one so other stealers
            // will not detect that stealing is currently ongoing and may try to
            // actually steal items and concurrently modify the position of the
            // heads.
            if count == 0 {
                return Err(StealError::Empty);
            }

            // Move the worker head only.
            let new_heads = pack(worker_head.wrapping_add(count), stealer_head);

            // Attempt to book the slots. Only one stealer can succeed since
            // once this atomic is changed, the other thread will necessarily
            // observe a mismatch between the two heads.
            match self
                .heads
                .compare_exchange_weak(heads, new_heads, Acquire, Acquire)
            {
                Ok(_) => return Ok((stealer_head, count)),
                // We lost the race to a concurrent pop or steal operation, or
                // the CAS failed spuriously; try again.
                Err(h) => heads = h,
            }
        }
    }

    /// Capacity of the queue.
    #[inline]
    fn capacity(&self) -> UnsignedShort {
        self.mask.wrapping_add(1)
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        let worker_head = unpack(self.heads.load(Relaxed)).0;
        let tail = self.tail.load(Relaxed);

        let count = tail.wrapping_sub(worker_head);
        for offset in 0..count {
            drop(unsafe { self.read_at(worker_head.wrapping_add(offset)) })
        }
    }
}

/// Handle for single-threaded FIFO push and pop operations.
#[derive(Debug)]
pub struct Worker<T> {
    queue: Arc<Queue<T>>,
}

impl<T> Worker<T> {
    /// Creates a new queue and returns a `Worker` handle.
    ///
    /// **The capacity of a queue is always a power of two**. It is set to the
    /// smallest power of two greater than or equal to the requested minimum
    /// capacity.
    ///
    /// # Panic
    ///
    /// This method will panic if the minimum requested capacity is greater than
    /// 2³¹ on targets that support 64-bit atomics, or greater than 2¹⁵ on
    /// targets that only support 32-bit atomics.
    pub fn new(min_capacity: usize) -> Self {
        const MAX_CAPACITY: usize = 1 << (UnsignedShort::BITS - 1);

        assert!(
            min_capacity <= MAX_CAPACITY,
            "the capacity of the queue cannot exceed {}",
            MAX_CAPACITY
        );

        // `next_power_of_two` cannot overflow since `min_capacity` cannot be
        // greater than `MAX_CAPACITY`, and the latter is a power of two that
        // always fits within an `UnsignedShort`.
        let capacity = min_capacity.next_power_of_two();
        let buffer = allocate_buffer(capacity);
        let mask = capacity as UnsignedShort - 1;

        let queue = Arc::new(Queue {
            heads: CachePadded::new(AtomicUnsignedLong::new(0)),
            tail: CachePadded::new(AtomicUnsignedShort::new(0)),
            buffer,
            mask,
        });

        Worker { queue }
    }

    /// Checks if the [`Worker`] points to the same underlying queue as a [`Stealer`].
    pub fn is_same_as(&self, stealer: &Stealer<T>) -> bool {
        Arc::ptr_eq(&self.queue, &stealer.queue)
    }

    /// Creates a new `Stealer` handle associated to this `Worker`.
    ///
    /// An arbitrary number of `Stealer` handles can be created, either using
    /// this method or cloning an existing `Stealer` handle.
    pub fn stealer(&self) -> Stealer<T> {
        Stealer {
            queue: self.queue.clone(),
        }
    }

    /// Returns the capacity of the queue.
    pub fn capacity(&self) -> usize {
        self.queue.capacity() as usize
    }

    /// Returns the number of items that can be successfully pushed onto the
    /// queue.
    ///
    /// Note that that the spare capacity may be underestimated due to
    /// concurrent stealing operations.
    pub fn spare_capacity(&self) -> usize {
        let stealer_head = unpack(self.queue.heads.load(Relaxed)).1;
        let tail = self.queue.tail.load(Relaxed);

        // Aggregate count of available items (those which can be popped) and of
        // items currently being stolen.
        let len = tail.wrapping_sub(stealer_head);

        (self.queue.capacity() - len) as usize
    }

    /// Returns true if the queue is empty.
    ///
    /// Note that the queue size is somewhat ill-defined in a multi-threaded
    /// context, but it is warranted that if `is_empty()` returns true, a
    /// subsequent call to `pop()` will fail.
    pub fn is_empty(&self) -> bool {
        let worker_head = unpack(self.queue.heads.load(Relaxed)).0;
        let tail = self.queue.tail.load(Relaxed);

        tail == worker_head
    }

    /// Attempts to push one item at the tail of the queue.
    ///
    /// # Errors
    ///
    /// This will fail if the queue is full, in which case the item is returned
    /// as the error field.
    pub fn push(&self, item: T) -> Result<(), T> {
        let stealer_head = unpack(self.queue.heads.load(Acquire)).1;
        let tail = self.queue.tail.load(Relaxed);

        // Check that the buffer is not full.
        if tail.wrapping_sub(stealer_head) > self.queue.mask {
            return Err(item);
        }

        // Store the item.
        unsafe { self.queue.write_at(tail, item) };

        // Make the item visible by moving the tail.
        //
        // Ordering: the Release ordering ensures that the subsequent
        // acquisition of this atomic by a stealer will make the previous write
        // visible.
        self.queue.tail.store(tail.wrapping_add(1), Release);

        Ok(())
    }

    /// Attempts to push the content of an iterator at the tail of the queue.
    ///
    /// It is the responsibility of the caller to ensure that there is enough
    /// spare capacity to accommodate all iterator items, for instance by
    /// calling `[Worker::spare_capacity]` beforehand. Otherwise, the iterator
    /// is dropped while still holding the items in excess.
    pub fn extend<I: IntoIterator<Item = T>>(&self, iter: I) {
        let stealer_head = unpack(self.queue.heads.load(Acquire)).1;
        let mut tail = self.queue.tail.load(Relaxed);

        let max_tail = stealer_head.wrapping_add(self.queue.capacity());
        for item in iter {
            // Check whether the buffer is full.
            if tail == max_tail {
                break;
            }
            // Store the item.
            unsafe { self.queue.write_at(tail, item) };
            tail = tail.wrapping_add(1);
        }

        // Make the items visible by incrementing the push count.
        //
        // Ordering: the Release ordering ensures that the subsequent
        // acquisition of this atomic by a stealer will make the previous write
        // visible.
        self.queue.tail.store(tail, Release);
    }

    /// Attempts to pop one item from the head of the queue.
    ///
    /// This returns None if the queue is empty.
    pub fn pop(&self) -> Option<T> {
        let mut heads = self.queue.heads.load(Acquire);

        let prev_worker_head = loop {
            let (worker_head, stealer_head) = unpack(heads);
            let tail = self.queue.tail.load(Relaxed);

            // Check if the queue is empty.
            if tail == worker_head {
                return None;
            }

            // Move the worker head. The weird cast from `bool` to
            // `UnsignedShort` is to steer the compiler towards branchless code.
            let next_heads = pack(
                worker_head.wrapping_add(1),
                stealer_head.wrapping_add((stealer_head == worker_head) as UnsignedShort),
            );

            // Attempt to book the items.
            let res = self
                .queue
                .heads
                .compare_exchange_weak(heads, next_heads, AcqRel, Acquire);

            match res {
                Ok(_) => break worker_head,
                // We lost the race to a stealer or the CAS failed spuriously; try again.
                Err(h) => heads = h,
            }
        };

        unsafe { Some(self.queue.read_at(prev_worker_head)) }
    }

    /// Returns an iterator that steals items from the head of the queue.
    ///
    /// The returned iterator steals up to `N` items, where `N` is specified by
    /// a closure which takes as argument the total count of items available for
    /// stealing. Upon success, the number of items ultimately stolen can be
    /// from 1 to `N`, depending on the number of available items.
    ///
    /// # Beware
    ///
    /// All items stolen by the iterator should be moved out as soon as
    /// possible, because until then or until the iterator is dropped, all
    /// concurrent stealing operations will fail with [`StealError::Busy`].
    ///
    /// # Leaking
    ///
    /// If the iterator is leaked before all stolen items have been moved out,
    /// subsequent stealing operations will permanently fail with
    /// [`StealError::Busy`].
    ///
    /// # Errors
    ///
    /// An error is returned in the following cases:
    /// 1) no item was stolen, either because the queue is empty or `N` is 0,
    /// 2) a concurrent stealing operation is ongoing.
    pub fn drain<C>(&self, count_fn: C) -> Result<Drain<'_, T>, StealError>
    where
        C: FnMut(usize) -> usize,
    {
        let (head, count) = self.queue.book_items(count_fn, UnsignedShort::MAX)?;

        Ok(Drain {
            queue: &self.queue,
            head,
            from_head: head,
            to_head: head.wrapping_add(count),
        })
    }
}

impl<T> UnwindSafe for Worker<T> {}
impl<T> RefUnwindSafe for Worker<T> {}
unsafe impl<T: Send> Send for Worker<T> {}

/// A draining iterator for [`Worker<T>`].
///
/// This iterator is created by [`Worker::drain`]. See its documentation for
/// more.
#[derive(Debug)]
pub struct Drain<'a, T> {
    queue: &'a Queue<T>,
    head: UnsignedShort,
    from_head: UnsignedShort,
    to_head: UnsignedShort,
}

impl<'a, T> Iterator for Drain<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.head == self.to_head {
            return None;
        }

        let item = Some(unsafe { self.queue.read_at(self.head) });

        self.head = self.head.wrapping_add(1);

        // We cannot rely on the caller to call `next` again after the last item
        // is yielded so the heads must be updated immediately when yielding the
        // last item.
        if self.head == self.to_head {
            // Signal that the stealing operation has completed.
            let mut heads = self.queue.heads.load(Relaxed);
            loop {
                let (worker_head, stealer_head) = unpack(heads);

                debug_or_loom_assert_eq!(stealer_head, self.from_head);

                let res = self.queue.heads.compare_exchange_weak(
                    heads,
                    pack(worker_head, worker_head),
                    AcqRel,
                    Acquire,
                );

                match res {
                    Ok(_) => break,
                    Err(h) => {
                        heads = h;
                    }
                }
            }
        }

        item
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let sz = self.to_head.wrapping_sub(self.head) as usize;

        (sz, Some(sz))
    }
}

impl<'a, T> ExactSizeIterator for Drain<'a, T> {}

impl<'a, T> FusedIterator for Drain<'a, T> {}

impl<'a, T> Drop for Drain<'a, T> {
    fn drop(&mut self) {
        // Drop all items and make sure the head is updated so that subsequent
        // stealing operations can succeed.
        for _item in self {}
    }
}

impl<'a, T> UnwindSafe for Drain<'a, T> {}
impl<'a, T> RefUnwindSafe for Drain<'a, T> {}
unsafe impl<'a, T: Send> Send for Drain<'a, T> {}
unsafe impl<'a, T: Send> Sync for Drain<'a, T> {}

/// Handle for multi-threaded stealing operations.
#[derive(Debug)]
pub struct Stealer<T> {
    queue: Arc<Queue<T>>,
}

impl<T> Stealer<T> {
    /// Attempts to steal items from the head of the queue and move them to the
    /// tail of another queue.
    ///
    /// Up to `N` items are moved to the destination queue, where `N` is
    /// specified by a closure which takes as argument the total count of items
    /// available for stealing. Upon success, the number of items ultimately
    /// transferred to the destination queue can be from 1 to `N`, depending on
    /// the number of available items and the capacity of the destination queue;
    /// the count of transferred items is returned as the success payload.
    ///
    /// # Errors
    ///
    /// An error is returned in the following cases:
    /// 1) no item was stolen, either because the queue is empty, the
    ///    destination is full or `N` is 0,
    /// 2) a concurrent stealing operation is ongoing.
    pub fn steal<C>(&self, dest: &Worker<T>, count_fn: C) -> Result<usize, StealError>
    where
        C: FnMut(usize) -> usize,
    {
        // Compute the free capacity of the destination queue.
        //
        // Ordering: see `Worker::push()` method.
        let dest_tail = dest.queue.tail.load(Relaxed);
        let dest_stealer_head = unpack(dest.queue.heads.load(Acquire)).1;
        let dest_free_capacity = dest.queue.capacity() - dest_tail.wrapping_sub(dest_stealer_head);

        debug_or_loom_assert!(dest_free_capacity <= dest.queue.capacity());

        let (stealer_head, transfer_count) = self.queue.book_items(count_fn, dest_free_capacity)?;

        debug_or_loom_assert!(transfer_count <= dest_free_capacity);

        // Move all items but the last to the destination queue.
        for offset in 0..transfer_count {
            unsafe {
                let item = self.queue.read_at(stealer_head.wrapping_add(offset));
                dest.queue.write_at(dest_tail.wrapping_add(offset), item);
            }
        }

        // Make the moved items visible by updating the destination tail position.
        //
        // Ordering: see comments in the `push()` method.
        dest.queue
            .tail
            .store(dest_tail.wrapping_add(transfer_count), Release);

        // Signal that the stealing operation has completed.
        let mut heads = self.queue.heads.load(Relaxed);
        loop {
            let (worker_head, sh) = unpack(heads);

            debug_or_loom_assert_eq!(stealer_head, sh);

            let res = self.queue.heads.compare_exchange_weak(
                heads,
                pack(worker_head, worker_head),
                AcqRel,
                Acquire,
            );

            match res {
                Ok(_) => return Ok(transfer_count as usize),
                Err(h) => {
                    heads = h;
                }
            }
        }
    }

    /// Attempts to steal items from the head of the queue, returning one of
    /// them directly and moving the others to the tail of another queue.
    ///
    /// Up to `N` items are stolen (including the one returned directly), where
    /// `N` is specified by a closure which takes as argument the total count of
    /// items available for stealing. Upon success, one item is returned and
    /// from 0 to `N-1` items are moved to the destination queue, depending on
    /// the number of available items and the capacity of the destination queue;
    /// the number of transferred items is returned as the second field of the
    /// success value.
    ///
    /// The returned item is the most recent one among the stolen items.
    ///
    /// # Errors
    ///
    /// An error is returned in the following cases:
    /// 1) no item was stolen, either because the queue is empty or `N` is 0,
    /// 2) a concurrent stealing operation is ongoing.
    ///
    /// Failure to transfer any item to the destination queue is not considered
    /// an error as long as one element could be returned directly. This can
    /// occur if the destination queue is full, if the source queue has only one
    /// item or if `N` is 1.
    pub fn steal_and_pop<C>(&self, dest: &Worker<T>, count_fn: C) -> Result<(T, usize), StealError>
    where
        C: FnMut(usize) -> usize,
    {
        // Compute the free capacity of the destination queue.
        //
        // Ordering: see `Worker::push()` method.
        let dest_tail = dest.queue.tail.load(Relaxed);
        let dest_stealer_head = unpack(dest.queue.heads.load(Acquire)).1;
        let dest_free_capacity = dest.queue.capacity() - dest_tail.wrapping_sub(dest_stealer_head);

        debug_or_loom_assert!(dest_free_capacity <= dest.queue.capacity());

        let (stealer_head, count) = self.queue.book_items(count_fn, dest_free_capacity + 1)?;
        let transfer_count = count - 1;

        debug_or_loom_assert!(transfer_count <= dest_free_capacity);

        // Move all items but the last to the destination queue.
        for offset in 0..transfer_count {
            unsafe {
                let item = self.queue.read_at(stealer_head.wrapping_add(offset));
                dest.queue.write_at(dest_tail.wrapping_add(offset), item);
            }
        }

        // Read the last item.
        let last_item = unsafe {
            self.queue
                .read_at(stealer_head.wrapping_add(transfer_count))
        };

        // Make the moved items visible by updating the destination tail position.
        //
        // Ordering: see comments in the `push()` method.
        dest.queue
            .tail
            .store(dest_tail.wrapping_add(transfer_count), Release);

        // Signal that the stealing operation has completed.
        let mut heads = self.queue.heads.load(Relaxed);
        loop {
            let (worker_head, sh) = unpack(heads);

            debug_or_loom_assert_eq!(stealer_head, sh);

            let res = self.queue.heads.compare_exchange_weak(
                heads,
                pack(worker_head, worker_head),
                AcqRel,
                Acquire,
            );

            match res {
                Ok(_) => return Ok((last_item, transfer_count as usize)),
                Err(h) => {
                    heads = h;
                }
            }
        }
    }
}

impl<T> Clone for Stealer<T> {
    fn clone(&self) -> Self {
        Stealer {
            queue: self.queue.clone(),
        }
    }
}

impl<T> UnwindSafe for Stealer<T> {}
impl<T> RefUnwindSafe for Stealer<T> {}
unsafe impl<T: Send> Send for Stealer<T> {}
unsafe impl<T: Send> Sync for Stealer<T> {}
