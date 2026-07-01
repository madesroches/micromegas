use anyhow::Result;
use micromegas_transit::value::{Object, Value};

/// A set of properties, backed by an arena-allocated `transit` object.
///
/// Borrows the parse arena, so it is valid only within a single `parse_block`
/// call; consumers must serialize/copy it out before the arena is dropped.
#[derive(Debug, Clone, Copy)]
pub struct PropertySet<'a> {
    obj: &'a Object<'a>,
}

impl<'a> PropertySet<'a> {
    pub fn new(obj: &'a Object<'a>) -> Self {
        Self { obj }
    }

    pub fn empty() -> PropertySet<'static> {
        static EMPTY: Object<'static> = Object {
            type_name: "EmptyPropertySet",
            members: &[],
        };
        PropertySet { obj: &EMPTY }
    }

    /// Iterates over the string-valued properties in the set as `(key, value)` pairs.
    pub fn for_each_property<Fun: FnMut(&'a str, &'a str) -> Result<()>>(
        &self,
        mut fun: Fun,
    ) -> Result<()> {
        for &(key, value) in self.obj.members {
            if let Value::String(value_str) = value {
                fun(key, value_str)?;
            }
        }
        Ok(())
    }

    /// Stable identity pointer to the underlying arena object.
    ///
    /// Used by the dictionary builder for pointer-based deduplication: within a
    /// single block, every event referencing the same property-set dependency
    /// shares one arena `Object`, so identical sets compare equal by address.
    /// The pointer is only ever compared, never dereferenced.
    pub fn object_ptr(&self) -> *const () {
        self.obj as *const Object as *const ()
    }
}

impl<'a> From<&'a Object<'a>> for PropertySet<'a> {
    fn from(value: &'a Object<'a>) -> Self {
        Self::new(value)
    }
}
