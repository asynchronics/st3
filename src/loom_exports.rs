#[cfg(st3_loom)]
#[allow(unused_imports)]
pub(crate) mod sync {
    pub(crate) mod atomic {
        #[cfg(not(target_has_atomic = "64"))]
        pub(crate) use loom::sync::atomic::AtomicU16;
        pub(crate) use loom::sync::atomic::AtomicU32;
        #[cfg(target_has_atomic = "64")]
        pub(crate) use loom::sync::atomic::AtomicU64;
    }
}
#[cfg(not(st3_loom))]
#[allow(unused_imports)]
pub(crate) mod sync {
    pub(crate) mod atomic {
        #[cfg(not(target_has_atomic = "64"))]
        pub(crate) use core::sync::atomic::AtomicU16;
        pub(crate) use core::sync::atomic::AtomicU32;
        #[cfg(target_has_atomic = "64")]
        pub(crate) use core::sync::atomic::AtomicU64;
    }
}

#[cfg(st3_loom)]
pub(crate) mod cell {
    pub(crate) use loom::cell::UnsafeCell;
}
#[cfg(not(st3_loom))]
pub(crate) mod cell {
    #[derive(Debug)]
    pub(crate) struct UnsafeCell<T>(core::cell::UnsafeCell<T>);

    #[allow(dead_code)]
    impl<T> UnsafeCell<T> {
        pub(crate) fn new(data: T) -> UnsafeCell<T> {
            UnsafeCell(core::cell::UnsafeCell::new(data))
        }
        pub(crate) fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
            f(self.0.get())
        }
        pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
            f(self.0.get())
        }
    }
}

#[allow(unused_macros)]
macro_rules! debug_or_loom_assert {
    ($($arg:tt)*) => (if cfg!(any(debug_assertions, st3_loom)) { assert!($($arg)*); })
}
#[allow(unused_macros)]
macro_rules! debug_or_loom_assert_eq {
    ($($arg:tt)*) => (if cfg!(any(debug_assertions, st3_loom)) { assert_eq!($($arg)*); })
}
#[allow(unused_imports)]
pub(crate) use debug_or_loom_assert;
#[allow(unused_imports)]
pub(crate) use debug_or_loom_assert_eq;
