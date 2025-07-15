use anyhow::Result;

/// Encodes a serializable object into CBOR format.
///
/// This function is a thin wrapper around `ciborium::ser::into_writer`.
pub fn encode_cbor<T: serde::Serialize>(obj: &T) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(obj, &mut bytes)?;
    Ok(bytes)
}
