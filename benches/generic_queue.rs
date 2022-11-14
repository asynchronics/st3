//! Generic traits for queue benchmarking.

use crate::tokio_queue;
use st3;

/// Error returned on stealing failure.
pub enum GenericStealError {
    Empty,
    Busy,
}

/// Generic interface for a queue worker.
pub trait GenericWorker<T>: Send {
    type S: GenericStealer<T, W = Self>;

    fn new() -> Self;
    fn push(&self, item: T) -> Result<(), T>;
    fn pop(&self) -> Option<T>;
    fn stealer(&self) -> Self::S;
}

/// Generic interface for a queue stealer.
pub trait GenericStealer<T>: Clone + Send + Sync {
    type W: GenericWorker<T>;

    fn steal_batch_and_pop(&self, worker: &Self::W) -> Result<T, GenericStealError>;
}

/// Generic work-stealing queue traits implementation for St3 (LIFO).
impl<T: Send, B: st3::Buffer<T>> GenericWorker<T> for st3::lifo::Worker<T, B> {
    type S = st3::lifo::Stealer<T, B>;

    fn new() -> Self {
        Self::new()
    }
    fn push(&self, item: T) -> Result<(), T> {
        self.push(item)
    }
    fn pop(&self) -> Option<T> {
        self.pop()
    }
    fn stealer(&self) -> Self::S {
        self.stealer()
    }
}
impl<T: Send, B: st3::Buffer<T>> GenericStealer<T> for st3::lifo::Stealer<T, B> {
    type W = st3::lifo::Worker<T, B>;

    fn steal_batch_and_pop(&self, worker: &Self::W) -> Result<T, GenericStealError> {
        // The maximum number of tasks to be stolen is limited in order to match
        // the behavior of `crossbeam-dequeue`.
        const MAX_BATCH_SIZE: usize = 32;

        self.steal_and_pop(worker, |n| (n - n / 2).min(MAX_BATCH_SIZE))
            .map(|out| out.0)
            .map_err(|e| match e {
                st3::StealError::Empty => GenericStealError::Empty,
                st3::StealError::Busy => GenericStealError::Busy,
            })
    }
}

/// Generic work-stealing queue traits implementation for St3 (FIFO).
impl<T: Send, B: st3::Buffer<T>> GenericWorker<T> for st3::fifo::Worker<T, B> {
    type S = st3::fifo::Stealer<T, B>;

    fn new() -> Self {
        Self::new()
    }
    fn push(&self, item: T) -> Result<(), T> {
        self.push(item)
    }
    fn pop(&self) -> Option<T> {
        self.pop()
    }
    fn stealer(&self) -> Self::S {
        self.stealer()
    }
}
impl<T: Send, B: st3::Buffer<T>> GenericStealer<T> for st3::fifo::Stealer<T, B> {
    type W = st3::fifo::Worker<T, B>;

    fn steal_batch_and_pop(&self, worker: &Self::W) -> Result<T, GenericStealError> {
        // The maximum number of tasks to be stolen is limited in order to match
        // the behavior of `crossbeam-dequeue`.
        const MAX_BATCH_SIZE: usize = 32;

        self.steal_and_pop(worker, |n| (n - n / 2).min(MAX_BATCH_SIZE))
            .map(|out| out.0)
            .map_err(|e| match e {
                st3::StealError::Empty => GenericStealError::Empty,
                st3::StealError::Busy => GenericStealError::Busy,
            })
    }
}

/// Generic work-stealing queue traits implementation for tokio.
impl<T: Send> GenericWorker<T> for tokio_queue::Local<T> {
    type S = tokio_queue::Steal<T>;

    fn new() -> Self {
        Self::new()
    }
    fn push(&self, item: T) -> Result<(), T> {
        self.push_back(item)
    }
    fn pop(&self) -> Option<T> {
        self.pop()
    }
    fn stealer(&self) -> Self::S {
        self.stealer()
    }
}
impl<T: Send> GenericStealer<T> for tokio_queue::Steal<T> {
    type W = tokio_queue::Local<T>;

    fn steal_batch_and_pop(&self, worker: &Self::W) -> Result<T, GenericStealError> {
        // Note that `steal_into` was slightly altered compared to the original
        // tokio implementation in order to match the behavior of
        // `crossbeam-dequeue`.
        self.steal_into(worker).map_err(|e| match e {
            tokio_queue::StealError::Empty => GenericStealError::Empty,
            tokio_queue::StealError::Busy => GenericStealError::Busy,
        })
    }
}

/// Newtypes distinguishing between FIFO and LIFO crossbeam queues.
pub struct CrossbeamFifoWorker<T>(crossbeam_deque::Worker<T>);
pub struct CrossbeamFifoStealer<T>(crossbeam_deque::Stealer<T>);
pub struct CrossbeamLifoWorker<T>(crossbeam_deque::Worker<T>);
pub struct CrossbeamLifoStealer<T>(crossbeam_deque::Stealer<T>);

/// Generic work-stealing queue traits implementation for crossbeam-deque (FIFO).
impl<T: Send> GenericWorker<T> for CrossbeamFifoWorker<T> {
    type S = CrossbeamFifoStealer<T>;

    fn new() -> Self {
        Self(crossbeam_deque::Worker::new_fifo())
    }
    fn push(&self, item: T) -> Result<(), T> {
        self.0.push(item);

        Ok(())
    }
    fn pop(&self) -> Option<T> {
        self.0.pop()
    }
    fn stealer(&self) -> Self::S {
        CrossbeamFifoStealer(self.0.stealer())
    }
}
impl<T> Clone for CrossbeamFifoStealer<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T: Send> GenericStealer<T> for CrossbeamFifoStealer<T> {
    type W = CrossbeamFifoWorker<T>;

    fn steal_batch_and_pop(&self, worker: &Self::W) -> Result<T, GenericStealError> {
        match self.0.steal_batch_and_pop(&worker.0) {
            crossbeam_deque::Steal::Empty => Err(GenericStealError::Empty),
            crossbeam_deque::Steal::Retry => Err(GenericStealError::Busy),
            crossbeam_deque::Steal::Success(item) => Ok(item),
        }
    }
}

/// Generic work-stealing queue traits implementation for crossbeam-deque (LIFO).
impl<T: Send> GenericWorker<T> for CrossbeamLifoWorker<T> {
    type S = CrossbeamLifoStealer<T>;

    fn new() -> Self {
        Self(crossbeam_deque::Worker::new_lifo())
    }
    fn push(&self, item: T) -> Result<(), T> {
        self.0.push(item);

        Ok(())
    }
    fn pop(&self) -> Option<T> {
        self.0.pop()
    }
    fn stealer(&self) -> Self::S {
        CrossbeamLifoStealer(self.0.stealer())
    }
}
impl<T> Clone for CrossbeamLifoStealer<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T: Send> GenericStealer<T> for CrossbeamLifoStealer<T> {
    type W = CrossbeamLifoWorker<T>;

    fn steal_batch_and_pop(&self, worker: &Self::W) -> Result<T, GenericStealError> {
        match self.0.steal_batch_and_pop(&worker.0) {
            crossbeam_deque::Steal::Empty => Err(GenericStealError::Empty),
            crossbeam_deque::Steal::Retry => Err(GenericStealError::Busy),
            crossbeam_deque::Steal::Success(item) => Ok(item),
        }
    }
}
