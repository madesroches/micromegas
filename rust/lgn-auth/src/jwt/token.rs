use super::{signature_validation::SignatureValidation, Header, Validation};

use crate::{Error, Result};

/// A JWT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    raw: &'a str,
    signed: &'a str,
    header: &'a str,
    payload: &'a str,
    signature: &'a str,
}

impl<'a> Token<'a> {
    /// Get the header of the JWT.
    ///
    /// This may fail if the header is not valid.
    pub fn header(&self) -> Result<Header> {
        self.header
            .parse()
            .map_err(|err| Error::Internal(format!("failed to parse header: {}", err)))
    }

    // Get the signature of the JWT.
    //
    // This may fail if the signature is not valid.
    pub fn signature(&self) -> Result<Vec<u8>> {
        base64::decode_config(self.signature, base64::URL_SAFE_NO_PAD)
            .map_err(|err| Error::Internal(format!("invalid base64 JWT signature: {}", err)))
    }

    /// Convert the token into its claims.
    ///
    /// This method does not validate the token and should **NOT** be used most
    /// of the time.
    pub fn into_claims_unsafe<C>(self) -> Result<C>
    where
        C: serde::de::DeserializeOwned,
    {
        self.to_claims_unsafe()
    }

    /// Parse the token into claims.
    ///
    /// This method does not validate the token and should **NOT** be used most
    /// of the time.
    pub(crate) fn to_claims_unsafe<C>(&self) -> Result<C>
    where
        C: serde::de::DeserializeOwned,
    {
        let payload =
            base64::decode_config(self.payload, base64::URL_SAFE_NO_PAD).map_err(|err| {
                Error::Internal(format!("failed to decode base64 JWT payload: {}", err))
            })?;

        serde_json::from_slice(payload.as_slice()).map_err(|err| {
            Error::Internal(format!(
                "failed to convert JWT payload into claims: {}",
                err
            ))
        })
    }

    /// Convert the token into its claims.
    ///
    /// This method validates the token and is recommended.
    pub fn into_claims<C, T>(self, validation: &Validation<T>) -> Result<C>
    where
        C: serde::de::DeserializeOwned,
        T: SignatureValidation,
    {
        self.validate(validation)?.into_claims_unsafe()
    }

    /// Validate the token.
    pub fn validate<T>(self, validation: &Validation<T>) -> Result<Self>
    where
        T: SignatureValidation,
    {
        validation.validate_signature(&self)?;
        validation.validate_claims(&self)?;

        Ok(self)
    }

    /// Validate the signature on the token without consuming it.
    pub fn validate_signature<T>(&self, signature_validation: &T) -> Result<()>
    where
        T: SignatureValidation,
    {
        let header = self.header()?;
        let signature = self.signature()?;

        signature_validation
            .validate_signature(&header.alg, header.kid.as_deref(), self.signed, &signature)
            .ok()
    }

    pub fn as_str(&self) -> &str {
        self.raw
    }
}

impl<'a> TryFrom<&'a str> for Token<'a> {
    type Error = Error;

    /// Parses a JWT from a string.
    ///
    /// The string must be in the format `<header>.<payload>.<signature>`.
    ///
    /// The header, payload and signature are base64 encoded, as per RFC 7519.
    ///
    /// Note: The signature is **NOT** verified.
    ///
    /// # Example
    /// ```
    /// use lgn_auth::jwt::Token;
    ///
    /// let raw_token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
    /// let token: Token = raw_token.clone().try_into().unwrap();
    ///
    /// assert_eq!(
    ///     token.as_str(),
    ///     raw_token,
    /// );
    /// ```
    fn try_from(raw: &'a str) -> Result<Self, Self::Error> {
        let (signed, signature) = raw
            .rsplit_once('.')
            .ok_or_else(|| Error::Internal("invalid JWT".to_string()))?;
        let mut parts = signed.splitn(2, '.');
        let header = parts
            .next()
            .ok_or_else(|| Error::Internal("missing header".to_string()))?;
        let payload = parts
            .next()
            .ok_or_else(|| Error::Internal("missing payload".to_string()))?;

        Ok(Self {
            raw,
            signed,
            header,
            payload,
            signature,
        })
    }
}

impl std::fmt::Display for Token<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.raw)
    }
}
