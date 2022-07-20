use crate::loom_types::sync::atomic;

// If the target is 64-bit or the `long_counter` feature is selected (which is
// the default), use u64 as a long integer and u16 as short integer.
#[cfg(any(target_pointer_width = "64", feature = "long_counter"))]
pub(crate) type UnsignedShort = u32;
#[cfg(any(target_pointer_width = "64", feature = "long_counter"))]
pub(crate) type UnsignedLong = u64;
#[cfg(any(target_pointer_width = "64", feature = "long_counter"))]
pub(crate) type AtomicUnsignedShort = atomic::AtomicU32;
#[cfg(any(target_pointer_width = "64", feature = "long_counter"))]
pub(crate) type AtomicUnsignedLong = atomic::AtomicU64;

// Otherwise use u32 as long integer and u8 as short integer.
#[cfg(not(any(target_pointer_width = "64", feature = "long_counter")))]
pub(crate) type UnsignedShort = u16;
#[cfg(not(any(target_pointer_width = "64", feature = "long_counter")))]
pub(crate) type UnsignedLong = u32;
#[cfg(not(any(target_pointer_width = "64", feature = "long_counter")))]
pub(crate) type AtomicUnsignedShort = atomic::AtomicU16;
#[cfg(not(any(target_pointer_width = "64", feature = "long_counter")))]
pub(crate) type AtomicUnsignedLong = atomic::AtomicU32;
