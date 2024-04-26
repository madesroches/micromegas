use sqlx::postgres::{PgHasArrayType, PgTypeInfo};
use std::collections::HashMap;

#[derive(sqlx::Type)]
#[sqlx(type_name = "micromegas_property")]
pub struct Property {
    pub key: String,
    pub value: String,
}

impl PgHasArrayType for Property {
    fn array_type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("_micromegas_property")
    }
}

impl Property {
    pub fn new(key: String, value: String) -> Self {
        Self { key, value }
    }
}

pub fn make_properties(map: &HashMap<String, String>) -> Vec<Property> {
    map.iter()
        .map(|(k, v)| Property::new(k.to_owned(), v.to_owned()))
        .collect()
}
