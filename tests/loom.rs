use loom::thread;

use st3::{Stealer, Worker};

// Test adapted from the Tokio test suite.
#[test]
fn loom_basic() {
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

        for _ in 0..2 {
            for _ in 0..2 {
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

        assert_eq!(6, n);
    });
}

// Test adapted from the Tokio test suite.
#[test]
fn loom_steal_overflow() {
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

        // Push a task, pop a task.
        if worker.push(42).is_err() {
            n += 1;
        }

        if worker.pop().is_some() {
            n += 1;
        }

        for _ in 0..6 {
            if worker.push(42).is_err() {
                n += 1;
            }
        }

        n += th.join().unwrap();

        while worker.pop().is_some() {
            n += 1;
        }

        assert_eq!(7, n);
    });
}

// Test adapted from the Tokio test suite.
#[test]
fn loom_multi_stealer() {
    const NUM_TASKS: usize = 5;

    fn steal_tasks(stealer: Stealer<usize, st3::B4>) -> usize {
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

        let th1 = thread::spawn(move || steal_tasks(stealer1));
        let th2 = thread::spawn(move || steal_tasks(stealer2));

        let mut n = 0;
        for _ in 0..NUM_TASKS {
            if worker.push(42).is_err() {
                n += 1;
            }
        }

        while worker.pop().is_some() {
            n += 1;
        }

        n += th1.join().unwrap();
        n += th2.join().unwrap();

        assert_eq!(n, NUM_TASKS);
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
    fn steal_tasks(stealer: Stealer<usize, st3::B4>) -> usize {
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

        let th1 = thread::spawn(move || steal_tasks(stealer1));
        let th2 = thread::spawn(move || steal_tasks(stealer2));

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
