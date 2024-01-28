use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// A JWT 'JOSE' header, as defined by RFC 7519.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub alg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typ: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
}

impl FromStr for Header {
    type Err = Error;

    /// Creates a new JWT header from its base64 representation.
    ///
    /// # Arguments
    ///
    /// * `base64` - The base64 representation of the header, without its
    ///   trailing '.'.
    ///
    /// # Errors
    ///
    /// * `base64::DecodeError` - If the base64 representation is invalid.
    /// * `serde_json::Error` - If the header could not be deserialized.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgn_auth::jwt::Header;
    ///
    /// let header: Header = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9".parse().unwrap();
    ///
    /// assert_eq!(header, Header {
    ///     alg: "HS256".to_string(),
    ///     typ: Some("JWT".to_string()),
    ///     kid: None,
    /// });
    /// ```
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_slice(
            base64::decode_config(s, base64::URL_SAFE_NO_PAD)
                .map_err(|err| {
                    Error::Internal(format!("failed to decode base64 JWT header: {}", err))
                })?
                .as_slice(),
        )
        .map_err(|err| Error::Internal(format!("failed to parse JSON JWT header: {}", err)))
    }
}

impl ToString for Header {
    /// Returns the base64 representation of the header.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgn_auth::jwt::Header;
    ///
    /// let header = Header {
    ///     alg: "HS256".to_string(),
    ///     typ: Some("JWT".to_string()),
    ///     kid: None,
    /// };
    ///
    /// assert_eq!(header.to_string(), "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9");
    /// ```
    fn to_string(&self) -> String {
        base64::encode_config(
            serde_json::to_string(self).expect("failed to encode JSON JWT header"),
            base64::URL_SAFE_NO_PAD,
        )
    }
}
