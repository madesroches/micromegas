use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rsa::RsaPrivateKey;
use rsa::pkcs1::{EncodeRsaPrivateKey, EncodeRsaPublicKey};
use serde::{Deserialize, Serialize};

/// Test OIDC claims
#[derive(Debug, Serialize, Deserialize)]
pub struct TestClaims {
    /// Subject (user ID)
    pub sub: String,
    /// Issuer
    pub iss: String,
    /// Audience
    pub aud: String,
    /// Email
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Expiration time (seconds since Unix epoch)
    pub exp: i64,
    /// Issued at (seconds since Unix epoch)
    pub iat: i64,
}

/// Test key pair for signing/verifying tokens
pub struct TestKeyPair {
    pub encoding_key: EncodingKey,
    pub decoding_key: DecodingKey,
    pub public_key_pem: String,
}

impl TestKeyPair {
    /// Generate a new RSA key pair for testing
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let private_key =
            RsaPrivateKey::new(&mut rng, 2048).expect("failed to generate RSA private key");
        let public_key = private_key.to_public_key();

        let private_pem = private_key
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .expect("failed to encode private key as PEM");
        let public_pem = public_key
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .expect("failed to encode public key as PEM");

        let encoding_key = EncodingKey::from_rsa_pem(private_pem.as_bytes())
            .expect("failed to create encoding key");
        let decoding_key = DecodingKey::from_rsa_pem(public_pem.as_bytes())
            .expect("failed to create decoding key");

        Self {
            encoding_key,
            decoding_key,
            public_key_pem: public_pem.to_string(),
        }
    }

    /// Create a test ID token with the given claims
    pub fn create_token(&self, claims: TestClaims) -> String {
        encode(&Header::new(Algorithm::RS256), &claims, &self.encoding_key)
            .expect("failed to encode token")
    }

    /// Verify a token and extract claims
    pub fn verify_token(&self, token: &str) -> Result<TestClaims, jsonwebtoken::errors::Error> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = true;
        validation.validate_aud = false; // Don't validate audience in test helper

        let token_data = decode::<TestClaims>(token, &self.decoding_key, &validation)?;
        Ok(token_data.claims)
    }
}

/// Create a valid test token with default claims
pub fn create_valid_token(
    keypair: &TestKeyPair,
    issuer: &str,
    audience: &str,
    subject: &str,
    email: Option<&str>,
) -> String {
    let now = Utc::now();
    let claims = TestClaims {
        sub: subject.to_string(),
        iss: issuer.to_string(),
        aud: audience.to_string(),
        email: email.map(String::from),
        exp: (now + Duration::hours(1)).timestamp(),
        iat: now.timestamp(),
    };
    keypair.create_token(claims)
}

/// Create an expired test token
pub fn create_expired_token(
    keypair: &TestKeyPair,
    issuer: &str,
    audience: &str,
    subject: &str,
) -> String {
    let now = Utc::now();
    let claims = TestClaims {
        sub: subject.to_string(),
        iss: issuer.to_string(),
        aud: audience.to_string(),
        email: None,
        exp: (now - Duration::hours(1)).timestamp(), // Expired 1 hour ago
        iat: (now - Duration::hours(2)).timestamp(),
    };
    keypair.create_token(claims)
}

/// Create a test token with wrong issuer
pub fn create_wrong_issuer_token(
    keypair: &TestKeyPair,
    issuer: &str,
    audience: &str,
    subject: &str,
) -> String {
    let now = Utc::now();
    let claims = TestClaims {
        sub: subject.to_string(),
        iss: format!("wrong-{}", issuer),
        aud: audience.to_string(),
        email: None,
        exp: (now + Duration::hours(1)).timestamp(),
        iat: now.timestamp(),
    };
    keypair.create_token(claims)
}

/// Create a test token with wrong audience
pub fn create_wrong_audience_token(
    keypair: &TestKeyPair,
    issuer: &str,
    audience: &str,
    subject: &str,
) -> String {
    let now = Utc::now();
    let claims = TestClaims {
        sub: subject.to_string(),
        iss: issuer.to_string(),
        aud: format!("wrong-{}", audience),
        email: None,
        exp: (now + Duration::hours(1)).timestamp(),
        iat: now.timestamp(),
    };
    keypair.create_token(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_keypair() {
        let keypair = TestKeyPair::generate();
        assert!(!keypair.public_key_pem.is_empty());
    }

    #[test]
    fn test_create_and_verify_token() {
        let keypair = TestKeyPair::generate();
        let token = create_valid_token(
            &keypair,
            "https://test.example.com",
            "test-audience",
            "user123",
            Some("user@example.com"),
        );

        let claims = keypair
            .verify_token(&token)
            .expect("failed to verify token");
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.iss, "https://test.example.com");
        assert_eq!(claims.aud, "test-audience");
        assert_eq!(claims.email, Some("user@example.com".to_string()));
    }

    #[test]
    fn test_expired_token() {
        let keypair = TestKeyPair::generate();
        let token = create_expired_token(
            &keypair,
            "https://test.example.com",
            "test-audience",
            "user123",
        );

        let result = keypair.verify_token(&token);
        assert!(result.is_err());
    }
}
