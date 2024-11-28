use anyhow::Result;
use micromegas_telemetry::property::Property;
use micromegas_transit::value::{Object, Value};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct PropertySet {
    obj: Arc<Object>,
}

impl PropertySet {
    pub fn new(obj: Arc<Object>) -> Self {
        Self { obj }
    }

    pub fn empty() -> Self {
        lazy_static::lazy_static! {
            static ref TYPE_NAME: Arc<String> = Arc::new("EmptyPropertySet".into());
            static ref EMPTY_SET: PropertySet = PropertySet::new( Arc::new( Object{ type_name: TYPE_NAME.clone(), members: vec![] }) );
        }
        EMPTY_SET.clone()
    }

    pub fn for_each_property<Fun: FnMut(Property) -> Result<()>>(
        &self,
        mut fun: Fun,
    ) -> Result<()> {
        for (key, value) in &self.obj.members {
            if let Value::String(value_str) = value {
                fun(Property::new(key.clone(), value_str.clone()))?;
            }
        }
        Ok(())
    }
}

impl From<Arc<Object>> for PropertySet {
    fn from(value: Arc<Object>) -> Self {
        Self::new(value)
    }
}
