//! Internal queue buffers of various sizes.

use alloc::boxed::Box;

use core::fmt::Debug;
use core::mem::MaybeUninit;

use crate::config::UnsignedShort;
use crate::loom_exports::cell::UnsafeCell;

/// Marker trait for fixed-size buffers.
pub trait Buffer<T>: private::Sealed {
    /// Buffer size.
    const CAPACITY: UnsignedShort;

    #[doc(hidden)]
    /// Buffer index bit mask.
    const MASK: UnsignedShort;

    #[doc(hidden)]
    /// Buffer data type.
    type Data: AsRef<[UnsafeCell<MaybeUninit<T>>]> + Debug;

    #[doc(hidden)]
    /// Returns an uninitialized buffer.
    fn allocate() -> Box<Self::Data>;
}

macro_rules! make_buffer {
    ($b:ident, $cap:expr) => {
        #[doc = concat!("Marker type for buffers of capacity ", $cap, ".")]
        #[derive(Copy, Clone, Debug)]
        pub struct $b {}

        impl private::Sealed for $b {}

        impl<T> Buffer<T> for $b {
            const CAPACITY: UnsignedShort = $cap;

            #[doc(hidden)]
            const MASK: UnsignedShort = $cap - 1;

            #[doc(hidden)]
            type Data = [UnsafeCell<MaybeUninit<T>>; $cap];

            #[doc(hidden)]
            #[cfg(not(st3_loom))]
            fn allocate() -> Box<Self::Data> {
                // Safety: initializing an array of `MaybeUninit` items with
                // `assume_init()` is valid, as per the `MaybeUninit` documentation.
                // Admittedly the situation is slightly different here: the buffer is
                // made of `MaybeUninit` elements wrapped in `UnsafeCell`s; however, the
                // latter is a `repr(transparent)` type with a trivial constructor, so
                // this should not make any difference.
                Box::new(unsafe { MaybeUninit::uninit().assume_init() })
            }
            #[doc(hidden)]
            #[cfg(st3_loom)]
            fn allocate() -> Box<Self::Data> {
                // Loom's `UnsafeCell` is not `repr(transparent)` and does not
                // have a trivial constructor so initialization must be done
                // element-wise.
                fn make_fixed_size<T>(buffer: Box<[T]>) -> Box<[T; $cap]> {
                    assert_eq!(buffer.len(), $cap);

                    // Safety: The length was checked.
                    unsafe { Box::from_raw(Box::into_raw(buffer).cast()) }
                }

                let mut buffer = Vec::with_capacity($cap);
                for _ in 0..$cap {
                    buffer.push(UnsafeCell::new(MaybeUninit::uninit()));
                }

                make_fixed_size(buffer.into_boxed_slice())
            }
        }
    };
}

// Define buffer capacities up to 2^15, which is the maximum that can be
// supported when the `long_counter` feature is disabled since buffer positions
// are then 16-bit wide (note that 1 bit is required for disambiguation between
// full and empty buffer). We could possibly define capacities up to 2^31 when
// `long_counter` is enabled, but this is probably enough in practice.
make_buffer!(B2, 2);
make_buffer!(B4, 4);
make_buffer!(B8, 8);
make_buffer!(B16, 16);
make_buffer!(B32, 32);
make_buffer!(B64, 64);
make_buffer!(B128, 128);
make_buffer!(B256, 256);
make_buffer!(B512, 512);
make_buffer!(B1024, 1024);
make_buffer!(B2048, 2048);
make_buffer!(B4096, 4096);
make_buffer!(B8192, 8192);
make_buffer!(B16384, 12384);
make_buffer!(B32768, 32768);

/// Prevent public implementation of Buffer.
mod private {
    pub trait Sealed {}
}
