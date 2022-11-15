use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::spawn;

use st3::{fifo, lifo, StealError};

// Rotates the internal ring buffer indices by `n` (FIFO queue).
fn fifo_rotate<T: Default + std::fmt::Debug>(worker: &fifo::Worker<T>, n: usize) {
    let stealer = worker.stealer();
    let dummy_worker = fifo::Worker::<T>::new(2);

    for _ in 0..n {
        worker.push(T::default()).unwrap();
        stealer.steal_and_pop(&dummy_worker, |_| 1).unwrap();
    }
}

// Rotates the internal ring buffer indices by `n` (LIFO queue).
fn lifo_rotate<T: Default + std::fmt::Debug>(worker: &lifo::Worker<T>, n: usize) {
    let stealer = worker.stealer();
    let dummy_worker = lifo::Worker::<T>::new(2);

    for _ in 0..n {
        worker.push(T::default()).unwrap();
        stealer.steal_and_pop(&dummy_worker, |_| 1).unwrap();
    }
}

#[test]
fn fifo_single_threaded_steal() {
    const ROTATIONS: &[usize] = if cfg!(miri) {
        &[42]
    } else {
        &[0, 255, 256, 257, 65535, 65536, 65537]
    };

    for &rotation in ROTATIONS {
        let worker1 = fifo::Worker::new(128);
        let worker2 = fifo::Worker::new(128);
        let stealer1 = worker1.stealer();
        fifo_rotate(&worker1, rotation);
        fifo_rotate(&worker2, rotation);

        worker1.push(1).unwrap();
        worker1.push(2).unwrap();
        worker1.push(3).unwrap();
        worker1.push(4).unwrap();

        assert_eq!(worker1.pop(), Some(1));
        assert_eq!(stealer1.steal_and_pop(&worker2, |_| 2), Ok((3, 1)));
        assert_eq!(worker1.pop(), Some(4));
        assert_eq!(worker1.pop(), None);
        assert_eq!(worker2.pop(), Some(2));
        assert_eq!(worker2.pop(), None);
    }
}

#[test]
fn lifo_single_threaded_steal() {
    const ROTATIONS: &[usize] = if cfg!(miri) {
        &[42]
    } else {
        &[0, 255, 256, 257, 65535, 65536, 65537]
    };

    for &rotation in ROTATIONS {
        let worker1 = lifo::Worker::new(128);
        let worker2 = lifo::Worker::new(128);
        let stealer1 = worker1.stealer();
        lifo_rotate(&worker1, rotation);
        lifo_rotate(&worker2, rotation);

        worker1.push(1).unwrap();
        worker1.push(2).unwrap();
        worker1.push(3).unwrap();
        worker1.push(4).unwrap();

        assert_eq!(worker1.pop(), Some(4));
        assert_eq!(stealer1.steal_and_pop(&worker2, |_| 2), Ok((2, 1)));
        assert_eq!(worker1.pop(), Some(3));
        assert_eq!(worker1.pop(), None);
        assert_eq!(worker2.pop(), Some(1));
        assert_eq!(worker2.pop(), None);
    }
}

#[test]
fn fifo_self_steal() {
    const ROTATIONS: &[usize] = if cfg!(miri) {
        &[42]
    } else {
        &[0, 255, 256, 257, 65535, 65536, 65537]
    };

    for &rotation in ROTATIONS {
        let worker = fifo::Worker::new(128);
        fifo_rotate(&worker, rotation);
        let stealer = worker.stealer();

        worker.push(1).unwrap();
        worker.push(2).unwrap();
        worker.push(3).unwrap();
        worker.push(4).unwrap();

        assert_eq!(worker.pop(), Some(1));
        assert_eq!(stealer.steal_and_pop(&worker, |_| 2), Ok((3, 1)));
        assert_eq!(worker.pop(), Some(4));
        assert_eq!(worker.pop(), Some(2));
        assert_eq!(worker.pop(), None);
    }
}

#[test]
fn lifo_self_steal() {
    const ROTATIONS: &[usize] = if cfg!(miri) {
        &[42]
    } else {
        &[0, 255, 256, 257, 65535, 65536, 65537]
    };

    for &rotation in ROTATIONS {
        let worker = lifo::Worker::new(128);
        lifo_rotate(&worker, rotation);
        let stealer = worker.stealer();

        worker.push(1).unwrap();
        worker.push(2).unwrap();
        worker.push(3).unwrap();
        worker.push(4).unwrap();

        assert_eq!(worker.pop(), Some(4));
        assert_eq!(stealer.steal_and_pop(&worker, |_| 2), Ok((2, 1)));
        assert_eq!(worker.pop(), Some(1));
        assert_eq!(worker.pop(), Some(3));
        assert_eq!(worker.pop(), None);
    }
}

