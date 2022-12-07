# 0.4.1 (2022-12-07)

- Make it possible to obtain a reference to a stealer from a worker with
  `stealer_ref` ([#5]).
- Implement `Eq` and `PartialEq` on stealers ([#5]).
- Replace the soon-to-be-deprecated `cache-padded` crate with `crossbeam-utils` ([#4])

[#4]: https://github.com/asynchronics/st3/pull/4
[#5]: https://github.com/asynchronics/st3/pull/5

# 0.4.0 (2022-11-15)

- Revert the fix added in 0.3.1 as the code was actually correct and the fix was
  unnecessary (comments added).
- Add a FIFO queue variant in the `fifo` module with the same API, based on the
  Tokio queue.

## :warning: Breaking changes

- Make the queue capacity a run-time parameter,
- Move the LIFO queue to a `lifo` module.
- Choose buffer indexing integer size based on `target_has_atomic` and remove
  the `long_counter` feature.
- Bump the MSRV to 1.60 to enable the use of `target_has_atomic`.

# 0.3.1 (2022-09-20)

- Fix bug that could result in an underflow when estimating the capacity of a
  stealer.

# 0.3.0 (2022-09-05)

- Implement FusedIterator for the Drain iterator.
- Add a `Worker::spare_capacity` method.
- BREAKING CHANGE: remove `Worker::len` as it was not very useful due to its
  weak guaranties.
- BREAKING CHANGE: move the `drain` method to the worker rather than the
  stealer, as it is mostly useful to move items back into an injection queue on
  overflow.

# 0.2.0 (2022-07-20)

- Add a drain iterator to efficiently steal batches of items into custom
  containers.

# 0.1.1 (2022-02-11)

- Mitigate other potential ABA problems by using 32-bit positions throughout.
- Update documentation.
- Inline MIT license in `tokio_queue.rs` and explicitly state license exceptions
  for tests/benchmark in README.
- Update copyright date in MIT license.

# 0.1.0 (2022-02-10)

Initial release
