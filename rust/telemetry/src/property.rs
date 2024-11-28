use sqlx::postgres::{PgHasArrayType, PgTypeInfo};
use std::{collections::HashMap, sync::Arc};

#[derive(Debug)]
pub struct Property {
    key: Arc<String>,
    value: Arc<String>,
}

impl Property {
    pub fn new(key: Arc<String>, value: Arc<String>) -> Self {
        Self { key, value }
    }

    pub fn key_str(&self) -> &str {
        self.key.as_str()
    }

    pub fn value_str(&self) -> &str {
        self.value.as_str()
    }
}

impl ::sqlx::encode::Encode<'_, ::sqlx::Postgres> for Property {
    fn encode_by_ref(
        &self,
        buf: &mut ::sqlx::postgres::PgArgumentBuffer,
    ) -> std::result::Result<
        sqlx::encode::IsNull,
        std::boxed::Box<(dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static)>,
    > {
        let mut encoder = ::sqlx::postgres::types::PgRecordEncoder::new(buf);
        encoder.encode(self.key.as_str())?;
        encoder.encode(self.value.as_str())?;
        encoder.finish();
        Ok(::sqlx::encode::IsNull::No)
    }
    fn size_hint(&self) -> ::std::primitive::usize {
        2usize * (4 + 4)
            + <String as ::sqlx::encode::Encode<::sqlx::Postgres>>::size_hint(&self.key)
            + <String as ::sqlx::encode::Encode<::sqlx::Postgres>>::size_hint(&self.value)
    }
}

impl ::sqlx::decode::Decode<'_, ::sqlx::Postgres> for Property {
    fn decode(
        value: ::sqlx::postgres::PgValueRef<'_>,
    ) -> ::std::result::Result<
        Self,
        ::std::boxed::Box<
            dyn ::std::error::Error + 'static + ::std::marker::Send + ::std::marker::Sync,
        >,
    > {
        let mut decoder = ::sqlx::postgres::types::PgRecordDecoder::new(value)?;
        let key = decoder.try_decode::<String>()?;
        let value = decoder.try_decode::<String>()?;
        ::std::result::Result::Ok(Property::new(Arc::new(key), Arc::new(value)))
    }
}
#[automatically_derived]
impl ::sqlx::Type<::sqlx::Postgres> for Property {
    fn type_info() -> ::sqlx::postgres::PgTypeInfo {
        ::sqlx::postgres::PgTypeInfo::with_name("micromegas_property")
    }
}

// array support
impl PgHasArrayType for Property {
    fn array_type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("_micromegas_property")
    }
}

pub fn make_properties(map: &HashMap<String, String>) -> Vec<Property> {
    map.iter()
        .map(|(k, v)| Property::new(Arc::new(k.clone()), Arc::new(v.clone())))
        .collect()
}

pub fn into_hashmap(properties: Vec<Property>) -> HashMap<String, String> {
    let mut hashmap = HashMap::new();
    for property in properties {
        hashmap.insert((*property.key).clone(), (*property.value).clone());
    }
    hashmap
}
