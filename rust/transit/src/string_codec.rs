use crate::{Reflect, UserDefinedType};

#[repr(u8)]
#[derive(Debug, Copy, Clone)]
pub enum StringCodec {
    Ansi = 0,
    Wide = 1,
    Utf8 = 2,
}

impl Reflect for StringCodec {
    fn reflect() -> UserDefinedType {
        UserDefinedType {
            name: String::from("StringCodec"),
            size: 1,
            members: vec![],
            is_reference: false,
            secondary_udts: vec![],
        }
    }
}

impl TryFrom<u8> for StringCodec {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(StringCodec::Ansi),
            1 => Ok(StringCodec::Wide),
            2 => Ok(StringCodec::Utf8),
            other => anyhow::bail!("invalid codec id {other}"),
        }
    }
}
