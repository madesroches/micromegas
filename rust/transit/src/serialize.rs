#[allow(unsafe_code)]
#[inline(always)]
pub fn write_any<T>(buffer: &mut Vec<u8>, value: &T) {
    let ptr = std::ptr::addr_of!(*value).cast::<u8>();
    let slice = std::ptr::slice_from_raw_parts(ptr, std::mem::size_of::<T>());
    unsafe {
        buffer.extend_from_slice(&*slice);
    }
}

#[allow(unsafe_code)]
/// Helper function to read a u* pointer to a value of type T.
///
/// # Safety
/// ptr must be valid it's size and it's memory size must be the size
/// of T or higher.
#[inline(always)]
pub unsafe fn read_any<T>(ptr: *const u8) -> T {
    std::ptr::read_unaligned(ptr.cast::<T>())
}

pub unsafe fn advance_window(window: &[u8], offset: usize) -> &[u8] {
    assert!(offset <= window.len());
    &window[offset..]
}

pub unsafe fn read_advance_any<T>(window: &mut &[u8]) -> T {
    let object_size = std::mem::size_of::<T>();
    let begin: *const u8 = window.as_ptr();
    *window = advance_window(window, object_size);
    std::ptr::read_unaligned(begin.cast::<T>())
}

/// Helps speed up the serialization of types which size is known at compile time.
pub enum InProcSize {
    Const(usize),
    Dynamic,
}

// InProcSerialize is used by the heterogeneous queue to write objects in its
// buffer serialized objects can have references with static lifetimes
pub trait InProcSerialize: Sized {
    const IN_PROC_SIZE: InProcSize = InProcSize::Const(std::mem::size_of::<Self>());

    fn get_value_size(&self) -> Option<u32> {
        // for POD serialization we don't write the size of each instance
        // the metadata will contain this size
        None
    }

    #[inline(always)]
    fn write_value(&self, buffer: &mut Vec<u8>) {
        assert!(matches!(Self::IN_PROC_SIZE, InProcSize::Const(_)));
        #[allow(clippy::needless_borrow)]
        //clippy complains here but we don't want to move or copy the value
        write_any::<Self>(buffer, &self);
    }

    // read_value allows to read objects from the same process they were stored in
    // i.e. iterating in the heterogenous queue
    /// # Safety
    /// This is called from the serializer context that that uses `value_size`
    /// call to make sure that the proper size is used
    #[allow(unsafe_code)]
    #[inline(always)]
    unsafe fn read_value(ptr: *const u8, _value_size: Option<u32>) -> Self {
        read_any::<Self>(ptr)
    }
}
