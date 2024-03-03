use std::time;

use serde::{Deserialize, Serialize};

use super::{
    signature_validation::{NoSignatureValidation, SignatureValidation},
    Token,
};

use crate::{Error, Result};

pub type UnsecureValidation = Validation<NoSignatureValidation>;

/// Provides JWT validation.
#[derive(Clone, Debug)]
pub struct Validation<SV = NoSignatureValidation> {
    /// A tolerance for the not-before and expiry times.
    leeway: time::Duration,

    /// The signature validation method.
    ///
    /// If `None`, the signature is not verified which is not recommended.
    signature_validation: SV,

    /// A function that returns the current time.
    time_fn: fn() -> time::SystemTime,

    /// Whether to validate the expiration.
    validate_exp: bool,

    /// Whether to validate the not-before.
    validate_nbf: bool,

    /// A issuer to check.
    iss: Option<String>,

    /// A subject identifier to check.
    sub: Option<String>,

    /// A audience to check.
    aud: Option<String>,
}

impl<SV> Default for Validation<SV>
where
    SV: Default,
{
    fn default() -> Self {
        Self {
            leeway: time::Duration::from_secs(0),
            signature_validation: SV::default(),
            time_fn: std::time::SystemTime::now,
            validate_exp: true,
            validate_nbf: true,
            iss: None,
            sub: None,
            aud: None,
        }
    }
}

