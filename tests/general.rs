use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::spawn;

use st3::{Buffer, StealError, Stealer, Worker};

// Rotate the internal ring buffer indices by `n`.
fn rotate<T: Default + std::fmt::Debug, B: Buffer<T>>(worker: &Worker<T, B>, n: usize) {
    let stealer = worker.stealer();
    let dummy_worker = Worker::<T, st3::B2>::new();

    for _ in 0..n {
        worker.push(T::default()).unwrap();
        stealer.steal_and_pop(&dummy_worker, |_| 1).unwrap();
    }
}

#[test]
fn single_threaded_steal() {
    for rotation in [0, 255, 256, 257, 65535, 65536, 65537] {
        let worker1 = Worker::<_, st3::B128>::new();
        let worker2 = Worker::<_, st3::B128>::new();
        let stealer1 = worker1.stealer();
        rotate(&worker1, rotation);
        rotate(&worker2, rotation);

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
fn self_steal() {
    for rotation in [0, 255, 256, 257, 65535, 65536, 65537] {
        let worker = Worker::<_, st3::B128>::new();
        rotate(&worker, rotation);
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
fn drain_steal() {
    for rotation in [0, 255, 256, 257, 65535, 65536, 65537] {
        let worker = Worker::<_, st3::B128>::new();
        let dummy_worker = Worker::<_, st3::B128>::new();
        let stealer1 = worker.stealer();
        let stealer2 = worker.stealer();
        rotate(&worker, rotation);

        worker.push(1).unwrap();
        worker.push(2).unwrap();
        worker.push(3).unwrap();
        worker.push(4).unwrap();

        assert_eq!(worker.pop(), Some(4));
        let mut iter = stealer1.drain(|n| n - 1).unwrap();
        assert_eq!(
            stealer2.steal_and_pop(&dummy_worker, |_| 1),
            Err(StealError::Busy)
        );
        assert_eq!(iter.next(), Some(1));
        assert_eq!(
            stealer2.steal_and_pop(&dummy_worker, |_| 1),
            Err(StealError::Busy)
        );
        assert_eq!(iter.next(), Some(2));
        assert_eq!(stealer2.steal_and_pop(&dummy_worker, |_| 1), Ok((3, 0)));
        assert_eq!(iter.next(), None);
    }
}

#[test]
fn multi_threaded_steal() {
    const N: usize = 80_000_000;

    let counter = Arc::new(AtomicUsize::new(0));
    let worker = Worker::<_, st3::B128>::new();
    let stealer = worker.stealer();

    let counter0 = counter.clone();
    let stealer1 = stealer.clone();
    let counter1 = counter.clone();
    let stealer2 = stealer;
    let counter2 = counter;

    // Worker thread.
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
        stealer: Stealer<usize, st3::B128>,
        counter: Arc<AtomicUsize>,
        rng_seed: u64,
    ) -> Vec<usize> {
        let mut stats = vec![0; N];
        let mut rng = oorandom::Rand32::new(rng_seed);
        let dest_worker = Worker::<_, st3::B128>::new();

        loop {
            if let Ok((i, _)) =
                stealer.steal_and_pop(&dest_worker, |m| rng.rand_range(0..(m + 1) as u32) as usize)
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
