use super::{Error, Result};
use serde::{de::Visitor, Deserializer};
use std::any::type_name;

macro_rules! unsupported_type {
    ($trait_fn:ident) => {
        fn $trait_fn<V>(self, _: V) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
        {
            Err(Error::Unsupported(type_name::<V::Value>().to_string()))
        }
    };
}

macro_rules! deserialize_type {
    ($trait_fn:ident, $visit_fn:ident, $ty:literal) => {
        fn $trait_fn<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
        {
            let value = self.input.parse().map_err(|err| {
                Error::Parsing(format!(
                    "cannot parse {} to {}: {}",
                    self.input.clone(),
                    $ty,
                    err
                ))
            })?;
            visitor.$visit_fn(value)
        }
    };
}

#[derive(Default)]
pub(crate) struct PercentEncodingDeserializer {
    input: String,
}

impl<'de> PercentEncodingDeserializer {
    pub(crate) fn new(input: &'de str) -> Result<Self> {
        Ok(Self {
            input: percent_encoding::percent_decode_str(input)
                .decode_utf8()?
                .to_string(),
        })
    }
}

impl<'de> Deserializer<'de> for &PercentEncodingDeserializer {
    type Error = Error;

    unsupported_type!(deserialize_any);
    unsupported_type!(deserialize_bytes);
    unsupported_type!(deserialize_option);
    unsupported_type!(deserialize_identifier);
    unsupported_type!(deserialize_ignored_any);

    deserialize_type!(deserialize_bool, visit_bool, "bool");
    deserialize_type!(deserialize_i8, visit_i8, "i8");
    deserialize_type!(deserialize_i16, visit_i16, "i16");
    deserialize_type!(deserialize_i32, visit_i32, "i32");
    deserialize_type!(deserialize_i64, visit_i64, "i64");
    deserialize_type!(deserialize_i128, visit_i128, "i128");
    deserialize_type!(deserialize_u8, visit_u8, "u8");
    deserialize_type!(deserialize_u16, visit_u16, "u16");
    deserialize_type!(deserialize_u32, visit_u32, "u32");
    deserialize_type!(deserialize_u64, visit_u64, "u64");
    deserialize_type!(deserialize_u128, visit_u128, "u128");
    deserialize_type!(deserialize_f32, visit_f32, "f32");
    deserialize_type!(deserialize_f64, visit_f64, "f64");
    deserialize_type!(deserialize_string, visit_string, "String");
    deserialize_type!(deserialize_byte_buf, visit_string, "String");
    deserialize_type!(deserialize_char, visit_char, "char");

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_str(self.input.as_str())
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(Error::Unsupported("<seq>".to_string()))
    }

    fn deserialize_tuple<V>(self, _len: usize, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(Error::Unsupported("<tuple>".to_string()))
    }

    fn deserialize_tuple_struct<V>(
        self,
        name: &'static str,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(Error::Unsupported(format!("<tuple struct {}>", name)))
    }

    fn deserialize_map<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(Error::Unsupported("<map>".to_string()))
    }

    fn deserialize_struct<V>(
        self,
        name: &'static str,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(Error::Unsupported(format!("<struct {}>", name)))
    }

    fn deserialize_enum<V>(
        self,
        name: &'static str,
        _variants: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(Error::Unsupported(format!("<enum {}>", name)))
    }
}
