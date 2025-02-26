use core::task::{RawWaker, Waker};

use crate::ThinArcItem;

pub trait ThinItemWake<H>: Sized {
    fn wake(this: ThinArcItem<H, Self>) {
        Self::wake_by_ref(&this);
    }

    fn wake_by_ref(this: &ThinArcItem<H, Self>);
}

#[cfg(target_has_atomic = "ptr")]
impl<H, W: ThinItemWake<H> + Send + Sync + 'static> From<ThinArcItem<H, W>> for Waker {
    /// Use a [`Wake`]-able type as a `Waker`.
    ///
    /// No heap allocations or atomic operations are used for this conversion.
    fn from(waker: ThinArcItem<H, W>) -> Waker {
        // SAFETY: This is safe because raw_waker safely constructs
        // a RawWaker from ThinArcItem<H, W>.
        unsafe { Waker::from_raw(raw_waker(waker)) }
    }
}

#[cfg(target_has_atomic = "ptr")]
impl<H, W: ThinItemWake<H> + Send + Sync + 'static> From<ThinArcItem<H, W>> for RawWaker {
    /// Use a `Wake`-able type as a `RawWaker`.
    ///
    /// No heap allocations or atomic operations are used for this conversion.
    fn from(waker: ThinArcItem<H, W>) -> RawWaker {
        raw_waker(waker)
    }
}

// NB: This private function for constructing a RawWaker is used, rather than
// inlining this into the `From<ThinArcItem<H, W>> for RawWaker` impl, to ensure that
// the safety of `From<ThinArcItem<H, W>> for Waker` does not depend on the correct
// trait dispatch - instead both impls call this function directly and
// explicitly.
#[cfg(target_has_atomic = "ptr")]
#[inline(always)]
fn raw_waker<H, W: ThinItemWake<H> + Send + Sync + 'static>(waker: ThinArcItem<H, W>) -> RawWaker {
    use core::{
        mem::ManuallyDrop,
        ptr::NonNull,
        task::{RawWaker, RawWakerVTable},
    };

    fn vtable<H, W: ThinItemWake<H> + Send + Sync + 'static>() -> &'static RawWakerVTable {
        &RawWakerVTable::new(
            clone_waker::<H, W>,
            wake::<H, W>,
            wake_by_ref::<H, W>,
            drop_waker::<H, W>,
        )
    }

    // Increment the reference count of the arc to clone it.
    unsafe fn clone_waker<H, W: ThinItemWake<H> + Send + Sync + 'static>(
        waker: *const (),
    ) -> RawWaker {
        let waker_ptr = unsafe { NonNull::new_unchecked(waker.cast_mut().cast()) };
        let waker_ref = unsafe { ThinArcItem::<H, W>::from_raw_ref(waker_ptr) };
        let _clone = ManuallyDrop::new(waker_ref.clone());

        RawWaker::new(waker, vtable::<H, W>())
    }

    // Wake by value, moving the Arc into the Wake::wake function
    unsafe fn wake<H, W: ThinItemWake<H> + Send + Sync + 'static>(waker: *const ()) {
        let waker_ptr = unsafe { NonNull::new_unchecked(waker.cast_mut().cast()) };
        let waker = unsafe { ThinArcItem::from_raw(waker_ptr) };
        <W as ThinItemWake<H>>::wake(waker);
    }

    // Wake by reference, wrap the waker in ManuallyDrop to avoid dropping it
    unsafe fn wake_by_ref<H, W: ThinItemWake<H> + Send + Sync + 'static>(waker: *const ()) {
        let waker_ptr = unsafe { NonNull::new_unchecked(waker.cast_mut().cast()) };
        let waker_ref = unsafe { ThinArcItem::<H, W>::from_raw_ref(waker_ptr) };
        <W as ThinItemWake<H>>::wake_by_ref(&waker_ref);
    }

    // Decrement the reference count of the Arc on drop
    unsafe fn drop_waker<H, W: ThinItemWake<H> + Send + Sync + 'static>(waker: *const ()) {
        let waker_ptr = unsafe { NonNull::new_unchecked(waker.cast_mut().cast()) };
        let _ = unsafe { ThinArcItem::<H, W>::from_raw(waker_ptr) };
    }

    RawWaker::new(waker.into_raw().as_ptr().cast(), vtable::<H, W>())
}
