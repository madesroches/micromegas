//! Values that the logging and metric macros accept as bare `key = value` entries.
use std::{borrow::Cow, rc::Rc, sync::Arc};

/// A value that can be recorded as a property without being formatted first.
///
/// Strings borrow, so a bare value costs no allocation. Use `%value` (`Display`) or `?value`
/// (`Debug`) for anything else. The trait is deliberately owned by this crate: integer and
/// bool impls can be added later without breaking callers, since downstream crates can't
/// implement it for foreign types.
pub trait PropertyValue {
    fn property_value(&self) -> Cow<'_, str>;
}

impl PropertyValue for str {
    fn property_value(&self) -> Cow<'_, str> {
        Cow::Borrowed(self)
    }
}

impl PropertyValue for String {
    fn property_value(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.as_str())
    }
}

impl PropertyValue for Cow<'_, str> {
    fn property_value(&self) -> Cow<'_, str> {
        Cow::Borrowed(&**self)
    }
}

impl<T: PropertyValue + ?Sized> PropertyValue for &T {
    fn property_value(&self) -> Cow<'_, str> {
        (**self).property_value()
    }
}

impl<T: PropertyValue + ?Sized> PropertyValue for Box<T> {
    fn property_value(&self) -> Cow<'_, str> {
        (**self).property_value()
    }
}

impl<T: PropertyValue + ?Sized> PropertyValue for Arc<T> {
    fn property_value(&self) -> Cow<'_, str> {
        (**self).property_value()
    }
}

impl<T: PropertyValue + ?Sized> PropertyValue for Rc<T> {
    fn property_value(&self) -> Cow<'_, str> {
        (**self).property_value()
    }
}
