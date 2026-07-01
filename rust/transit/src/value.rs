use anyhow::{Result, bail};

/// A parsed object: a type name and a slice of named members.
///
/// All fields borrow from the parse arena (or the source buffer / stream
/// metadata), so `Object` is `Copy` and carries no `Drop` glue. This is what
/// lets it be bump-allocated safely.
#[derive(Debug, Clone, Copy)]
pub struct Object<'a> {
    pub type_name: &'a str,
    pub members: &'a [(&'a str, Value<'a>)],
}

impl<'a> Object<'a> {
    pub fn get<T>(&self, member_name: &str) -> Result<T>
    where
        T: TransitValue<'a>,
    {
        for m in self.members {
            if m.0 == member_name {
                return T::get(m.1);
            }
        }
        bail!("member {} not found in {:?}", member_name, self);
    }

    pub fn get_ref(&self, member_name: &str) -> Result<&'a Value<'a>> {
        for m in self.members {
            if m.0 == member_name {
                return Ok(&m.1);
            }
        }
        bail!("member {} not found", member_name);
    }
}

pub trait TransitValue<'a>: Sized {
    fn get(value: Value<'a>) -> Result<Self>;
}

impl<'a> TransitValue<'a> for u8 {
    fn get(value: Value<'a>) -> Result<Self> {
        if let Value::U8(val) = value {
            Ok(val)
        } else {
            bail!("bad type cast u8 for value {:?}", value);
        }
    }
}

impl<'a> TransitValue<'a> for u32 {
    fn get(value: Value<'a>) -> Result<Self> {
        match value {
            Value::U32(val) => Ok(val),
            Value::U8(val) => Ok(Self::from(val)),
            _ => {
                bail!("bad type cast u32 for value {:?}", value);
            }
        }
    }
}

impl<'a> TransitValue<'a> for u64 {
    fn get(value: Value<'a>) -> Result<Self> {
        match value {
            Value::I64(val) => Ok(val as Self),
            Value::U64(val) => Ok(val),
            _ => {
                bail!("bad type cast u64 for value {:?}", value)
            }
        }
    }
}

impl<'a> TransitValue<'a> for i64 {
    #[allow(clippy::cast_possible_wrap)]
    fn get(value: Value<'a>) -> Result<Self> {
        match value {
            Value::I64(val) => Ok(val),
            Value::U64(val) => Ok(val as Self),
            _ => {
                bail!("bad type cast i64 for value {:?}", value)
            }
        }
    }
}

impl<'a> TransitValue<'a> for f64 {
    fn get(value: Value<'a>) -> Result<Self> {
        if let Value::F64(val) = value {
            Ok(val)
        } else {
            bail!("bad type cast f64 for value {:?}", value);
        }
    }
}

impl<'a> TransitValue<'a> for &'a str {
    fn get(value: Value<'a>) -> Result<Self> {
        if let Value::String(val) = value {
            Ok(val)
        } else {
            bail!("bad type cast str for value {:?}", value);
        }
    }
}

impl<'a> TransitValue<'a> for &'a Object<'a> {
    fn get(value: Value<'a>) -> Result<Self> {
        if let Value::Object(val) = value {
            Ok(val)
        } else {
            bail!("bad type cast Object for value {:?}", value);
        }
    }
}

impl<'a> TransitValue<'a> for &'a [u8] {
    fn get(value: Value<'a>) -> Result<Self> {
        if let Value::Bytes(val) = value {
            Ok(val)
        } else {
            bail!("bad type cast bytes for value {:?}", value);
        }
    }
}

/// A schemaless runtime value parsed from a transit buffer.
///
/// Every variant is a primitive or a shared borrow, so `Value` is `Copy` and
/// `Drop`-free; it can be stored in a bump arena and discarded by resetting the
/// arena rather than by running destructors.
#[derive(Debug, Clone, Copy)]
pub enum Value<'a> {
    Bytes(&'a [u8]),
    F64(f64),
    I64(i64),
    None,
    Object(&'a Object<'a>),
    String(&'a str),
    U8(u8),
    U32(u32),
    U64(u64),
}

impl<'a> Value<'a> {
    pub fn as_str(&self) -> Option<&'a str> {
        if let Value::String(s) = self {
            Some(*s)
        } else {
            None
        }
    }
}

// Compile-time guarantee that the arena-allocated representation stays `Copy`
// (hence `Drop`-free): bump allocation never runs destructors, so a `Drop` type
// here would leak.
const _: fn() = || {
    fn assert_copy<T: Copy>() {}
    assert_copy::<Value<'static>>();
    assert_copy::<Object<'static>>();
};
