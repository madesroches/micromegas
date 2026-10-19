//! Values that the logging and metric macros accept as bare `key = value` entries.
use std::{borrow::Cow, rc::Rc, sync::Arc};

mod sealed {
    pub trait Sealed {}
    impl Sealed for str {}
    impl Sealed for String {}
    impl Sealed for std::borrow::Cow<'_, str> {}
    impl<T: Sealed + ?Sized> Sealed for &T {}
    impl<T: Sealed + ?Sized> Sealed for Box<T> {}
    impl<T: Sealed + ?Sized> Sealed for std::sync::Arc<T> {}
    impl<T: Sealed + ?Sized> Sealed for std::rc::Rc<T> {}
}

/// A value that can be recorded as a property without being formatted first.
///
/// Strings borrow, so a bare value costs no allocation. Use `%value` (`Display`) or `?value`
/// (`Debug`) for anything else. The trait is sealed: a deferred log message is rebuilt from
/// `property_value()` while the local sink prints the value with `Display`, so every impl must
/// produce the same text for both.
pub trait PropertyValue: sealed::Sealed {
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
