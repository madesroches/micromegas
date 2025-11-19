//! OAuth state parameter signing and verification
//!
//! Provides HMAC-SHA256 signing for OAuth state parameters to prevent CSRF attacks
//! by ensuring the state parameter cannot be tampered with during the OAuth flow.
//!
//! # Security
//!
//! The state parameter is signed with HMAC-SHA256 to prevent attackers from:
//! - Modifying the return_url to redirect users to malicious sites
//! - Tampering with the PKCE verifier
//! - Forging nonce values
//!
//! # Format
//!
//! Signed state: `base64url(state_json).base64url(hmac_signature)`
//!
//! # Example
//!
//! ```rust
//! use micromegas_auth::oauth_state::{OAuthState, sign_state, verify_state};
//!
//! let state = OAuthState {
//!     nonce: "random-nonce".to_string(),
//!     return_url: "/dashboard".to_string(),
//!     pkce_verifier: "pkce-verifier".to_string(),
//! };
//!
//! let secret = b"your-32-byte-secret-key-here!!!";
//! let signed = sign_state(&state, secret).expect("signing failed");
//!
//! let verified = verify_state(&signed, secret).expect("verification failed");
//! assert_eq!(verified.return_url, "/dashboard");
//! ```

use anyhow::{Result, anyhow};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// Type alias for HMAC-SHA256
type HmacSha256 = Hmac<Sha256>;

/// OAuth state stored in the state parameter
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OAuthState {
    /// CSRF nonce for validation
    pub nonce: String,
    /// URL to redirect to after successful authentication
    pub return_url: String,
    /// PKCE code verifier for OAuth PKCE flow
    pub pkce_verifier: String,
}

/// Sign OAuth state parameter with HMAC-SHA256 to prevent tampering
///
/// Returns: base64url(state_json).base64url(hmac_signature)
///
/// # Arguments
///
/// * `state` - The OAuth state to sign
/// * `secret` - Secret key for HMAC (recommended: 32 bytes)
///
/// # Example
///
/// ```rust
/// use micromegas_auth::oauth_state::{OAuthState, sign_state};
///
/// let state = OAuthState {
///     nonce: "random-nonce".to_string(),
///     return_url: "/dashboard".to_string(),
///     pkce_verifier: "pkce-verifier".to_string(),
/// };
///
/// let secret = b"your-32-byte-secret-key-here!!!";
/// let signed = sign_state(&state, secret).expect("signing failed");
/// ```
pub fn sign_state(state: &OAuthState, secret: &[u8]) -> Result<String> {
    let state_json = serde_json::to_string(state)?;

    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|e| anyhow!("Failed to create HMAC: {e}"))?;
    mac.update(state_json.as_bytes());
    let signature = mac.finalize().into_bytes();

    let signed = format!(
        "{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&state_json),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&signature)
    );
    Ok(signed)
}

/// Verify and decode signed OAuth state parameter
///
/// Validates HMAC signature and returns the decoded state
///
/// # Arguments
///
/// * `signed_state` - The signed state string (base64url(json).base64url(signature))
/// * `secret` - Secret key used for HMAC (must match signing secret)
///
/// # Example
///
/// ```rust
/// use micromegas_auth::oauth_state::{OAuthState, sign_state, verify_state};
///
/// let state = OAuthState {
///     nonce: "random-nonce".to_string(),
///     return_url: "/dashboard".to_string(),
///     pkce_verifier: "pkce-verifier".to_string(),
/// };
///
/// let secret = b"your-32-byte-secret-key-here!!!";
/// let signed = sign_state(&state, secret).expect("signing failed");
/// let verified = verify_state(&signed, secret).expect("verification failed");
///
/// assert_eq!(verified.nonce, "random-nonce");
/// assert_eq!(verified.return_url, "/dashboard");
/// ```
pub fn verify_state(signed_state: &str, secret: &[u8]) -> Result<OAuthState> {
    let parts: Vec<&str> = signed_state.split('.').collect();
    if parts.len() != 2 {
        return Err(anyhow!(
            "Invalid state format: expected 2 parts, got {}",
            parts.len()
        ));
    }

    // Decode state JSON
    let state_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[0])?;

    // Decode signature
    let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1])?;

    // Verify HMAC signature
    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|e| anyhow!("Failed to create HMAC: {e}"))?;
    mac.update(&state_bytes);
    mac.verify_slice(&signature_bytes)
        .map_err(|_| anyhow!("HMAC signature verification failed"))?;

    // Deserialize state
    Ok(serde_json::from_slice(&state_bytes)?)
}
