//! # LIFO, bounded, work-stealing queue.
//!
//! ## Example
//!
//! ```
//! use std::thread;
//! use st3::B256;
//! use st3::lifo::Worker;
//!
//! // Push 4 items into a LIFO queue of capacity 256.
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
use alloc::boxed::Box;
use alloc::sync::Arc;

use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::mem::{drop, MaybeUninit};
use core::panic::{RefUnwindSafe, UnwindSafe};
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};

use cache_padded::CachePadded;

use crate::config::{AtomicUnsignedLong, AtomicUnsignedShort, UnsignedShort};
use crate::loom_exports::cell::UnsafeCell;
use crate::loom_exports::debug_or_loom_assert;
use crate::{pack, unpack, Buffer, StealError};

/// A double-ended LIFO queue.
///
/// The queue tracks its tail and head position within a ring buffer with
/// wrap-around integers, where the least significant bits specify the actual
/// buffer index. All positions have bit widths that are intentionally larger
/// than necessary for buffer indexing because:
/// - an extra bit is needed to disambiguate between empty and full buffers when
///   the start and end position of the buffer are equal,
/// - the pop count and the head in `pop_count_and_head` are also used as
///   long-cycle counters to prevent ABA issues in the pop CAS and in the
///   stealer CAS.
///
/// The position of the head can be at any moment determined by subtracting 2
/// counters: the push operations counter and the pop operations counter.
#[derive(Debug)]
struct Queue<T, B: Buffer<T>> {
    /// Total number of push operations.
    push_count: CachePadded<AtomicUnsignedShort>,

    /// Total number of pop operations, packed together with the position of the
    /// head that a stealer will set once stealing is complete. This head
    /// position always coincides with the `head` field below if the last
    /// stealing operation has completed.
    pop_count_and_head: CachePadded<AtomicUnsignedLong>,

    /// Position of the queue head, updated after completion of each stealing
    /// operation.
    head: CachePadded<AtomicUnsignedShort>,

    /// Queue items.
    buffer: Box<B::Data>,

    /// Make the type !Send and !Sync by default.
    _phantom: PhantomData<UnsafeCell<T>>,
}

impl<T, B: Buffer<T>> Queue<T, B> {
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
        let index = (position & B::MASK) as usize;
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
        let index = (position & B::MASK) as usize;
        (*self.buffer).as_ref()[index].with_mut(|slot| slot.write(MaybeUninit::new(item)));
    }

    /// Attempt to book `N` items for stealing where `N` is specified by a
    /// closure which takes as argument the total count of available items.
    ///
    /// In case of success, the returned triplet is the *current* head, the
    /// *next* head and an item count at least equal to 1.
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
    /// stealing operation by modifying the post-stealing head in
    /// `push_count_and_head` without ever updating the `head` atomic variable,
    /// its misuse can result in permanently blocking subsequent stealing
    /// operations.
    fn book_items<C>(
        &self,
        mut count_fn: C,
        max_count: UnsignedShort,
    ) -> Result<(UnsignedShort, UnsignedShort, UnsignedShort), StealError>
    where
        C: FnMut(usize) -> usize,
    {
        // Ordering: Acquire on the `pop_count_and_head` load synchronizes with
        // the release at the end of a previous pop operation. It is therefore
        // warranted that the push count loaded later is at least the same as it
        // was when the pop count was set, ensuring in turn that the computed
        // tail is not less than the head and therefore the item count does not
        // wrap around. For the same reason, the failure ordering on the CAS is
        // also Acquire since the push count is loaded again at every CAS
        // iteration.
        let mut pop_count_and_head = self.pop_count_and_head.load(Acquire);

        // Ordering: Acquire on the `head` load synchronizes with a release at
        // the end of a previous steal operation. Once this head is confirmed
        // equal to the head in `pop_count_and_head`, it is therefore warranted
        // that the push count loaded later is at least the same as it was on
        // the last completed steal operation, ensuring in turn that the
        // computed tail is not less than the head and therefore the item count
        // does not wrap around. Alternatively, the ordering could be Relaxed if
        // the success ordering on the CAS was AcqRel, which would achieve the
        // same by synchronizing with the head field of `pop_count_and_head`.
        let old_head = self.head.load(Acquire);

        loop {
            let (pop_count, head) = unpack(pop_count_and_head);

            // Bail out if both heads differ because it means another stealing
            // operation is concurrently ongoing.
            if old_head != head {
                return Err(StealError::Busy);
            }

            // Ordering: Acquire synchronizes with the Release in the push
            // method and ensure that all items pushed to the queue are visible.
            let push_count = self.push_count.load(Acquire);
            let tail = push_count.wrapping_sub(pop_count);

            // Note: it is possible for the computed item_count to be spuriously
            // greater than the number of available items if, in this iteration
            // of the CAS loop, `pop_count_and_head` and `head` are both
            // obsolete. This is not an issue, however, since the CAS will then
            // fail due to `pop_count_and_head` being obsolete.
            let item_count = tail.wrapping_sub(head);

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
            // value will be the same as the old one so other stealers will not
            // detect that stealing is currently ongoing and may try to actually
            // steal items and concurrently modify the position of the head.
            if count == 0 {
                return Err(StealError::Empty);
            }

            let new_head = head.wrapping_add(count);
            let new_pop_count_and_head = pack(pop_count, new_head);

            // Attempt to book the slots. Only one stealer can succeed since
            // once this atomic is changed, the other thread will necessarily
            // observe a mismatch between `head` and the head sub-field of
            // `pop_count_and_head`.
            //
            // Ordering: see justification for Acquire on failure in the first
            // load of `pop_count_and_head`. No further synchronization is
            // necessary on success.
            match self.pop_count_and_head.compare_exchange_weak(
                pop_count_and_head,
                new_pop_count_and_head,
                Acquire,
                Acquire,
            ) {
                Ok(_) => return Ok((head, new_head, count)),
                // We lost the race to a concurrent pop or steal operation, or
                // the CAS failed spuriously; try again.
                Err(current) => pop_count_and_head = current,
            }
        }
    }
}

