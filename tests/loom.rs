use loom::thread;

use st3::{Stealer, Worker};

// Test adapted from the Tokio test suite.
#[test]
fn loom_basic_steal() {
    const LOOP_COUNT: usize = 2;
    const ITEM_COUNT_PER_LOOP: usize = 3;

    loom::model(|| {
        let worker = Worker::<usize, st3::B4>::new();
        let stealer = worker.stealer();

        let th = thread::spawn(move || {
            let dest_worker = Worker::<usize, st3::B4>::new();
            let mut n = 0;

            for _ in 0..3 {
                let _ = stealer.steal(&dest_worker, |n| n - n / 2);
                while dest_worker.pop().is_some() {
                    n += 1;
                }
            }

            n
        });

        let mut n = 0;

        for _ in 0..LOOP_COUNT {
            for _ in 0..(ITEM_COUNT_PER_LOOP - 1) {
                if worker.push(42).is_err() {
                    n += 1;
                }
            }

            if worker.pop().is_some() {
                n += 1;
            }

            // Push another task
            if worker.push(42).is_err() {
                n += 1;
            }

            while worker.pop().is_some() {
                n += 1;
            }
        }

        n += th.join().unwrap();

        assert_eq!(ITEM_COUNT_PER_LOOP * LOOP_COUNT, n);
    });
}

// Test adapted from the Tokio test suite.
#[test]
fn loom_drain_overflow() {
    const ITEM_COUNT: usize = 7;

    loom::model(|| {
        let worker = Worker::<usize, st3::B4>::new();
        let stealer = worker.stealer();

        let th = thread::spawn(move || {
            let dest_worker = Worker::<usize, st3::B4>::new();
            let mut n = 0;

            let _ = stealer.steal(&dest_worker, |n| n - n / 2);
            while dest_worker.pop().is_some() {
                n += 1;
            }

            n
        });

        let mut n = 0;

        // Push an item, pop an item.
        worker.push(42).unwrap();

        if worker.pop().is_some() {
            n += 1;
        }

        for _ in 0..(ITEM_COUNT - 1) {
            if worker.push(42).is_err() {
                // Spin until some of the old items can be drained to make room
                // for the new item.
                loop {
                    if let Ok(drain) = worker.drain(|n| n - n / 2) {
                        for _ in drain {
                            n += 1;
                        }
                        assert_eq!(worker.push(42), Ok(()));
                        break;
                    }
                    thread::yield_now();
                }
            }
        }

        n += th.join().unwrap();

        while worker.pop().is_some() {
            n += 1;
        }

        assert_eq!(ITEM_COUNT, n);
    });
}

// Test adapted from the Tokio test suite.
#[test]
fn loom_multi_stealer() {
    const ITEM_COUNT: usize = 5;

    fn steal_half(stealer: Stealer<usize, st3::B4>) -> usize {
        let dest_worker = Worker::<usize, st3::B4>::new();

        let _ = stealer.steal(&dest_worker, |n| n - n / 2);

        let mut n = 0;
        while dest_worker.pop().is_some() {
            n += 1;
        }

        n
    }

    loom::model(|| {
        let worker = Worker::<usize, st3::B4>::new();
        let stealer1 = worker.stealer();
        let stealer2 = worker.stealer();

        let th1 = thread::spawn(move || steal_half(stealer1));
        let th2 = thread::spawn(move || steal_half(stealer2));

        let mut n = 0;
        for _ in 0..ITEM_COUNT {
            if worker.push(42).is_err() {
                n += 1;
            }
        }

        while worker.pop().is_some() {
            n += 1;
        }

        n += th1.join().unwrap();
        n += th2.join().unwrap();

        assert_eq!(ITEM_COUNT, n);
    });
}

// Test adapted from the Tokio test suite.
#[test]
fn loom_chained_steal() {
    loom::model(|| {
        let w1 = Worker::<usize, st3::B4>::new();
        let w2 = Worker::<usize, st3::B4>::new();
        let s1 = w1.stealer();
        let s2 = w2.stealer();

        for _ in 0..4 {
            w1.push(42).unwrap();
            w2.push(42).unwrap();
        }

        let th = thread::spawn(move || {
            let dest_worker = Worker::<usize, st3::B4>::new();
            let _ = s1.steal(&dest_worker, |n| n - n / 2);

            while dest_worker.pop().is_some() {}
        });

        while w1.pop().is_some() {}

        let _ = s2.steal(&w1, |n| n - n / 2);

        th.join().unwrap();

        while w1.pop().is_some() {}
        while w2.pop().is_some() {}
    });
}

// A variant of multi-stealer with concurrent push.
#[test]
fn loom_push_and_steal() {
    fn steal_half(stealer: Stealer<usize, st3::B4>) -> usize {
        let dest_worker = Worker::<usize, st3::B4>::new();

        match stealer.steal(&dest_worker, |n| n - n / 2) {
            Ok(n) => n,
            Err(_) => 0,
        }
    }

    loom::model(|| {
        let worker = Worker::<usize, st3::B4>::new();
        let stealer1 = worker.stealer();
        let stealer2 = worker.stealer();

        let th1 = thread::spawn(move || steal_half(stealer1));
        let th2 = thread::spawn(move || steal_half(stealer2));

        worker.push(42).unwrap();
        worker.push(42).unwrap();

        let mut n = 0;
        while worker.pop().is_some() {
            n += 1;
        }

        n += th1.join().unwrap();
        n += th2.join().unwrap();

        assert_eq!(n, 2);
    });
}

// Attempts extending the queue based on `Worker::free_capacity`.
#[test]
fn loom_extend() {
    fn steal_half(stealer: Stealer<usize, st3::B4>) -> usize {
        let dest_worker = Worker::<usize, st3::B4>::new();

        match stealer.steal(&dest_worker, |n| n - n / 2) {
            Ok(n) => n,
            Err(_) => 0,
        }
    }

    loom::model(|| {
        let worker = Worker::<usize, st3::B4>::new();
        let stealer1 = worker.stealer();
        let stealer2 = worker.stealer();

        let th1 = thread::spawn(move || steal_half(stealer1));
        let th2 = thread::spawn(move || steal_half(stealer2));

        worker.push(1).unwrap();
        worker.push(7).unwrap();

        // Try to fill up the queue.
        let spare_capacity = worker.spare_capacity();
        assert!(spare_capacity >= 2);
        worker.extend(0..spare_capacity);

        let mut n = 0;

        n += th1.join().unwrap();
        n += th2.join().unwrap();

        while worker.pop().is_some() {
            n += 1;
        }

        assert_eq!(2 + spare_capacity, n);
    });
}