impl<SV> Validation<SV>
where
    SV: SignatureValidation,
{
    pub fn new(signature_validation: SV) -> Self {
        Self {
            leeway: time::Duration::from_secs(0),
            signature_validation,
            time_fn: std::time::SystemTime::now,
            validate_exp: true,
            validate_nbf: true,
            iss: None,
            sub: None,
            aud: None,
        }
    }

    /// Sets the leeway for the not-before and expiry times.
    #[must_use]
    pub fn with_leeway(mut self, leeway: time::Duration) -> Self {
        self.leeway = leeway;
        self
    }

    /// Sets the signature validation method.
    #[must_use]
    pub fn with_signature_validation(mut self, signature_validation: SV) -> Self {
        self.signature_validation = signature_validation;
        self
    }

    /// Sets the time function.
    #[must_use]
    pub fn with_time_fn(mut self, time_fn: fn() -> time::SystemTime) -> Self {
        self.time_fn = time_fn;
        self
    }

    /// Disables the validation of the expiration.
    #[must_use]
    pub fn disable_exp_validation(mut self) -> Self {
        self.validate_exp = false;
        self
    }

    /// Disables the validation of the not-before.
    #[must_use]
    pub fn disable_nbf_validation(mut self) -> Self {
        self.validate_nbf = false;
        self
    }

    /// Sets the issuer to check.
    #[must_use]
    pub fn with_iss(mut self, iss: impl Into<String>) -> Self {
        self.iss = Some(iss.into());
        self
    }

    /// Sets the subject identifier to check.
    #[must_use]
    pub fn with_sub(mut self, sub: impl Into<String>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    /// Sets the audience to check.
    #[must_use]
    pub fn with_aud(mut self, aud: impl Into<String>) -> Self {
        self.aud = Some(aud.into());
        self
    }

    /// Validate the specified token's signature.
    pub(crate) fn validate_signature(&self, token: &Token<'_>) -> Result<()> {
        token.validate_signature(&self.signature_validation)
    }

    /// Validate the specified token's claims.
    pub(crate) fn validate_claims(&self, token: &Token<'_>) -> Result<()> {
        let now = (self.time_fn)();
        let claims: Claims = token
            .to_claims_unsafe()
            .map_err(|err| Error::Internal(format!("failed to read common claims: {}", err)))?;

        if self.validate_exp {
            let exp = time::UNIX_EPOCH
                .checked_add(time::Duration::from_secs(claims.exp))
                .ok_or_else(|| Error::Internal("invalid exp".to_string()))?;

            if exp + self.leeway < now {
                return Err(Error::TokenExpired { exp, now });
            }
        }

        if self.validate_nbf {
            if let Some(nbf) = claims.nbf {
                let nbf = time::UNIX_EPOCH
                    .checked_add(time::Duration::from_secs(nbf))
                    .ok_or_else(|| Error::Internal("invalid nbf".to_string()))?;

                if nbf > now + self.leeway {
                    return Err(Error::Internal(format!(
                        "the token is not yet valid: {:#?} > {:#?}",
                        nbf, now
                    )));
                }
            }
        }

        if let Some(iss) = &self.iss {
            match &claims.iss {
                Some(claims_iss) => {
                    if iss != claims_iss {
                        return Err(Error::Internal(format!(
                            "the token's issuer is invalid: got `{}` when `{}` was expected",
                            claims_iss, iss,
                        )));
                    }
                }
                _ => {
                    return Err(Error::Internal(format!(
                        "the token has no issuer but `{}` was expected",
                        iss
                    )))
                }
            }
        }

        if let Some(sub) = &self.sub {
            match &claims.sub {
                Some(claims_sub) => {
                    if sub != claims_sub {
                        return Err(Error::Internal(format!(
                            "the token's subject identifier is invalid: got `{}` when `{}` was expected",
                            claims_sub,
                            sub,
                        )));
                    }
                }
                _ => {
                    return Err(Error::Internal(format!(
                        "the token has no subject identifier but `{}` was expected",
                        sub
                    )))
                }
            }
        }

        if let Some(aud) = &self.aud {
            match &claims.aud {
                Some(claims_aud) => {
                    if aud != claims_aud {
                        return Err(Error::Internal(format!(
                            "the token's audience is invalid: got `{}` when `{}` was expected",
                            claims_aud, aud,
                        )));
                    }
                }
                _ => {
                    return Err(Error::Internal(format!(
                        "the token has no audience but `{}` was expected",
                        aud
                    )))
                }
            }
        }

        Ok(())
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
struct Claims {
    pub exp: u64,
    pub nbf: Option<u64>,
    pub sub: Option<String>,
    pub iss: Option<String>,
    pub aud: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwt::signature_validation::RsaSignatureValidation;

    #[derive(Deserialize, PartialEq, Eq, Debug, Clone)]
    struct MyClaims {}

    #[test]
    fn test_validation() {
        let signature_validation = RsaSignatureValidation::new_from_components(
         "6S7asUuzq5Q_3U9rbs-PkDVIdjgmtgWreG5qWPsC9xXZKiMV1AiV9LXyqQsAYpCqEDM3XbfmZqGb48yLhb_XqZaKgSYaC_h2DjM7lgrIQAp9902Rr8fUmLN2ivr5tnLxUUOnMOc2SQtr9dgzTONYW5Zu3PwyvAWk5D6ueIUhLtYzpcB-etoNdL3Ir2746KIy_VUsDwAM7dhrqSK8U2xFCGlau4ikOTtvzDownAMHMrfE7q1B6WZQDAQlBmxRQsyKln5DIsKv6xauNsHRgBAKctUxZG8M4QJIx3S6Aughd3RZC4Ca5Ae9fd8L8mlNYBCrQhOZ7dS0f4at4arlLcajtw",
         "AQAB",
    ).unwrap();

        let validation = Validation::new(signature_validation)
            .with_time_fn(|| {
                time::UNIX_EPOCH
                    .checked_add(time::Duration::from_secs(1635964695))
                    .unwrap()
            })
            .with_sub("1234567890");

        let token: Token<'_> = "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiYWRtaW4iOnRydWUsImlhdCI6MTYzNTk2NDY5NSwiZXhwIjoxNjM1OTY4Mjk1fQ.PombMRzfZo00dGKI6KGqdBMcojVulZSJYRrXytpkPjGoz8A_74rnXmOZGq9UhJobOF8zsek9e4mYTcO30FzBwAXygTKB3S4x7kRnaw6YCzkPo5gzGiPWiNRtSByn58OTU7xYNikA4JJBCSyT6U6nV0yoTjg-sMC2yg7YlvXMRvKL9JIBDdA7xdIWHw7krigG-hpL75WLqAXTzffuFgCAsh739e06ukdacL3QiZ36dqYGXh3A-QA5Ls9xw4ZW_AISO8QoZ4gKUIpWbftl36lvl5ZQht0bO7g9J3RMJ1GuwWj82SCCii7w15oR5o5HVfETvTxPDzy6fnBDkclel1VvKg".try_into().unwrap();

        let claims: MyClaims = token.into_claims(&validation).unwrap();

        assert_eq!(claims, MyClaims {});
    }
}
