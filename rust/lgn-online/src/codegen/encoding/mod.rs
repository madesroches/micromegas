mod de;
mod ser;

pub use super::{Error, Result};

use serde::{Deserialize, Serialize};

pub fn to_percent_encoded_string<T>(value: &T) -> Result<String>
where
    T: Serialize,
{
    let mut serializer = ser::PercentEncodingSerializer::default();
    value.serialize(&mut serializer)
}

pub fn from_percent_encoded_string<'de, T>(value: &str) -> Result<T>
where
    T: Deserialize<'de>,
{
    let deserializer = de::PercentEncodingDeserializer::new(value)?;
    T::deserialize(&deserializer)
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use crate::codegen::Bytes;

    pub use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    struct ComplexType {
        a: String,
        b: u32,
        c: Vec<u32>,
    }

    #[test]
    fn test_to_percent_encoded_string() {
        assert_eq!(
            to_percent_encoded_string::<i32>(&2).unwrap(),
            "2".to_string()
        );
        assert_eq!(
            to_percent_encoded_string(&"foo").unwrap(),
            "foo".to_string()
        );
        assert_eq!(
            to_percent_encoded_string(&"hello world").unwrap(),
            "hello%20world".to_string()
        );
        assert_eq!(
            to_percent_encoded_string(&"hello%20world").unwrap(),
            "hello%2520world".to_string()
        );
        assert_eq!(
            to_percent_encoded_string(&"this is a \"very\" <complex> val`ue").unwrap(),
            "this%20is%20a%20%22very%22%20%3Ccomplex%3E%20val%60ue".to_string()
        );
        assert_eq!(
            to_percent_encoded_string(&Bytes(b"1234".to_vec())).unwrap(),
            "MTIzNA".to_string()
        );

        // Some complex types are not supported.
        to_percent_encoded_string(&ComplexType {
            a: "foo".to_string(),
            b: 42,
            c: vec![1, 2, 3],
        })
        .unwrap_err();
    }

    #[test]
    fn test_from_percent_encoded_string() {
        assert_eq!(from_percent_encoded_string::<i32>("2").unwrap(), 2);
        assert_eq!(
            from_percent_encoded_string::<String>("foo").unwrap(),
            "foo".to_string()
        );
        assert_eq!(
            from_percent_encoded_string::<String>("hello%2520world").unwrap(),
            "hello%20world".to_string()
        );
        assert_eq!(
            from_percent_encoded_string::<String>("hello%20world").unwrap(),
            "hello world".to_string()
        );
        assert_eq!(
            from_percent_encoded_string::<String>(
                "this%20is%20a%20%22very%22%20%3Ccomplex%3E%20val%60ue"
            )
            .unwrap(),
            "this is a \"very\" <complex> val`ue".to_string()
        );
        assert_eq!(
            from_percent_encoded_string::<Bytes>("MTIzNA").unwrap(),
            Bytes(b"1234".to_vec())
        );
        from_percent_encoded_string::<ComplexType>("{ a: \"foo\" }").unwrap_err();
    }
}
