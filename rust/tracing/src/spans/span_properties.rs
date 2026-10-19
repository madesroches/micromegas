//! Properties a span carries until its end event.
use crate::property_set::{DynProperty, PropertySet};

#[cfg(not(target_arch = "wasm32"))]
use smallvec::SmallVec;
#[cfg(not(target_arch = "wasm32"))]
use std::fmt::{self, Write as _};

/// Properties a span carries until its end event. Values are copied in, so they only need to
/// outlive the call that sets them.
///
/// Names stay `'static` because the number of distinct names must stay bounded; put unbounded
/// data in the values.
#[cfg(not(target_arch = "wasm32"))]
pub struct SpanProperties {
    static_set: &'static PropertySet,
    // name, then offset and length of the value in `values`
    entries: SmallVec<[(&'static str, u32, u32); 8]>,
    values: SmallVec<[u8; 128]>,
}

#[cfg(not(target_arch = "wasm32"))]
struct ValueWriter<'a>(&'a mut SmallVec<[u8; 128]>);

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Write for ValueWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl SpanProperties {
    pub fn new(static_set: &'static PropertySet) -> Self {
        Self {
            static_set,
            entries: SmallVec::new(),
            values: SmallVec::new(),
        }
    }

    /// Sets a property, replacing the value of a property of the same name (compared
    /// case-insensitively, like `property_get`). It beats a literal of the same name given when
    /// the span was created.
    ///
    /// ```
    /// use micromegas_tracing::prelude::*;
    ///
    /// # fn main() {
    /// let mut span = span_scope!(name = "fetch_rows", region = "eu");
    /// span.set_property("rows", 42);
    /// span.set_property("cache", if true { "hit" } else { "miss" });
    /// # }
    /// ```
    pub fn set_property(&mut self, name: &'static str, value: impl fmt::Display) {
        if let Some(i) = self
            .entries
            .iter()
            .position(|e| e.0.eq_ignore_ascii_case(name))
        {
            let (_, offset, len) = self.entries.remove(i);
            // Compacted so that a property set in a loop doesn't grow the buffer.
            self.values.drain(offset as usize..(offset + len) as usize);
            for entry in self.entries.iter_mut().filter(|e| e.1 > offset) {
                entry.1 -= len;
            }
        }
        let start = self.values.len();
        // `ValueWriter` never fails; a `Display` that errs leaves its partial output.
        let _ = write!(ValueWriter(&mut self.values), "{value}");
        let len = self.values.len() - start;
        self.entries.push((name, start as u32, len as u32));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.static_set.get_properties().is_empty()
    }

    /// Bytes held by the values set at run time.
    #[doc(hidden)]
    pub fn value_bytes(&self) -> usize {
        self.values.len()
    }

    /// Hands the static set and the entries set at run time to `f`, as the tagged end functions
    /// of the dispatch take them.
    #[doc(hidden)]
    pub fn with_dyn<R>(&self, f: impl FnOnce(&'static PropertySet, &[DynProperty<'_>]) -> R) -> R {
        let dyn_properties: SmallVec<[DynProperty<'_>; 8]> = self
            .entries
            .iter()
            .map(|&(name, offset, len)| {
                let bytes = &self.values[offset as usize..(offset + len) as usize];
                // SAFETY: the bytes were written by `fmt::Write::write_str` from `&str`s, and
                // entries are removed whole.
                DynProperty::new(name, unsafe { std::str::from_utf8_unchecked(bytes) })
            })
            .collect();
        f(self.static_set, &dyn_properties)
    }
}

/// Properties are not recorded on wasm32.
#[cfg(target_arch = "wasm32")]
pub struct SpanProperties {
    static_set: &'static PropertySet,
}

#[cfg(target_arch = "wasm32")]
impl SpanProperties {
    pub fn new(static_set: &'static PropertySet) -> Self {
        Self { static_set }
    }

    pub fn set_property(&mut self, _name: &'static str, _value: impl std::fmt::Display) {}

    pub(crate) fn is_empty(&self) -> bool {
        true
    }

    #[doc(hidden)]
    pub fn with_dyn<R>(&self, f: impl FnOnce(&'static PropertySet, &[DynProperty<'_>]) -> R) -> R {
        f(self.static_set, &[])
    }

    #[doc(hidden)]
    pub fn value_bytes(&self) -> usize {
        0
    }
}