impl<T, B: Buffer<T>> Drop for Queue<T, B> {
    fn drop(&mut self) {
        let head = self.head.load(Relaxed);
        let push_count = self.push_count.load(Relaxed);
        let pop_count = unpack(self.pop_count_and_head.load(Relaxed)).0;
        let tail = push_count.wrapping_sub(pop_count);

        let count = tail.wrapping_sub(head);
        for offset in 0..count {
            drop(unsafe { self.read_at(head.wrapping_add(offset)) });
        }
    }
}

/// Handle for single-threaded LIFO push and pop operations.
#[derive(Debug)]
pub struct Worker<T, B: Buffer<T>> {
    queue: Arc<Queue<T, B>>,
}

impl<T, B: Buffer<T>> Worker<T, B> {
    /// Creates a new queue and returns a `Worker` handle.
    pub fn new() -> Self {
        let queue = Arc::new(Queue {
            push_count: CachePadded::new(AtomicUnsignedShort::new(0)),
            pop_count_and_head: CachePadded::new(AtomicUnsignedLong::new(0)),
            head: CachePadded::new(AtomicUnsignedShort::new(0)),
            buffer: B::allocate(),
            _phantom: PhantomData,
        });

        Worker { queue }
    }

    /// Creates a new `Stealer` handle associated to this `Worker`.
    ///
    /// An arbitrary number of `Stealer` handles can be created, either using
    /// this method or cloning an existing `Stealer` handle.
    pub fn stealer(&self) -> Stealer<T, B> {
        Stealer {
            queue: self.queue.clone(),
        }
    }

    /// Returns the number of items that can be successfully pushed onto the
    /// queue.
    ///
    /// Note that that the spare capacity may be underestimated due to
    /// concurrent stealing operations.
    pub fn spare_capacity(&self) -> usize {
        let capacity = <B as Buffer<T>>::CAPACITY;

        let push_count = self.queue.push_count.load(Relaxed);
        let pop_count = unpack(self.queue.pop_count_and_head.load(Relaxed)).0;
        let tail = push_count.wrapping_sub(pop_count);

        // Ordering: Relaxed ordering is sufficient since no element will be
        // read or written.
        let head = self.queue.head.load(Relaxed);

        // Aggregate count of available items (those which can be popped) and of
        // items currently being stolen. Note that even if the value of `head`
        // is stale, `len` can never exceed the maximum capacity because it is
        // computed on the same thread that pushes items, but `push` would fail
        // if `head` suggested that there is no spare capacity.
        let len = tail.wrapping_sub(head);

        (capacity - len) as usize
    }

