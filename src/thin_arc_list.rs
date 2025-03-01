use core::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem::{self, offset_of, ManuallyDrop},
    ops::Deref,
    ptr::{self, NonNull},
};

use crate::{ArcItemBorrow, HeaderSliceWithLengthUnchecked};

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

    /// Temporarily converts |self| into a bonafide Arc and exposes it to the
    /// provided callback. The refcount is not modified.
    #[inline]
    pub fn with_arc<F, U>(&self, f: F) -> U
    where
        F: FnOnce(&Arc<HeaderSliceWithLengthUnchecked<H, WithOffset<T>>>) -> U,
    {
        self.inner.with_arc(f)
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
    pub unsafe fn from_raw_ref<'a>(ptr: NonNull<WithOffset<T>>) -> ArcItemBorrow<'a, H, T> {
        ArcItemBorrow(ptr, PhantomData)
    }

    /// Produce a pointer to the data that can be converted back
    /// to an ThinArcItem. This is basically an `&ThinArcItem<T>`, without the extra indirection.
    /// It has the benefits of an `&T` but also knows about the underlying refcount
    /// and can be converted into more `ThinArcItem<T>`s if necessary.
    #[inline]
    pub fn borrow_arc(&self) -> ArcItemBorrow<'_, H, T> {
        ArcItemBorrow(self.ptr, PhantomData)
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

impl<H, T: PartialEq> PartialEq for ThinArcItem<H, T> {
    fn eq(&self, other: &ThinArcItem<H, T>) -> bool {
        // TODO: pointer equality is incorrect if `T` is not `Eq`.
        self.ptr == other.ptr || *(*self) == *(*other)
    }

    #[allow(clippy::partialeq_ne_impl)]
    fn ne(&self, other: &ThinArcItem<H, T>) -> bool {
        self.ptr != other.ptr && *(*self) != *(*other)
    }
}

impl<H, T: PartialOrd> PartialOrd for ThinArcItem<H, T> {
    fn partial_cmp(&self, other: &ThinArcItem<H, T>) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }

    fn lt(&self, other: &ThinArcItem<H, T>) -> bool {
        *(*self) < *(*other)
    }

    fn le(&self, other: &ThinArcItem<H, T>) -> bool {
        *(*self) <= *(*other)
    }

    fn gt(&self, other: &ThinArcItem<H, T>) -> bool {
        *(*self) > *(*other)
    }

    fn ge(&self, other: &ThinArcItem<H, T>) -> bool {
        *(*self) >= *(*other)
    }
}

impl<H, T: Ord> Ord for ThinArcItem<H, T> {
    fn cmp(&self, other: &ThinArcItem<H, T>) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<H, T: Eq> Eq for ThinArcItem<H, T> {}

impl<H, T: fmt::Display> fmt::Display for ThinArcItem<H, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<H, T: fmt::Debug> fmt::Debug for ThinArcItem<H, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<H, T> fmt::Pointer for ThinArcItem<H, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&self.ptr, f)
    }
}

impl<H, T: Hash> Hash for ThinArcItem<H, T> {
    fn hash<Ha: Hasher>(&self, state: &mut Ha) {
        (**self).hash(state)
    }
}
