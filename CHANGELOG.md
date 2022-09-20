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
