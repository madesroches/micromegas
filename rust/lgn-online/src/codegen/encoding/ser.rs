use super::{Error, Result};

#[derive(Default)]
pub(crate) struct PercentEncodingSerializer {}

impl PercentEncodingSerializer {
    #[allow(clippy::unnecessary_wraps)]
    fn serialize_as_string<T: std::fmt::Display>(v: T) -> Result<String> {
        Ok(percent_encoding::utf8_percent_encode(
            &v.to_string(),
            percent_encoding::NON_ALPHANUMERIC,
        )
        .to_string())
    }
}

impl<'a> serde::ser::Serializer for &'a mut PercentEncodingSerializer {
    type Ok = String;
    type Error = Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        PercentEncodingSerializer::serialize_as_string(v)
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(Error::unsupported(v))
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::Unsupported("<none>".to_string()))
    }

    fn serialize_some<T: ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: serde::Serialize,
    {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::Unsupported("<unit>".to_string()))
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Err(Error::Unsupported(format!(
            "<unit variant {} {}>",
            name, variant
        )))
    }

    fn serialize_newtype_struct<T: ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: serde::Serialize,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: serde::Serialize,
    {
        Err(Error::Unsupported("<newtype variant>".to_string()))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(Error::Unsupported("<seq>".to_string()))
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(Error::Unsupported("<tuple>".to_string()))
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(Error::Unsupported(format!("<tuple struct {}>", name)))
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(Error::Unsupported(format!("<tuple variant {}>", name)))
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(Error::Unsupported("<map>".to_string()))
    }

    fn serialize_struct(
        self,
        name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Err(Error::Unsupported(format!("<struct {}>", name)))
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(Error::Unsupported(format!("<struct variant {}>", name)))
    }
}

impl<'a> serde::ser::SerializeSeq for &'a mut PercentEncodingSerializer {
    type Ok = String;
    type Error = Error;

    fn serialize_element<T: ?Sized>(&mut self, _value: &T) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        Err(Error::Unsupported("<seq element>".to_string()))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::Unsupported("<seq end>".to_string()))
    }
}

impl<'a> serde::ser::SerializeTuple for &'a mut PercentEncodingSerializer {
    type Ok = String;
    type Error = Error;

    fn serialize_element<T: ?Sized>(&mut self, _value: &T) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        Err(Error::Unsupported("<tuple element>".to_string()))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::Unsupported("<tuple end>".to_string()))
    }
}

impl<'a> serde::ser::SerializeTupleStruct for &'a mut PercentEncodingSerializer {
    type Ok = String;
    type Error = Error;

    fn serialize_field<T: ?Sized>(&mut self, _value: &T) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        Err(Error::Unsupported("<tuple struct field>".to_string()))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::Unsupported("<tuple struct end>".to_string()))
    }
}

impl<'a> serde::ser::SerializeTupleVariant for &'a mut PercentEncodingSerializer {
    type Ok = String;
    type Error = Error;

    fn serialize_field<T: ?Sized>(&mut self, _value: &T) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        Err(Error::Unsupported("<tuple variant field>".to_string()))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::Unsupported("<tuple variant end>".to_string()))
    }
}

impl<'a> serde::ser::SerializeMap for &'a mut PercentEncodingSerializer {
    type Ok = String;
    type Error = Error;

    fn serialize_key<T: ?Sized>(&mut self, _key: &T) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        Err(Error::Unsupported("<map key>".to_string()))
    }

    fn serialize_value<T: ?Sized>(&mut self, _value: &T) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        Err(Error::Unsupported("<map value>".to_string()))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::Unsupported("<map end>".to_string()))
    }
}

impl<'a> serde::ser::SerializeStruct for &'a mut PercentEncodingSerializer {
    type Ok = String;
    type Error = Error;

    fn serialize_field<T: ?Sized>(
        &mut self,
        _key: &'static str,
        _value: &T,
    ) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        Err(Error::Unsupported("<struct field>".to_string()))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::Unsupported("<struct end>".to_string()))
    }
}

impl<'a> serde::ser::SerializeStructVariant for &'a mut PercentEncodingSerializer {
    type Ok = String;
    type Error = Error;

    fn serialize_field<T: ?Sized>(
        &mut self,
        _key: &'static str,
        _value: &T,
    ) -> Result<(), Self::Error>
    where
        T: serde::Serialize,
    {
        Err(Error::Unsupported("<struct variant field>".to_string()))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::Unsupported("<struct variant end>".to_string()))
    }
}