#[test]
fn fifo_drain_steal() {
    const ROTATIONS: &[usize] = if cfg!(miri) {
        &[42]
    } else {
        &[0, 255, 256, 257, 65535, 65536, 65537]
    };

    for &rotation in ROTATIONS {
        let worker = fifo::Worker::new(128);
        let dummy_worker = fifo::Worker::new(128);
        let stealer = worker.stealer();
        fifo_rotate(&worker, rotation);

        worker.push(1).unwrap();
        worker.push(2).unwrap();
        worker.push(3).unwrap();
        worker.push(4).unwrap();

        assert_eq!(worker.pop(), Some(1));
        let mut iter = worker.drain(|n| n - 1).unwrap();
        assert_eq!(
            stealer.steal_and_pop(&dummy_worker, |_| 1),
            Err(StealError::Busy)
        );
        assert_eq!(iter.next(), Some(2));
        assert_eq!(
            stealer.steal_and_pop(&dummy_worker, |_| 1),
            Err(StealError::Busy)
        );
        assert_eq!(iter.next(), Some(3));
        assert_eq!(stealer.steal_and_pop(&dummy_worker, |_| 1), Ok((4, 0)));
        assert_eq!(iter.next(), None);
    }
}

#[test]
fn lifo_drain_steal() {
    const ROTATIONS: &[usize] = if cfg!(miri) {
        &[42]
    } else {
        &[0, 255, 256, 257, 65535, 65536, 65537]
    };

    for &rotation in ROTATIONS {
        let worker = lifo::Worker::new(128);
        let dummy_worker = lifo::Worker::new(128);
        let stealer = worker.stealer();
        lifo_rotate(&worker, rotation);

        worker.push(1).unwrap();
        worker.push(2).unwrap();
        worker.push(3).unwrap();
        worker.push(4).unwrap();

        assert_eq!(worker.pop(), Some(4));
        let mut iter = worker.drain(|n| n - 1).unwrap();
        assert_eq!(
            stealer.steal_and_pop(&dummy_worker, |_| 1),
            Err(StealError::Busy)
        );
        assert_eq!(iter.next(), Some(1));
        assert_eq!(
            stealer.steal_and_pop(&dummy_worker, |_| 1),
            Err(StealError::Busy)
        );
        assert_eq!(iter.next(), Some(2));
        assert_eq!(stealer.steal_and_pop(&dummy_worker, |_| 1), Ok((3, 0)));
        assert_eq!(iter.next(), None);
    }
}

#[test]
fn fifo_extend_basic() {
    const ROTATIONS: &[usize] = if cfg!(miri) {
        &[42]
    } else {
        &[0, 255, 256, 257, 65535, 65536, 65537]
    };

    for &rotation in ROTATIONS {
        let worker = fifo::Worker::new(128);
        fifo_rotate(&worker, rotation);

        let initial_capacity = worker.spare_capacity();
        worker.push(1).unwrap();
        worker.push(2).unwrap();
        worker.extend([3, 4]);

        assert_eq!(worker.spare_capacity(), initial_capacity - 4);
        assert_eq!(worker.pop(), Some(1));
        assert_eq!(worker.pop(), Some(2));
        assert_eq!(worker.pop(), Some(3));
        assert_eq!(worker.pop(), Some(4));
        assert_eq!(worker.pop(), None);
    }
}

#[test]
fn lifo_extend_basic() {
    const ROTATIONS: &[usize] = if cfg!(miri) {
        &[42]
    } else {
        &[0, 255, 256, 257, 65535, 65536, 65537]
    };

    for &rotation in ROTATIONS {
        let worker = lifo::Worker::new(128);
        lifo_rotate(&worker, rotation);

        let initial_capacity = worker.spare_capacity();
        worker.push(1).unwrap();
        worker.push(2).unwrap();
        worker.extend([3, 4]);

        assert_eq!(worker.spare_capacity(), initial_capacity - 4);
        assert_eq!(worker.pop(), Some(4));
        assert_eq!(worker.pop(), Some(3));
        assert_eq!(worker.pop(), Some(2));
        assert_eq!(worker.pop(), Some(1));
        assert_eq!(worker.pop(), None);
    }
}

#[test]
fn fifo_extend_overflow() {
    const ROTATIONS: &[usize] = if cfg!(miri) {
        &[42]
    } else {
        &[0, 255, 256, 257, 65535, 65536, 65537]
    };

    for &rotation in ROTATIONS {
        let worker = fifo::Worker::new(128);
        fifo_rotate(&worker, rotation);

        let initial_capacity = worker.spare_capacity();
        worker.push(1).unwrap();
        worker.push(2).unwrap();
        worker.extend(3..); // try to append infinitely many integers

        assert_eq!(worker.spare_capacity(), 0);
        for i in 1..=initial_capacity {
            assert_eq!(worker.pop(), Some(i));
        }
        assert_eq!(worker.pop(), None);
    }
}