    /// Returns true if the queue is empty.
    ///
    /// Note that the queue size is somewhat ill-defined in a multi-threaded
    /// context, but it is warranted that if `is_empty()` returns true, a
    /// subsequent call to `pop()` will fail.
    pub fn is_empty(&self) -> bool {
        let push_count = self.queue.push_count.load(Relaxed);
        let (pop_count, head) = unpack(self.queue.pop_count_and_head.load(Relaxed));
        let tail = push_count.wrapping_sub(pop_count);

        tail == head
    }

    /// Attempts to push one item at the tail of the queue.
    ///
    /// # Errors
    ///
    /// This will fail if the queue is full, in which case the item is returned
    /// as the error field.
    pub fn push(&self, item: T) -> Result<(), T> {
        let push_count = self.queue.push_count.load(Relaxed);
        let pop_count = unpack(self.queue.pop_count_and_head.load(Relaxed)).0;
        let tail = push_count.wrapping_sub(pop_count);

        // Ordering: Acquire ordering is required to synchronize with the
        // Release of the `head` atomic at the end of a stealing operation and
        // ensure that the stealer has finished copying the items from the
        // buffer.
        let head = self.queue.head.load(Acquire);

        // Check that the buffer is not full.
        if tail.wrapping_sub(head) >= B::CAPACITY {
            return Err(item);
        }

        // Store the item.
        unsafe { self.queue.write_at(tail, item) };

        // Make the item visible by incrementing the push count.
        //
        // Ordering: the Release ordering ensures that the subsequent
        // acquisition of this atomic by a stealer will make the previous write
        // visible.
        self.queue
            .push_count
            .store(push_count.wrapping_add(1), Release);

        Ok(())
    }

    /// Attempts to push the content of an iterator at the tail of the queue.
    ///
    /// It is the responsibility of the caller to ensure that there is enough
    /// spare capacity to accommodate all iterator items, for instance by
    /// calling `[Worker::spare_capacity]` beforehand. Otherwise, the iterator
    /// is dropped while still holding the excess items.
    pub fn extend<I: IntoIterator<Item = T>>(&self, iter: I) {
        let push_count = self.queue.push_count.load(Relaxed);
        let pop_count = unpack(self.queue.pop_count_and_head.load(Relaxed)).0;
        let mut tail = push_count.wrapping_sub(pop_count);

        // Ordering: Acquire ordering is required to synchronize with the
        // Release of the `head` atomic at the end of a stealing operation and
        // ensure that the stealer has finished copying the items from the
        // buffer.
        let head = self.queue.head.load(Acquire);

        let max_tail = head.wrapping_add(B::CAPACITY);
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
        self.queue
            .push_count
            .store(tail.wrapping_add(pop_count), Release);
    }

