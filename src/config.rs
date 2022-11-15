use crate::loom_exports::sync::atomic;

// If the target supports 64-bit atomics, use u64 as a long integer and u32 as
// short integer.
#[cfg(target_has_atomic = "64")]
pub(crate) type UnsignedShort = u32;
#[cfg(target_has_atomic = "64")]
pub(crate) type UnsignedLong = u64;
#[cfg(target_has_atomic = "64")]
pub(crate) type AtomicUnsignedShort = atomic::AtomicU32;
#[cfg(target_has_atomic = "64")]
pub(crate) type AtomicUnsignedLong = atomic::AtomicU64;

// Otherwise use u32 as long integer and u16 as short integer.
#[cfg(not(target_has_atomic = "64"))]
pub(crate) type UnsignedShort = u16;
#[cfg(not(target_has_atomic = "64"))]
pub(crate) type UnsignedLong = u32;
#[cfg(not(target_has_atomic = "64"))]
pub(crate) type AtomicUnsignedShort = atomic::AtomicU16;
#[cfg(not(target_has_atomic = "64"))]
pub(crate) type AtomicUnsignedLong = atomic::AtomicU32;
