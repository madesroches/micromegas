use anyhow::Result;

pub fn encode_cbor<T: serde::Serialize>(obj: &T) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(obj, &mut bytes)?;
    Ok(bytes)
}