#[test]
fn lifo_extend_overflow() {
    const ROTATIONS: &[usize] = if cfg!(miri) {
        &[42]
    } else {
        &[0, 255, 256, 257, 65535, 65536, 65537]
    };

    for &rotation in ROTATIONS {
        let worker = lifo::Worker::new(128);
        lifo_rotate(&worker, rotation);

        let initial_capacity = worker.spare_capacity();
        worker.push(1).unwrap();
        worker.push(2).unwrap();
        worker.extend(3..); // try to append infinitely many integers

        assert_eq!(worker.spare_capacity(), 0);
        for i in (1..=initial_capacity).rev() {
            assert_eq!(worker.pop(), Some(i));
        }
        assert_eq!(worker.pop(), None);
    }
}

macro_rules! multi_threaded_steal {
    ($flavor:ident, $test_name:ident) => {
        #[test]
        fn $test_name() {
            const N: usize = if cfg!(miri) { 200 } else { 80_000_000 };

            let counter = Arc::new(AtomicUsize::new(0));
            let worker = $flavor::Worker::new(128);
            let stealer = worker.stealer();

            let counter0 = counter.clone();
            let stealer1 = stealer.clone();
            let counter1 = counter.clone();
            let stealer2 = stealer;
            let counter2 = counter;

            // lifo::Worker thread.
            //
            // Push all numbers from 0 to N, popping one from time to time.
            let t0 = spawn(move || {
                let mut i = 0;
                let mut rng = oorandom::Rand32::new(0);
                let mut stats = vec![0; N];
                'outer: loop {
                    for _ in 0..rng.rand_range(1..10) {
                        while let Err(_) = worker.push(i) {}
                        i += 1;
                        if i == N {
                            break 'outer;
                        }
                    }
                    if let Some(j) = worker.pop() {
                        stats[j] += 1;
                        counter0.fetch_add(1, Ordering::Relaxed);
                    }
                }

                stats
            });

            // Stealer threads.
            //
            // Repeatedly steal a random number of items.
            fn steal_periodically(
                stealer: $flavor::Stealer<usize>,
                counter: Arc<AtomicUsize>,
                rng_seed: u64,
            ) -> Vec<usize> {
                let mut stats = vec![0; N];
                let mut rng = oorandom::Rand32::new(rng_seed);
                let dest_worker = $flavor::Worker::new(128);

                loop {
                    if let Ok((i, _)) = stealer
                        .steal_and_pop(&dest_worker, |m| rng.rand_range(0..(m + 1) as u32) as usize)
                    {
                        stats[i] += 1; // the popped item
                        counter.fetch_add(1, Ordering::Relaxed);
                        while let Some(j) = dest_worker.pop() {
                            stats[j] += 1;
                            counter.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    let count = counter.load(Ordering::Relaxed);
                    if count == N {
                        break;
                    }
                    assert!(count < N);
                }

                stats
            }
            let t1 = spawn(move || steal_periodically(stealer1, counter1, 1));
            let t2 = spawn(move || steal_periodically(stealer2, counter2, 2));
            let mut stats = Vec::new();
            stats.push(t0.join().unwrap());
            stats.push(t1.join().unwrap());
            stats.push(t2.join().unwrap());
            for i in 0..N {
                let mut count = 0;
                for j in 0..stats.len() {
                    count += stats[j][i];
                }
                assert_eq!(count, 1);
            }
        }
    };
}
multi_threaded_steal!(fifo, fifo_multi_threaded_steal);
multi_threaded_steal!(lifo, lifo_multi_threaded_steal);

macro_rules! queue_capacity {
    ($flavor:ident, $test_name:ident) => {
        #[test]
        fn $test_name() {
            for (min_capacity, expected_capacity) in [(0, 1), (1, 1), (3, 4), (8, 8), (100, 128)] {
                let worker = $flavor::Worker::new(min_capacity);

                assert_eq!(worker.capacity(), expected_capacity);
                assert_eq!(worker.spare_capacity(), expected_capacity);
                for _ in 0..expected_capacity {
                    assert!(worker.push(42).is_ok())
                }
                assert!(worker.push(42).is_err());
                assert_eq!(worker.spare_capacity(), 0);
            }
        }
    };
}

queue_capacity!(fifo, fifo_queue_capacity);
queue_capacity!(lifo, lifo_queue_capacity);
