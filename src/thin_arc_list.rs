use core::{
    marker::PhantomData,
    mem::{self, offset_of, ManuallyDrop},
    ops::Deref,
    ptr::{self, NonNull},
};

use super::{Arc, ArcInner, HeaderSlice, HeaderWithLength, ThinArc};

#[derive(Clone, Copy)]
pub struct WithOffset<T> {
    /// the offset in the `ThinArcList` this value is stored
    offset: usize,
    /// the value stored
    pub value: T,
}

#[repr(transparent)]
pub struct ThinArcItem<H, T> {
    ptr: ptr::NonNull<WithOffset<T>>,
    phantom: PhantomData<ThinArcList<H, T>>,
}

impl<H, T> Deref for ThinArcItem<H, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &self.ptr.as_ref().value }
    }
}

unsafe impl<H: Sync + Send, T: Sync + Send> Send for ThinArcItem<H, T> {}
unsafe impl<H: Sync + Send, T: Sync + Send> Sync for ThinArcItem<H, T> {}

#[repr(transparent)]
pub struct ThinArcList<H, T> {
    inner: ThinArc<H, WithOffset<T>>,
}

impl<H, T> ThinArcList<H, T> {
    /// Creates a `ThinArc` for a HeaderSlice using the given header struct and
    /// iterator to generate the slice.
    pub fn header_from_iter<I, F>(items: I, f: F) -> Self
    where
        I: Iterator<Item = T> + ExactSizeIterator,
        F: FnOnce(&mut [WithOffset<T>]) -> H,
    {
        Self {
            inner: ThinArc::header_from_iter(
                items
                    .enumerate()
                    .map(|(offset, value)| WithOffset { offset, value }),
                f,
            ),
        }
    }

    /// Creates a `ThinArcList` for a HeaderSlice using the given header struct and
    /// iterator to generate the slice.
    pub fn from_header_and_iter<I>(header: H, items: I) -> Self
    where
        I: Iterator<Item = T> + ExactSizeIterator,
    {
        Self {
            inner: ThinArc::from_header_and_iter(
                header,
                items
                    .enumerate()
                    .map(|(offset, value)| WithOffset { offset, value }),
            ),
        }
    }

    pub fn header(&self) -> &H {
        &self.inner.header.header
    }

    pub fn with_item<F, U>(&self, index: usize, f: F) -> U
    where
        F: FnOnce(&ThinArcItem<H, T>) -> U,
    {
        // very fiddly :(
        let len = unsafe { (*self.inner.ptr.as_ptr()).data.header.length };
        assert!(index < len);
        let slice =
            unsafe { ptr::addr_of!((*self.inner.ptr.as_ptr()).data.slice).cast::<WithOffset<T>>() };

        let transient = ManuallyDrop::new(ThinArcItem {
            ptr: unsafe { NonNull::new_unchecked(slice.add(index).cast_mut()) },
            phantom: PhantomData,
        });
        f(&transient)
    }
}

impl<H, T> ThinArcItem<H, T> {
    pub fn into_raw(self) -> NonNull<WithOffset<T>> {
        ManuallyDrop::new(self).ptr
    }

    /// # Safety
    /// * Must come from as_ref, must have the same H.
    pub unsafe fn from_raw(ptr: NonNull<WithOffset<T>>) -> Self {
        Self {
            ptr,
            phantom: PhantomData,
        }
    }

    pub fn as_raw(&self) -> NonNull<WithOffset<T>> {
        self.ptr
    }

    /// # Safety
    /// * Must come from as_ref, must have the same H.
    /// * Must not be dropped.
    pub unsafe fn from_raw_ref(ptr: NonNull<WithOffset<T>>) -> ManuallyDrop<Self> {
        ManuallyDrop::new(Self {
            ptr,
            phantom: PhantomData,
        })
    }

    pub fn index(&self) -> usize {
        unsafe { self.ptr.as_ref().offset }
    }

    pub fn with_parent<F, U>(&self, f: F) -> U
    where
        F: FnOnce(&ThinArcList<H, T>) -> U,
    {
        unsafe {
            let transient = ManuallyDrop::new(self.ref_into_parent());
            f(&transient)
        }
    }

    unsafe fn ref_into_parent(&self) -> ThinArcList<H, T> {
        let value_ptr = self.ptr.as_ptr().cast_const();
        let offset = unsafe { (*value_ptr).offset };

        // inner.data.slice[offset] -> inner.data.slice[0]
        let slice_root = unsafe { value_ptr.sub(offset) };

        let slice_offset = offset_of!(
            ArcInner<HeaderSlice<HeaderWithLength<H>, [WithOffset<T>; 0]>>,
            data.slice
        );

        // inner.data.slice[0] -> inner
        let arc = unsafe {
            slice_root
                .byte_sub(slice_offset)
                .cast::<ArcInner<HeaderSlice<HeaderWithLength<H>, [WithOffset<T>; 0]>>>()
        };

        // inner -> inner.data.header.length
        let len = unsafe { *ptr::addr_of!((*arc).data.header.length) };

        // Synthesize the fat pointer. We do this by claiming we have a direct
        // pointer to a [T], and then changing the type of the borrow. The key
        // point here is that the length portion of the fat pointer applies
        // only to the number of elements in the dynamically-sized portion of
        // the type, so the value will be the same whether it points to a [T]
        // or something else with a [T] as its last member.
        let fake_slice = ptr::slice_from_raw_parts_mut(arc as *mut WithOffset<T>, len);
        let arc_slice =
            fake_slice as *mut ArcInner<HeaderSlice<HeaderWithLength<H>, [WithOffset<T>]>>;

        unsafe {
            ThinArcList {
                inner: Arc::into_thin(Arc::from_raw_inner(arc_slice)),
            }
        }
    }
}

impl<H, T> Clone for ThinArcList<H, T> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<H, T> Clone for ThinArcItem<H, T> {
    #[inline]
    fn clone(&self) -> Self {
        mem::forget(self.with_parent(ThinArcList::clone));
        Self {
            ptr: self.ptr,
            phantom: self.phantom,
        }
    }
}

impl<H, T> Drop for ThinArcItem<H, T> {
    #[inline]
    fn drop(&mut self) {
        let _ = unsafe { self.ref_into_parent() };
    }
}
