use anyhow::{Result, bail};

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
    unsafe { std::ptr::read_unaligned(ptr.cast::<T>()) }
}

/// Trusted-path window advance: panics if `offset` exceeds the window length.
/// Only safe to use on same-process, self-produced buffers (in-proc queue
/// reads). Payload-derived (untrusted) offsets must use [`try_advance_window`].
pub fn advance_window(window: &[u8], offset: usize) -> &[u8] {
    assert!(offset <= window.len());
    &window[offset..]
}

/// Checked variant of [`advance_window`]: returns `Err` instead of panicking
/// when `offset` exceeds the window length. Use for payload- or
/// metadata-derived offsets, which must be treated as untrusted.
#[inline(always)]
pub fn try_advance_window(window: &[u8], offset: usize) -> Result<&[u8]> {
    if offset > window.len() {
        bail!(
            "truncated window: need {offset} bytes, have {}",
            window.len()
        );
    }
    Ok(&window[offset..])
}

/// Trusted-path pod read: panics (via `advance_window`'s assert) if the
/// window is shorter than `size_of::<T>()`. Only safe to use on same-process,
/// self-produced buffers (in-proc queue reads, e.g.
/// `InProcSerialize::read_value`). Payload-derived (untrusted) windows must
/// use [`try_read_consume_pod`].
pub fn read_consume_pod<T>(window: &mut &[u8]) -> T {
    let object_size = std::mem::size_of::<T>();
    let begin: *const u8 = window.as_ptr();
    *window = advance_window(window, object_size);
    unsafe { std::ptr::read_unaligned(begin.cast::<T>()) }
}

/// Checked variant of [`read_consume_pod`]: returns `Err` instead of
/// panicking when the window is shorter than `size_of::<T>()`. Use for
/// payload-derived (untrusted) windows.
#[allow(unsafe_code)]
#[inline(always)]
pub fn try_read_consume_pod<T>(window: &mut &[u8]) -> Result<T> {
    let object_size = std::mem::size_of::<T>();
    if object_size > window.len() {
        bail!(
            "truncated window reading {}: need {object_size} bytes, have {}",
            std::any::type_name::<T>(),
            window.len()
        );
    }
    let begin: *const u8 = window.as_ptr();
    *window = &window[object_size..];
    Ok(unsafe { std::ptr::read_unaligned(begin.cast::<T>()) })
}

/// Bounds-checked read of a POD value at `offset` within `window`. Replaces
/// `read_any(window.as_ptr().add(offset))` on untrusted windows — both
/// `offset` and `size_of::<T>()` may originate from untrusted stream
/// metadata, so their sum is validated with `checked_add` before the window
/// length check.
#[allow(unsafe_code)]
#[inline(always)]
pub fn try_read_pod_at<T>(window: &[u8], offset: usize) -> Result<T> {
    let object_size = std::mem::size_of::<T>();
    let end = match offset.checked_add(object_size) {
        Some(end) => end,
        None => bail!(
            "offset {offset} overflows reading {}",
            std::any::type_name::<T>()
        ),
    };
    if end > window.len() {
        bail!(
            "truncated window reading {} at offset {offset}: need {object_size} bytes, have {}",
            std::any::type_name::<T>(),
            window.len().saturating_sub(offset)
        );
    }
    Ok(unsafe { std::ptr::read_unaligned(window.as_ptr().add(offset).cast::<T>()) })
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
    unsafe fn read_value(mut window: &[u8]) -> Self {
        let res = read_consume_pod(&mut window);
        assert_eq!(window.len(), 0);
        res
    }
}
