use std::iter;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::spawn;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, Criterion};
use num_cpus;
use st3;

mod generic_queue;
mod tokio_queue;

use generic_queue::{
    CrossbeamFifoWorker, CrossbeamLifoWorker, GenericStealError, GenericStealer, GenericWorker,
};

// Single-threaded benchmark.
//
// `N` items are pushed and then popped from the queue.
pub fn push_pop<T, W: GenericWorker<usize>, const N: usize>(name: &str, c: &mut Criterion) {
    let worker = W::new();
    c.bench_function(&format!("push_pop-{}", name), |b| {
        b.iter(|| {
            for i in 0..N {
                let _ = worker.push(i);
            }
            for _ in 0..N {
                let _ = worker.pop();
            }
        })
    });
}

pub fn push_pop_small_batch_st3_fifo(c: &mut Criterion) {
    push_pop::<usize, st3::fifo::Worker<_, st3::B256>, 64>("small_batch-st3_fifo", c);
}
pub fn push_pop_small_batch_st3_lifo(c: &mut Criterion) {
    push_pop::<usize, st3::lifo::Worker<_, st3::B256>, 64>("small_batch-st3_lifo", c);
}
pub fn push_pop_small_batch_tokio(c: &mut Criterion) {
    push_pop::<usize, tokio_queue::Local<_>, 64>("small_batch-tokio", c);
}
pub fn push_pop_small_batch_crossbeam_fifo(c: &mut Criterion) {
    push_pop::<usize, CrossbeamFifoWorker<_>, 64>("small_batch-crossbeam_fifo", c);
}
pub fn push_pop_small_batch_crossbeam_lifo(c: &mut Criterion) {
    push_pop::<usize, CrossbeamLifoWorker<_>, 64>("small_batch-crossbeam_lifo", c);
}
pub fn push_pop_large_batch_st3_fifo(c: &mut Criterion) {
    push_pop::<usize, st3::fifo::Worker<_, st3::B256>, 256>("large_batch-st3_fifo", c);
}
pub fn push_pop_large_batch_st3_lifo(c: &mut Criterion) {
    push_pop::<usize, st3::lifo::Worker<_, st3::B256>, 256>("large_batch-st3_lifo", c);
}
pub fn push_pop_large_batch_tokio(c: &mut Criterion) {
    push_pop::<usize, tokio_queue::Local<_>, 256>("large_batch-tokio", c);
}
pub fn push_pop_large_batch_crossbeam_fifo(c: &mut Criterion) {
    push_pop::<usize, CrossbeamFifoWorker<_>, 256>("large_batch-crossbeam_fifo", c);
}
pub fn push_pop_large_batch_crossbeam_lifo(c: &mut Criterion) {
    push_pop::<usize, CrossbeamLifoWorker<_>, 256>("large_batch-crossbeam_lifo", c);
}