    /// Attempts to pop one item from the tail of the queue.
    ///
    /// This returns None if the queue is empty.
    pub fn pop(&self) -> Option<T> {
        // Acquire the item to be popped.
        //
        // Ordering: Relaxed ordering is sufficient since (i) the push and pop
        // count are only set by this thread and (ii) no stealer will read this
        // slot until it has been again written to with a push operation. In the
        // worse case, the head position read below will be obsolete and the
        // first CAS will fail.
        let mut pop_count_and_head = self.queue.pop_count_and_head.load(Relaxed);
        let push_count = self.queue.push_count.load(Relaxed);

        let (pop_count, mut head) = unpack(pop_count_and_head);
        let tail = push_count.wrapping_sub(pop_count);
        let new_pop_count = pop_count.wrapping_add(1);

        loop {
            // Check if the queue is empty.
            if tail == head {
                return None;
            }
            let new_pop_count_and_head = pack(new_pop_count, head);

            // Attempt to claim this slot.
            //
            // Ordering: Release is necessary so that stealers can acquire the
            // pop count and be sure that all previous push operations have been
            // accounted for, otherwise the calculated tail could end up less
            // than the head.
            match self.queue.pop_count_and_head.compare_exchange_weak(
                pop_count_and_head,
                new_pop_count_and_head,
                Release,
                Relaxed,
            ) {
                Ok(_) => break,
                // We lost the race to a stealer or the CAS failed spuriously; try again.
                Err(current) => {
                    pop_count_and_head = current;
                    head = unpack(current).1;
                }
            }
        }

        // Read the item.
        unsafe { Some(self.queue.read_at(tail.wrapping_sub(1))) }
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
    pub fn drain<C>(&self, count_fn: C) -> Result<Drain<'_, T, B>, StealError>
    where
        C: FnMut(usize) -> usize,
    {
        let (old_head, new_head, _) = self.queue.book_items(count_fn, UnsignedShort::MAX)?;

        Ok(Drain {
            queue: &self.queue,
            current: old_head,
            end: new_head,
        })
    }
}

impl<T, B: Buffer<T>> Default for Worker<T, B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, B: Buffer<T>> UnwindSafe for Worker<T, B> {}
impl<T, B: Buffer<T>> RefUnwindSafe for Worker<T, B> {}
unsafe impl<T: Send, B: Buffer<T>> Send for Worker<T, B> {}

/// A draining iterator for [`Worker<T, B>`].
///
/// This iterator is created by [`Worker::drain`]. See its documentation for
/// more information.
#[derive(Debug)]
pub struct Drain<'a, T, B: Buffer<T>> {
    queue: &'a Queue<T, B>,
    current: UnsignedShort,
    end: UnsignedShort,
}

impl<'a, T, B: Buffer<T>> Iterator for Drain<'a, T, B> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.current == self.end {
            return None;
        }

        let item = Some(unsafe { self.queue.read_at(self.current) });

        self.current = self.current.wrapping_add(1);

        // We cannot rely on the caller to call `next` again after the last item
        // is yielded so the head position must be updated immediately when
        // yielding the last item.
        if self.current == self.end {
            // Update the head position.
            //
            // Ordering: the Release ordering ensures that all items have been moved
            // out when a subsequent push operation synchronizes by acquiring
            // `head`. It also ensures that the push count seen by a subsequent
            // steal operation (which acquires `head`) is at least equal to the one
            // seen by the present steal operation.
            self.queue.head.store(self.end, Release);
        }

        item
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let sz = self.end.wrapping_sub(self.current) as usize;

        (sz, Some(sz))
    }
}

impl<'a, T, B: Buffer<T>> ExactSizeIterator for Drain<'a, T, B> {}

impl<'a, T, B: Buffer<T>> FusedIterator for Drain<'a, T, B> {}

impl<'a, T, B: Buffer<T>> Drop for Drain<'a, T, B> {
    fn drop(&mut self) {
        // Drop all items and make sure the head is updated so that subsequent
        // stealing operations can succeed.
        for _item in self {}
    }
}

impl<'a, T, B: Buffer<T>> UnwindSafe for Drain<'a, T, B> {}
impl<'a, T, B: Buffer<T>> RefUnwindSafe for Drain<'a, T, B> {}
unsafe impl<'a, T: Send, B: Buffer<T>> Send for Drain<'a, T, B> {}
unsafe impl<'a, T: Send, B: Buffer<T>> Sync for Drain<'a, T, B> {}

/// Handle for multi-threaded stealing operations.
#[derive(Debug)]
pub struct Stealer<T, B: Buffer<T>> {
    queue: Arc<Queue<T, B>>,
}

