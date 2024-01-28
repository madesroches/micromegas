use simple_asn1::BigUint;

use crate::{Error, Result};

use super::{SignatureValidation, ValidationResult};

pub struct RsaSignatureValidation {
    pub key: ring::signature::RsaPublicKeyComponents<Vec<u8>>,
}

impl RsaSignatureValidation {
    pub fn new(key: ring::signature::RsaPublicKeyComponents<Vec<u8>>) -> Self {
        Self { key }
    }

    /// Create a new `RsaSignatureValidation` from the RSA key public components
    /// in their base64 representation.
    pub fn new_from_components(n: &str, e: &str) -> Result<Self> {
        let n = BigUint::from_bytes_be(
            &base64::decode_config(n, base64::URL_SAFE_NO_PAD)
                .map_err(|err| Error::Internal(format!("failed to decode n component: {}", err)))?,
        )
        .to_bytes_be();
        let e = BigUint::from_bytes_be(
            &base64::decode_config(e, base64::URL_SAFE_NO_PAD)
                .map_err(|err| Error::Internal(format!("failed to decode e component: {}", err)))?,
        )
        .to_bytes_be();
        let key = ring::signature::RsaPublicKeyComponents { n, e };

        Ok(Self::new(key))
    }

    fn alg_to_rsa_parameters(alg: &str) -> Result<&'static ring::signature::RsaParameters> {
        Ok(match alg {
            "RS256" => &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            "RS384" => &ring::signature::RSA_PKCS1_2048_8192_SHA384,
            "RS512" => &ring::signature::RSA_PKCS1_2048_8192_SHA512,
            "PS256" => &ring::signature::RSA_PSS_2048_8192_SHA256,
            "PS384" => &ring::signature::RSA_PSS_2048_8192_SHA384,
            "PS512" => &ring::signature::RSA_PSS_2048_8192_SHA512,
            _ => return Err(Error::Internal(format!("unsupported algorithm: {}", alg))),
        })
    }
}

impl SignatureValidation for RsaSignatureValidation {
    /// Validate the JWT signature.
    ///
    /// # Example
    ///
    /// ```
    /// use lgn_auth::jwt::{
    ///     Token,
    ///     signature_validation::{
    ///         RsaSignatureValidation,
    ///         SignatureValidation,
    ///     },
    /// };
    ///
    /// let validation = RsaSignatureValidation::new_from_components(
    ///     "6S7asUuzq5Q_3U9rbs-PkDVIdjgmtgWreG5qWPsC9xXZKiMV1AiV9LXyqQsAYpCqEDM3XbfmZqGb48yLhb_XqZaKgSYaC_h2DjM7lgrIQAp9902Rr8fUmLN2ivr5tnLxUUOnMOc2SQtr9dgzTONYW5Zu3PwyvAWk5D6ueIUhLtYzpcB-etoNdL3Ir2746KIy_VUsDwAM7dhrqSK8U2xFCGlau4ikOTtvzDownAMHMrfE7q1B6WZQDAQlBmxRQsyKln5DIsKv6xauNsHRgBAKctUxZG8M4QJIx3S6Aughd3RZC4Ca5Ae9fd8L8mlNYBCrQhOZ7dS0f4at4arlLcajtw",
    ///     "AQAB",
    /// ).unwrap();
    /// let token: Token = "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiYWRtaW4iOnRydWUsImlhdCI6MTYzNTk2NDY5NSwiZXhwIjoxNjM1OTY4Mjk1fQ.PombMRzfZo00dGKI6KGqdBMcojVulZSJYRrXytpkPjGoz8A_74rnXmOZGq9UhJobOF8zsek9e4mYTcO30FzBwAXygTKB3S4x7kRnaw6YCzkPo5gzGiPWiNRtSByn58OTU7xYNikA4JJBCSyT6U6nV0yoTjg-sMC2yg7YlvXMRvKL9JIBDdA7xdIWHw7krigG-hpL75WLqAXTzffuFgCAsh739e06ukdacL3QiZ36dqYGXh3A-QA5Ls9xw4ZW_AISO8QoZ4gKUIpWbftl36lvl5ZQht0bO7g9J3RMJ1GuwWj82SCCii7w15oR5o5HVfETvTxPDzy6fnBDkclel1VvKg".try_into().unwrap();
    ///
    /// token.validate_signature(&validation).unwrap();
    /// ```
    fn validate_signature<'a>(
        &self,
        alg: &'a str,
        kid: Option<&'a str>,
        message: &'a str,
        signature: &'a [u8],
    ) -> ValidationResult<'a> {
        match Self::alg_to_rsa_parameters(alg) {
            Ok(parameters) => match self.key.verify(parameters, message.as_bytes(), signature) {
                Ok(()) => ValidationResult::Valid,
                Err(_) => ValidationResult::Invalid(Error::Internal(
                    "the signature does not match".to_string(),
                )),
            },
            Err(_) => ValidationResult::Unsupported(alg, kid),
        }
    }
}