// Multi-threaded synthetic benchmark.
//
// This is a toy work-stealing executor which schedules a random number of tasks
// (up to 256), each one being repeated by re-injection onto its worker a random
// number of times (up to 100). The number of tasks initially assigned to each
// workers and the number of times each task is to be repeated are
// deterministically pre-determined, meaning that the workload is the same for
// all benchmarked queues. All queues use the crossbeam work-stealing strategy:
// half of the tasks are stolen, up to a maximum of 32 tasks. Nevertheless, the
// re-distribution of tasks via work-stealing is ultimately non-deterministic as
// it is affected by thread timing. The numbers from this benchmark must
// therefore be taken with a grain of salt.
pub fn executor<T, W: GenericWorker<u64> + 'static>(name: &str, c: &mut Criterion) {
    c.bench_function(&format!("executor-{}", name), |b| {
        // The maximum number of tasks to be enqueued.
        const MAX_TASKS_PER_THREAD: u64 = 256;
        // The maximum number of times a task may be re-injected onto a worker.
        const MAX_TASK_REPEAT: u64 = 100;

        // Use up to 4 threads, depending on the number of CPU threads.
        let thread_count = num_cpus::get().min(4);

        b.iter(|| {
            let workers: Vec<_> = iter::repeat_with(|| W::new()).take(thread_count).collect();
            let stealers: Vec<_> = workers.iter().map(|w| w.stealer()).collect();
            // A barrier that block all threads until all are ready.
            let wait_barrier = Arc::new((Mutex::new(0usize), Condvar::new()));
            let mut threads = Vec::with_capacity(thread_count);

            for (th_id, worker) in workers.into_iter().enumerate() {
                let stealers = stealers.clone();
                let wait_barrier = wait_barrier.clone();

                threads.push(spawn(move || -> Option<Instant> {
                    // Seed the RNG with the thread ID.
                    let mut rng = oorandom::Rand64::new(th_id as u128);

                    // Schedule from 1 to MAX_TASKS_PER_THREAD tasks.
                    let task_count = rng.rand_range(1..(MAX_TASKS_PER_THREAD + 1));
                    for _ in 0..task_count {
                        worker
                            .push(rng.rand_range(0..MAX_TASK_REPEAT))
                            .expect("attempting to schedule more tasks than the queue can hold");
                    }

                    // Preallocate an array that will be populated with the IDs
                    // of other workers.
                    let mut other_workers_id = Vec::with_capacity(thread_count - 1);
                    other_workers_id.resize(thread_count - 1, 0);

                    // Block until all threads are ready.
                    let start_time = {
                        let (lock, cvar) = &*wait_barrier;
                        let mut ready_count = lock.lock().unwrap();
                        *ready_count += 1;
                        if *ready_count == thread_count {
                            cvar.notify_all();

                            Some(Instant::now())
                        } else {
                            // Block until all threads are ready.
                            while *ready_count < thread_count {
                                ready_count = cvar.wait(ready_count).unwrap();
                            }

                            None
                        }
                    };

                    // Loop over all tasks in the queue and enqueue them again
                    // after decreasing their repeat count, until the count
                    // falls to 0.
                    'new_task: loop {
                        if let Some(repeat_count) = worker.pop() {
                            // Re-inject the task if needed.
                            if repeat_count > 0 {
                                // The actual queue size should never be greater
                                // than `MAX_TASKS_PER_THREAD`. Unfortunately
                                // the tokio queue can spuriously report a full
                                // queue on a push when many pop/push operations
                                // are made while a stealer is preempted. In
                                // such rare cases, spinning is necessary.
                                while let Err(_) = worker.push(repeat_count - 1) {}
                            }
                        } else {
                            // No more local tasks, try to steal.
                            // Make an array containing all other thread IDs.
                            for (i, th) in other_workers_id.iter_mut().enumerate() {
                                if i < th_id {
                                    *th = i;
                                } else {
                                    *th = i + 1;
                                }
                            }

                            // Randomly select a worker to steal from. Upon
                            // failure, try again until all workers have been
                            // found empty.
                            let mut pool_size = thread_count - 1;
                            loop {
                                // Choose randomly another worker to steal from.
                                let idx = rng.rand_range(0..pool_size as u64) as usize;
                                let steal_from = other_workers_id[idx];
                                match stealers[steal_from].steal_batch_and_pop(&worker) {
                                    Ok(repeat_count) => {
                                        // Re-inject the task if needed.
                                        if repeat_count > 0 {
                                            while let Err(_) = worker.push(repeat_count - 1) {}
                                        }
                                        continue 'new_task;
                                    }
                                    Err(GenericStealError::Empty) => {
                                        // Give up if all workers were found
                                        // empty when attempting to steal and
                                        // this was the last worker.
                                        if pool_size == 1 {
                                            return start_time;
                                        }
                                        // Evict the sampled worker from the
                                        // pool and replace it with the last
                                        // worker in the list, then try again
                                        // to steal within this reduced pool.
                                        pool_size -= 1;
                                        other_workers_id[idx] = other_workers_id[pool_size];
                                    }
                                    Err(GenericStealError::Busy) => {}
                                }
                            }
                        }
                    }
                }));
            }
            let mut start = None;
            for th in threads {
                if let Ok(Some(start_time)) = th.join() {
                    start = Some(start_time);
                }
            }

            start.unwrap().elapsed()
        })
    });
}
pub fn executor_st3_fifo(c: &mut Criterion) {
    executor::<usize, st3::fifo::Worker<_, st3::B256>>("st3_fifo", c);
}
pub fn executor_st3_lifo(c: &mut Criterion) {
    executor::<usize, st3::lifo::Worker<_, st3::B256>>("st3_lifo", c);
}
pub fn executor_tokio(c: &mut Criterion) {
    executor::<usize, tokio_queue::Local<_>>("tokio", c);
}
pub fn executor_crossbeam_fifo(c: &mut Criterion) {
    executor::<usize, CrossbeamFifoWorker<_>>("crossbeam_fifo", c);
}
pub fn executor_crossbeam_lifo(c: &mut Criterion) {
    executor::<usize, CrossbeamLifoWorker<_>>("crossbeam_lifo", c);
}

criterion_group!(
    benches,
    push_pop_small_batch_st3_fifo,
    push_pop_small_batch_st3_lifo,
    push_pop_small_batch_tokio,
    push_pop_small_batch_crossbeam_fifo,
    push_pop_small_batch_crossbeam_lifo,
    push_pop_large_batch_st3_fifo,
    push_pop_large_batch_st3_lifo,
    push_pop_large_batch_tokio,
    push_pop_large_batch_crossbeam_fifo,
    push_pop_large_batch_crossbeam_lifo,
    executor_st3_fifo,
    executor_st3_lifo,
    executor_tokio,
    executor_crossbeam_fifo,
    executor_crossbeam_lifo,
);
criterion_main!(benches);