impl<T, B: Buffer<T>> Stealer<T, B> {
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
    pub fn steal<C, BDest>(&self, dest: &Worker<T, BDest>, count_fn: C) -> Result<usize, StealError>
    where
        C: FnMut(usize) -> usize,
        BDest: Buffer<T>,
    {
        // Compute the free capacity of the destination queue.
        //
        // Note that even if the value of `dest_head` is stale, the subtraction
        // that computes `dest_free_capacity` can never overflow since it is
        // computed on the same thread that pushes items to the destination
        // queue, but `push` would fail if `dest_head` suggested that there is
        // no spare capacity.
        //
        // Ordering: see `Worker::push()` method.
        let dest_push_count = dest.queue.push_count.load(Relaxed);
        let dest_pop_count = unpack(dest.queue.pop_count_and_head.load(Relaxed)).0;
        let dest_tail = dest_push_count.wrapping_sub(dest_pop_count);
        let dest_head = dest.queue.head.load(Acquire);
        let dest_free_capacity = BDest::CAPACITY - dest_tail.wrapping_sub(dest_head);

        debug_or_loom_assert!(dest_free_capacity <= BDest::CAPACITY);

        let (old_head, new_head, transfer_count) =
            self.queue.book_items(count_fn, dest_free_capacity)?;

        debug_or_loom_assert!(transfer_count <= dest_free_capacity);

        // Move all items but the last to the destination queue.
        for offset in 0..transfer_count {
            unsafe {
                let item = self.queue.read_at(old_head.wrapping_add(offset));
                dest.queue.write_at(dest_tail.wrapping_add(offset), item);
            }
        }

        // Make the moved items visible by updating the destination tail position.
        //
        // Ordering: see comments in the `push()` method.
        dest.queue
            .push_count
            .store(dest_push_count.wrapping_add(transfer_count), Release);

        // Update the head position.
        //
        // Ordering: the Release ordering ensures that all items have been moved
        // out when a subsequent push operation synchronizes by acquiring
        // `head`. It also ensures that the push count seen by a subsequent
        // steal operation (which acquires `head`) is at least equal to the one
        // seen by the present steal operation.
        self.queue.head.store(new_head, Release);

        Ok(transfer_count as usize)
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
    pub fn steal_and_pop<C, BDest>(
        &self,
        dest: &Worker<T, BDest>,
        count_fn: C,
    ) -> Result<(T, usize), StealError>
    where
        C: FnMut(usize) -> usize,
        BDest: Buffer<T>,
    {
        // Compute the free capacity of the destination queue.
        //
        // Ordering: see `Worker::push()` method.
        let dest_push_count = dest.queue.push_count.load(Relaxed);
        let dest_pop_count = unpack(dest.queue.pop_count_and_head.load(Relaxed)).0;
        let dest_tail = dest_push_count.wrapping_sub(dest_pop_count);
        let dest_head = dest.queue.head.load(Acquire);
        let dest_free_capacity = BDest::CAPACITY - dest_tail.wrapping_sub(dest_head);

        debug_or_loom_assert!(dest_free_capacity <= BDest::CAPACITY);

        let (old_head, new_head, count) =
            self.queue.book_items(count_fn, dest_free_capacity + 1)?;
        let transfer_count = count - 1;

        debug_or_loom_assert!(transfer_count <= dest_free_capacity);

        // Move all items but the last to the destination queue.
        for offset in 0..transfer_count {
            unsafe {
                let item = self.queue.read_at(old_head.wrapping_add(offset));
                dest.queue.write_at(dest_tail.wrapping_add(offset), item);
            }
        }

        // Read the last item.
        let last_item = unsafe { self.queue.read_at(old_head.wrapping_add(transfer_count)) };

        // Make the moved items visible by updating the destination tail position.
        //
        // Ordering: see comments in the `push()` method.
        dest.queue
            .push_count
            .store(dest_push_count.wrapping_add(transfer_count), Release);

        // Update the head position.
        //
        // Ordering: the Release ordering ensures that all items have been moved
        // out when a subsequent push operation synchronizes by acquiring
        // `head`. It also ensures that the push count seen by a subsequent
        // steal operation (which acquires `head`) is at least equal to the one
        // seen by the present steal operation.
        self.queue.head.store(new_head, Release);

        Ok((last_item, transfer_count as usize))
    }
}

impl<T, B: Buffer<T>> Clone for Stealer<T, B> {
    fn clone(&self) -> Self {
        Stealer {
            queue: self.queue.clone(),
        }
    }
}

impl<T, B: Buffer<T>> UnwindSafe for Stealer<T, B> {}
impl<T, B: Buffer<T>> RefUnwindSafe for Stealer<T, B> {}
unsafe impl<T: Send, B: Buffer<T>> Send for Stealer<T, B> {}
unsafe impl<T: Send, B: Buffer<T>> Sync for Stealer<T, B> {}
