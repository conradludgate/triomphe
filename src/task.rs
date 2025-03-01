use core::{
    marker::PhantomData,
    mem::ManuallyDrop,
    ops::Deref,
    ptr::NonNull,
    task::{RawWaker, RawWakerVTable, Waker},
};

use crate::{ArcItemBorrow, ThinArcItem};

pub trait ThinItemWake<H>: Sized {
    fn wake(this: ThinArcItem<H, Self>) {
        Self::wake_by_ref(&this);
    }

    fn wake_by_ref(this: &ThinArcItem<H, Self>);
}

impl<H: 'static, W: ThinItemWake<H> + Send + Sync + 'static> From<ThinArcItem<H, W>> for Waker {
    /// Use a [`Wake`]-able type as a `Waker`.
    ///
    /// No heap allocations or atomic operations are used for this conversion.
    fn from(waker: ThinArcItem<H, W>) -> Waker {
        // SAFETY: This is safe because raw_waker safely constructs
        // a RawWaker from ThinArcItem<H, W>.
        unsafe { Waker::from_raw(RawWaker::from(waker)) }
    }
}

impl<H: 'static, W: ThinItemWake<H> + Send + Sync + 'static> From<ThinArcItem<H, W>> for RawWaker {
    /// Use a `Wake`-able type as a `RawWaker`.
    ///
    /// No heap allocations or atomic operations are used for this conversion.
    fn from(waker: ThinArcItem<H, W>) -> RawWaker {
        RawWaker::new(waker.into_raw().as_ptr().cast(), waker_vtable::<H, W>())
    }
}

impl<'a, H: 'static, W: ThinItemWake<H> + Send + Sync + 'static> From<&'a ThinArcItem<H, W>>
    for WakerRef<'a>
{
    fn from(waker: &'a ThinArcItem<H, W>) -> WakerRef<'a> {
        Self::from(waker.borrow_arc())
    }
}

impl<'a, H: 'static, W: ThinItemWake<H> + Send + Sync + 'static> From<ArcItemBorrow<'a, H, W>>
    for WakerRef<'a>
{
    fn from(waker: ArcItemBorrow<'a, H, W>) -> WakerRef<'a> {
        let waker = ManuallyDrop::new(unsafe {
            Waker::from_raw(RawWaker::new(
                waker.0.as_ptr().cast(),
                waker_vtable::<H, W>(),
            ))
        });
        WakerRef::new_unowned(waker)
    }
}

fn waker_vtable<H, W: ThinItemWake<H> + Send + Sync + 'static>() -> &'static RawWakerVTable {
    // Increment the reference count of the arc to clone it.
    unsafe fn clone_waker<H, W: ThinItemWake<H> + Send + Sync + 'static>(
        waker: *const (),
    ) -> RawWaker {
        let waker_ptr = unsafe { NonNull::new_unchecked(waker.cast_mut().cast()) };
        let waker_ref = unsafe { ThinArcItem::<H, W>::from_raw_ref(waker_ptr) };
        let waker = waker_ref.clone_arc().into_raw();

        RawWaker::new(waker.as_ptr().cast(), waker_vtable::<H, W>())
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
        waker_ref.with_arc(|a| <W as ThinItemWake<H>>::wake_by_ref(a))
    }

    // Decrement the reference count of the Arc on drop
    unsafe fn drop_waker<H, W: ThinItemWake<H> + Send + Sync + 'static>(waker: *const ()) {
        let waker_ptr = unsafe { NonNull::new_unchecked(waker.cast_mut().cast()) };
        let _ = unsafe { ThinArcItem::<H, W>::from_raw(waker_ptr) };
    }

    &RawWakerVTable::new(
        clone_waker::<H, W>,
        wake::<H, W>,
        wake_by_ref::<H, W>,
        drop_waker::<H, W>,
    )
}

/// A [`Waker`] that is only valid for a given lifetime.
///
/// Note: this type implements [`Deref<Target = Waker>`](std::ops::Deref),
/// so it can be used to get a `&Waker`.
#[derive(Debug)]
pub struct WakerRef<'a> {
    waker: ManuallyDrop<Waker>,
    _marker: PhantomData<&'a ()>,
}

impl<'a> WakerRef<'a> {
    /// Create a new [`WakerRef`] from a [`Waker`] reference.
    #[inline]
    pub fn new(waker: &'a Waker) -> Self {
        // copy the underlying (raw) waker without calling a clone,
        // as we won't call Waker::drop either.
        let waker = ManuallyDrop::new(unsafe { core::ptr::read(waker) });
        Self {
            waker,
            _marker: PhantomData,
        }
    }

    /// Create a new [`WakerRef`] from a [`Waker`] that must not be dropped.
    ///
    /// Note: this if for rare cases where the caller created a [`Waker`] in
    /// an unsafe way (that will be valid only for a lifetime to be determined
    /// by the caller), and the [`Waker`] doesn't need to or must not be
    /// destroyed.
    #[inline]
    pub fn new_unowned(waker: ManuallyDrop<Waker>) -> Self {
        Self {
            waker,
            _marker: PhantomData,
        }
    }
}

impl Deref for WakerRef<'_> {
    type Target = Waker;

    #[inline]
    fn deref(&self) -> &Waker {
        &self.waker
    }
}
