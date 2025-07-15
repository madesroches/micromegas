use anyhow::{Result, bail};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Object {
    pub type_name: Arc<String>,
    pub members: Vec<(Arc<String>, Value)>,
}

impl Object {
    pub fn get<T>(&self, member_name: &str) -> Result<T>
    where
        T: TransitValue,
    {
        for m in &self.members {
            if *m.0 == member_name {
                return T::get(&m.1);
            }
        }
        bail!("member {} not found in {:?}", member_name, self);
    }

    pub fn get_ref(&self, member_name: &str) -> Result<&Value> {
        for m in &self.members {
            if *m.0 == member_name {
                return Ok(&m.1);
            }
        }
        bail!("member {} not found", member_name);
    }
}

pub trait TransitValue {
    fn get(value: &Value) -> Result<Self>
    where
        Self: Sized;
}

impl TransitValue for u8 {
    fn get(value: &Value) -> Result<Self> {
        if let Value::U8(val) = value {
            Ok(*val)
        } else {
            bail!("bad type cast u8 for value {:?}", value);
        }
    }
}

impl TransitValue for u32 {
    fn get(value: &Value) -> Result<Self> {
        match value {
            Value::U32(val) => Ok(*val),
            Value::U8(val) => Ok(Self::from(*val)),
            _ => {
                bail!("bad type cast u32 for value {:?}", value);
            }
        }
    }
}

impl TransitValue for u64 {
    fn get(value: &Value) -> Result<Self> {
        match value {
            Value::I64(val) => Ok(*val as Self),
            Value::U64(val) => Ok(*val),
            _ => {
                bail!("bad type cast u64 for value {:?}", value)
            }
        }
    }
}

impl TransitValue for i64 {
    #[allow(clippy::cast_possible_wrap)]
    fn get(value: &Value) -> Result<Self> {
        match value {
            Value::I64(val) => Ok(*val),
            Value::U64(val) => Ok(*val as Self),
            _ => {
                bail!("bad type cast i64 for value {:?}", value)
            }
        }
    }
}

impl TransitValue for f64 {
    fn get(value: &Value) -> Result<Self> {
        if let Value::F64(val) = value {
            Ok(*val)
        } else {
            bail!("bad type cast f64 for value {:?}", value);
        }
    }
}

impl TransitValue for Arc<String> {
    fn get(value: &Value) -> Result<Self> {
        if let Value::String(val) = value {
            Ok(val.clone())
        } else {
            bail!("bad type cast String for value {:?}", value);
        }
    }
}

impl TransitValue for Arc<Object> {
    fn get(value: &Value) -> Result<Self> {
        if let Value::Object(val) = value {
            Ok(val.clone())
        } else {
            bail!("bad type cast String for value {:?}", value);
        }
    }
}

#[derive(Debug, Clone)]
pub enum Value {
    String(Arc<String>),
    Object(Arc<Object>),
    U8(u8),
    U32(u32),
    U64(u64),
    I64(i64),
    F64(f64),
    None,
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        if let Value::String(s) = &self {
            Some(s.as_str())
        } else {
            None
        }
    }
}
