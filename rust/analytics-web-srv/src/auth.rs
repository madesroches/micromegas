//! Authentication endpoints for analytics-web-srv
//!
//! Implements OIDC authorization code flow with PKCE:
//! - /auth/login - Initiate OIDC login
//! - /auth/callback - Handle OIDC callback
//! - /auth/refresh - Refresh tokens
//! - /auth/logout - Clear session
//! - /auth/me - Get current user info

use anyhow::{Result, anyhow};
use axum::{
    Json,
    extract::{Query, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::Engine;
use chrono::Utc;
use micromegas::tracing::prelude::*;
use micromegas_auth::url_validation::validate_return_url;
use openidconnect::{
    AuthenticationFlow, CsrfToken, Nonce, PkceCodeChallenge, Scope,
    core::{CoreProviderMetadata, CoreResponseType},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Type alias for the OIDC client with endpoints set from provider metadata
type ConfiguredCoreClient = openidconnect::Client<
    openidconnect::EmptyAdditionalClaims,
    openidconnect::core::CoreAuthDisplay,
    openidconnect::core::CoreGenderClaim,
    openidconnect::core::CoreJweContentEncryptionAlgorithm,
    openidconnect::core::CoreJsonWebKey,
    openidconnect::core::CoreAuthPrompt,
    openidconnect::StandardErrorResponse<openidconnect::core::CoreErrorResponseType>,
    openidconnect::core::CoreTokenResponse,
    openidconnect::core::CoreTokenIntrospectionResponse,
    openidconnect::core::CoreRevocableToken,
    openidconnect::core::CoreRevocationErrorResponse,
    openidconnect::EndpointSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointMaybeSet,
    openidconnect::EndpointMaybeSet,
>;

/// OIDC client configuration
#[derive(Debug, Clone, Deserialize)]
pub struct OidcClientConfig {
    /// OIDC provider issuer URL
    pub issuer: String,
    /// Client ID (public client)
    pub client_id: String,
    /// Redirect URI for callback
    pub redirect_uri: String,
}

impl OidcClientConfig {
    /// Load configuration from environment variables
    ///
    /// Required environment variables:
    /// - MICROMEGAS_OIDC_CONFIG: JSON with "issuers" array (same format as FlightSQL server)
    /// - MICROMEGAS_AUTH_REDIRECT_URI: OAuth callback URL
    ///
    /// Expected MICROMEGAS_OIDC_CONFIG format (uses micromegas-auth's OidcConfig):
    /// {
    ///   "issuers": [
    ///     {
    ///       "issuer": "https://...",
    ///       "audience": "client-id"
    ///     }
    ///   ]
    /// }
    ///
    /// Note: The web app only supports a single issuer. If multiple issuers are
    /// configured in the array, this function will return an error. The `audience`
    /// field serves as the OAuth client_id for the web app.
    pub fn from_env() -> Result<Self> {
        // Use the shared OidcConfig from micromegas-auth
        let config = micromegas_auth::oidc::OidcConfig::from_env()?;

        // Ensure exactly one issuer is configured
        if config.issuers.is_empty() {
            return Err(anyhow!(
                "MICROMEGAS_OIDC_CONFIG must contain at least one issuer in the 'issuers' array"
            ));
        }

        if config.issuers.len() > 1 {
            return Err(anyhow!(
                "Analytics web app only supports a single OIDC issuer. Found {} issuers in MICROMEGAS_OIDC_CONFIG. \
                 Please configure only one issuer in the 'issuers' array.",
                config.issuers.len()
            ));
        }

        let issuer_config = &config.issuers[0];

        let redirect_uri = std::env::var("MICROMEGAS_AUTH_REDIRECT_URI")
            .map_err(|_| anyhow!("MICROMEGAS_AUTH_REDIRECT_URI environment variable not set"))?;

        Ok(OidcClientConfig {
            issuer: issuer_config.issuer.clone(),
            client_id: issuer_config.audience.clone(),
            redirect_uri,
        })
    }
}

/// OIDC provider metadata cached
#[derive(Clone)]
pub struct OidcProviderInfo {
    pub metadata: Arc<CoreProviderMetadata>,
    pub client_id: openidconnect::ClientId,
    pub redirect_uri: openidconnect::RedirectUrl,
}

/// State for auth endpoints
#[derive(Clone)]
pub struct AuthState {
    /// OIDC provider info (lazy initialized)
    pub oidc_provider: Arc<tokio::sync::OnceCell<OidcProviderInfo>>,
    /// OIDC client configuration
    pub config: OidcClientConfig,
    /// Cookie domain (optional)
    pub cookie_domain: Option<String>,
    /// Whether we're in production (secure cookies)
    pub secure_cookies: bool,
}

/// Create HTTP client for OIDC operations
fn create_http_client() -> Result<reqwest::Client> {
    reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| anyhow!("Failed to create HTTP client: {e:?}"))
}

impl AuthState {
    pub async fn get_oidc_provider(&self) -> Result<&OidcProviderInfo> {
        let config = self.config.clone();
        self.oidc_provider
            .get_or_try_init(|| async move {
                let issuer_url = openidconnect::IssuerUrl::new(config.issuer.clone())
                    .map_err(|e| anyhow!("Invalid issuer URL: {e:?}"))?;

                let http_client = create_http_client()?;
                let provider_metadata =
                    CoreProviderMetadata::discover_async(issuer_url, &http_client)
                        .await
                        .map_err(|e| anyhow!("Failed to discover OIDC provider: {e:?}"))?;

                let client_id = openidconnect::ClientId::new(config.client_id.clone());
                let redirect_uri = openidconnect::RedirectUrl::new(config.redirect_uri.clone())
                    .map_err(|e| anyhow!("Invalid redirect URI: {e:?}"))?;

                Ok(OidcProviderInfo {
                    metadata: Arc::new(provider_metadata),
                    client_id,
                    redirect_uri,
                })
            })
            .await
    }

    pub fn build_oidc_client(&self, provider: &OidcProviderInfo) -> ConfiguredCoreClient {
        openidconnect::core::CoreClient::from_provider_metadata(
            (*provider.metadata).clone(),
            provider.client_id.clone(),
            None, // No client secret (public client with PKCE)
        )
        .set_redirect_uri(provider.redirect_uri.clone())
    }
}

/// Query parameters for login endpoint
#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    /// Return URL after successful login
    return_url: Option<String>,
}

/// State stored in OAuth state parameter
#[derive(Debug, Serialize, Deserialize)]
struct OAuthState {
    /// CSRF nonce for validation
    nonce: String,
    /// URL to redirect to after login
    return_url: String,
    /// PKCE code verifier
    pkce_verifier: String,
}

/// Query parameters for callback endpoint
#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    /// Authorization code from OIDC provider
    code: String,
    /// State parameter (contains nonce and return_url)
    state: String,
}

/// User info response
#[derive(Debug, Serialize)]
pub struct UserInfo {
    sub: String,
    email: Option<String>,
    name: Option<String>,
}

/// JWT claims for decoding (minimal)
#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    exp: i64,
}

/// Cookie names
const ID_TOKEN_COOKIE: &str = "id_token"; // ID token (JWT) for user info and FlightSQL API authorization
const REFRESH_TOKEN_COOKIE: &str = "refresh_token";
const OAUTH_STATE_COOKIE: &str = "oauth_state";

/// Generate a random nonce
fn generate_nonce() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.r#gen();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Create a cookie with common settings
fn create_cookie<'a>(
    name: &'a str,
    value: String,
    max_age_secs: i64,
    state: &AuthState,
) -> Cookie<'a> {
    let mut cookie = Cookie::build((name, value))
        .http_only(true)
        .secure(state.secure_cookies)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::seconds(max_age_secs));

    if let Some(domain) = &state.cookie_domain {
        cookie = cookie.domain(domain.clone());
    }

    cookie.build()
}

/// Create an expired cookie to clear it
fn clear_cookie<'a>(name: &'a str, state: &AuthState) -> Cookie<'a> {
    let mut cookie = Cookie::build((name, ""))
        .http_only(true)
        .secure(state.secure_cookies)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::seconds(0));

    if let Some(domain) = &state.cookie_domain {
        cookie = cookie.domain(domain.clone());
    }

    cookie.build()
}

/// GET /auth/login - Initiate OIDC login
#[span_fn]
pub async fn auth_login(
    State(state): State<AuthState>,
    Query(query): Query<LoginQuery>,
) -> Result<impl IntoResponse, AuthApiError> {
    let return_url = query.return_url.unwrap_or_else(|| "/".to_string());

    // Validate return URL to prevent open redirect
    if !validate_return_url(&return_url) {
        return Err(AuthApiError::InvalidReturnUrl);
    }

    // Generate PKCE challenge
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    // Generate state with nonce and return URL
    let nonce = generate_nonce();
    let oauth_state = OAuthState {
        nonce: nonce.clone(),
        return_url,
        pkce_verifier: pkce_verifier.secret().to_string(),
    };
    let state_json = serde_json::to_string(&oauth_state)
        .map_err(|e| AuthApiError::Internal(format!("Failed to serialize state: {e:?}")))?;
    let state_encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(state_json);

    // Get OIDC provider and build client
    let provider = state
        .get_oidc_provider()
        .await
        .map_err(|e| AuthApiError::Internal(format!("Failed to get OIDC provider: {e:?}")))?;
    let client = state.build_oidc_client(provider);

    // Generate authorization URL
    let (auth_url, _csrf_token, _nonce) = client
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            move || CsrfToken::new(state_encoded.clone()),
            Nonce::new_random,
        )
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .add_scope(Scope::new("offline_access".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    // Set cookie with nonce for validation
    let cookie = create_cookie(OAUTH_STATE_COOKIE, nonce, 600, &state); // 10 minutes

    Ok((
        CookieJar::new().add(cookie),
        Redirect::temporary(auth_url.as_str()),
    ))
}

/// GET /auth/callback - Handle OIDC callback
#[span_fn]
pub async fn auth_callback(
    State(state): State<AuthState>,
    jar: CookieJar,
    Query(query): Query<CallbackQuery>,
) -> Result<impl IntoResponse, AuthApiError> {
    // Decode state parameter
    let state_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(query.state.as_bytes())
        .map_err(|e| {
            warn!("failed to decode state: {e:?}");
            AuthApiError::InvalidState
        })?;
    let oauth_state: OAuthState =
        serde_json::from_slice(&state_bytes).map_err(|_| AuthApiError::InvalidState)?;

    // Validate nonce from cookie
    let cookie_nonce = jar
        .get(OAUTH_STATE_COOKIE)
        .ok_or_else(|| {
            warn!("oauth_state cookie not found");
            AuthApiError::InvalidState
        })?
        .value();

    if cookie_nonce != oauth_state.nonce {
        warn!("nonce mismatch!");
        return Err(AuthApiError::InvalidState);
    }

    // Get OIDC provider and build client
    let provider = state
        .get_oidc_provider()
        .await
        .map_err(|e| AuthApiError::Internal(format!("Failed to get OIDC provider: {e:?}")))?;
    let _client = state.build_oidc_client(provider);

    // Exchange code for tokens using manual HTTP request
    // Note: We don't use the openidconnect library's exchange_code() because:
    // - Auth0 includes non-standard fields (e.g., updated_at) that cause parsing failures
    // - The library's strict typing doesn't handle provider-specific extensions well
    // - Manual HTTP gives us better error visibility and control over parsing
    let http_client = create_http_client()
        .map_err(|e| AuthApiError::Internal(format!("Failed to create HTTP client: {e:?}")))?;

    let token_url = provider
        .metadata
        .token_endpoint()
        .expect("token endpoint should exist");

    let params = [
        ("grant_type", "authorization_code"),
        ("code", &query.code),
        ("redirect_uri", &state.config.redirect_uri),
        ("client_id", &state.config.client_id),
        ("code_verifier", &oauth_state.pkce_verifier),
    ];

    // Note: Generic error messages are intentional to avoid leaking authentication details
    // Detailed errors are logged server-side for debugging
    let response = http_client
        .post(token_url.as_str())
        .form(&params)
        .send()
        .await
        .map_err(|e| {
            warn!("token exchange HTTP request failed: {e:?}");
            AuthApiError::TokenExchangeFailed
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "unknown".to_string());
        warn!("token exchange failed with status {status}: {body}");
        return Err(AuthApiError::TokenExchangeFailed);
    }

    let token_response: serde_json::Value = response.json().await.map_err(|e| {
        warn!("failed to parse token response: {e:?}");
        AuthApiError::TokenExchangeFailed
    })?;

    // Extract tokens from JSON response
    let id_token = token_response["id_token"]
        .as_str()
        .ok_or_else(|| {
            warn!("no id_token in response");
            AuthApiError::TokenExchangeFailed
        })?
        .to_string();

    let refresh_token = token_response["refresh_token"]
        .as_str()
        .map(|s| s.to_string());

    // Calculate expiration times
    let access_token_expires = token_response["expires_in"]
        .as_u64()
        .map(|d| d as i64)
        .unwrap_or(3600); // Default 1 hour

    let refresh_token_expires = 30 * 24 * 3600; // 30 days

    // Create cookies
    let mut new_jar = jar;
    new_jar = new_jar.add(create_cookie(
        ID_TOKEN_COOKIE,
        id_token,
        access_token_expires,
        &state,
    ));

    if let Some(refresh) = refresh_token {
        new_jar = new_jar.add(create_cookie(
            REFRESH_TOKEN_COOKIE,
            refresh,
            refresh_token_expires,
            &state,
        ));
    }

    // Clear oauth state cookie
    new_jar = new_jar.add(clear_cookie(OAUTH_STATE_COOKIE, &state));

    // Redirect to return URL
    Ok((new_jar, Redirect::temporary(&oauth_state.return_url)))
}

/// POST /auth/refresh - Refresh tokens
#[span_fn]
pub async fn auth_refresh(
    State(state): State<AuthState>,
    jar: CookieJar,
) -> Result<impl IntoResponse, AuthApiError> {
    // Get refresh token from cookie
    let refresh_token = jar
        .get(REFRESH_TOKEN_COOKIE)
        .ok_or(AuthApiError::Unauthorized)?
        .value()
        .to_string();

    // Get OIDC provider and build client
    let provider = state
        .get_oidc_provider()
        .await
        .map_err(|e| AuthApiError::Internal(format!("Failed to get OIDC provider: {e:?}")))?;
    let _client = state.build_oidc_client(provider);

    // Exchange refresh token for new tokens using manual HTTP request
    // Note: Same reasoning as auth_callback - Auth0's non-standard fields break library parsing
    let http_client = create_http_client()
        .map_err(|e| AuthApiError::Internal(format!("Failed to create HTTP client: {e:?}")))?;

    let token_url = provider
        .metadata
        .token_endpoint()
        .expect("token endpoint should exist");

    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", &refresh_token),
        ("client_id", &state.config.client_id),
    ];

    let response = http_client
        .post(token_url.as_str())
        .form(&params)
        .send()
        .await
        .map_err(|e| {
            warn!("refresh token HTTP request failed: {e:?}");
            AuthApiError::Unauthorized
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "unknown".to_string());
        warn!("refresh token failed with status {status}: {body}");
        return Err(AuthApiError::Unauthorized);
    }

    let token_response: serde_json::Value = response.json().await.map_err(|e| {
        warn!("failed to parse refresh token response: {e:?}");
        AuthApiError::Unauthorized
    })?;

    // Extract new tokens from JSON response
    let id_token = token_response["id_token"]
        .as_str()
        .ok_or_else(|| {
            warn!("missing id_token in refresh response");
            AuthApiError::Unauthorized
        })?
        .to_string();

    let refresh_token = token_response["refresh_token"]
        .as_str()
        .map(|s| s.to_string());

    // Calculate expiration times
    let id_token_expires = token_response["expires_in"]
        .as_u64()
        .map(|d| d as i64)
        .unwrap_or(3600);

    let refresh_token_expires = 30 * 24 * 3600; // 30 days

    // Update cookies
    let mut new_jar = jar;
    new_jar = new_jar.add(create_cookie(
        ID_TOKEN_COOKIE,
        id_token,
        id_token_expires,
        &state,
    ));

    if let Some(refresh) = refresh_token {
        new_jar = new_jar.add(create_cookie(
            REFRESH_TOKEN_COOKIE,
            refresh,
            refresh_token_expires,
            &state,
        ));
    }

    Ok((new_jar, StatusCode::OK))
}

/// POST /auth/logout - Clear session
#[span_fn]
pub async fn auth_logout(State(state): State<AuthState>, jar: CookieJar) -> impl IntoResponse {
    let new_jar = jar
        .add(clear_cookie(ID_TOKEN_COOKIE, &state))
        .add(clear_cookie(REFRESH_TOKEN_COOKIE, &state));

    (new_jar, StatusCode::OK)
}

/// GET /auth/me - Get current user info
/// Reads the ID token (JWT) to extract user information
#[span_fn]
pub async fn auth_me(jar: CookieJar) -> Result<Json<UserInfo>, AuthApiError> {
    // Get ID token from cookie
    let id_token = jar
        .get(ID_TOKEN_COOKIE)
        .ok_or_else(|| {
            warn!("no id_token cookie found");
            AuthApiError::Unauthorized
        })?
        .value();

    // Decode JWT payload (no validation needed here, just extract claims)
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        warn!(
            "invalid token format: expected 3 parts, got {}",
            parts.len()
        );
        return Err(AuthApiError::InvalidToken);
    }

    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .map_err(|e| {
            warn!("failed to decode token payload: {}", e);
            AuthApiError::InvalidToken
        })?;

    let claims: IdTokenClaims = serde_json::from_slice(&payload_bytes).map_err(|e| {
        warn!("failed to parse token claims: {}", e);
        AuthApiError::InvalidToken
    })?;

    // Check expiration
    let now = Utc::now().timestamp();
    if claims.exp < now {
        warn!("token expired: exp={} now={}", claims.exp, now);
        return Err(AuthApiError::Unauthorized);
    }

    Ok(Json(UserInfo {
        sub: claims.sub,
        email: claims.email.or(claims.preferred_username),
        name: claims.name,
    }))
}

/// Authentication API errors
#[derive(Debug)]
pub enum AuthApiError {
    InvalidReturnUrl,
    InvalidState,
    TokenExchangeFailed,
    Unauthorized,
    InvalidToken,
    Internal(String),
}

impl IntoResponse for AuthApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthApiError::InvalidReturnUrl => (StatusCode::BAD_REQUEST, "Invalid return URL"),
            AuthApiError::InvalidState => (StatusCode::BAD_REQUEST, "Invalid OAuth state"),
            AuthApiError::TokenExchangeFailed => {
                (StatusCode::UNAUTHORIZED, "Token exchange failed")
            }
            AuthApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            AuthApiError::InvalidToken => (StatusCode::UNAUTHORIZED, "Invalid token"),
            AuthApiError::Internal(msg) => {
                tracing::error!("Auth internal error: {msg}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
            }
        };

        (status, message).into_response()
    }
}

/// Basic JWT validation (format and expiration only, no signature check)
///
/// Security note: This web server acts as a proxy and has no direct access to
/// telemetry data. All data queries are forwarded to the FlightSQL service,
/// which performs full JWT signature validation with the OIDC provider. This
/// basic validation provides fast feedback for obviously invalid tokens while
/// delegating the authoritative security checks to the data-owning service.
fn validate_jwt_basic(token: &str) -> Result<(), AuthApiError> {
    // Check JWT format: must have 3 parts (header.payload.signature)
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        warn!("invalid JWT format: expected 3 parts, got {}", parts.len());
        return Err(AuthApiError::InvalidToken);
    }

    // Decode and parse payload to check expiration
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .map_err(|e| {
            warn!("failed to decode JWT payload: {}", e);
            AuthApiError::InvalidToken
        })?;

    let claims: IdTokenClaims = serde_json::from_slice(&payload_bytes).map_err(|e| {
        warn!("failed to parse JWT claims: {}", e);
        AuthApiError::InvalidToken
    })?;

    // Check expiration
    let now = Utc::now().timestamp();
    if claims.exp < now {
        warn!("JWT expired: exp={} now={}", claims.exp, now);
        return Err(AuthApiError::Unauthorized);
    }

    Ok(())
}

/// Cookie-based authentication middleware
///
/// Reads the ID token from httpOnly cookie and validates it.
/// Injects the token into request extensions for downstream handlers.
///
/// Note: We use the ID token (JWT) for FlightSQL API calls because:
/// - ID tokens can be validated locally by FlightSQL's OIDC provider
/// - This matches the Python API behavior which also uses ID tokens
/// - Access tokens (JWE) would require token introspection endpoints
///
/// Validation strategy:
/// - Basic checks (format, expiration) done here for fast feedback
/// - Full signature validation delegated to FlightSQL service (authoritative)
#[span_fn]
pub async fn cookie_auth_middleware(req: Request, next: Next) -> Result<Response, AuthApiError> {
    // Extract cookies from request
    let jar = CookieJar::from_headers(req.headers());

    // Get ID token from cookie (JWT format for FlightSQL API calls)
    let id_token = jar
        .get(ID_TOKEN_COOKIE)
        .ok_or_else(|| {
            warn!("id_token cookie not found");
            AuthApiError::Unauthorized
        })?
        .value()
        .to_string();

    // Basic validation: format and expiration
    // (FlightSQL will do full signature validation)
    validate_jwt_basic(&id_token)?;

    // Store token in request extensions for downstream use (FlightSQL API calls)
    let mut req = req;
    req.extensions_mut().insert(AuthToken(id_token));

    // Continue to next middleware/handler
    Ok(next.run(req).await)
}

/// Wrapper for the authenticated user's token
/// Will be used to pass token to FlightSQL in future phases
#[derive(Clone, Debug)]
pub struct AuthToken(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_nonce_uniqueness() {
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();
        assert_ne!(nonce1, nonce2);
    }

    #[test]
    fn test_generate_nonce_length() {
        let nonce = generate_nonce();
        // 32 bytes base64 encoded should be 43 characters (URL_SAFE_NO_PAD)
        assert_eq!(nonce.len(), 43);
    }

    #[test]
    fn test_generate_nonce_valid_base64() {
        let nonce = generate_nonce();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&nonce);
        assert!(decoded.is_ok());
        assert_eq!(decoded.expect("should decode").len(), 32);
    }

    #[test]
    fn test_create_cookie_basic_properties() {
        let state = AuthState {
            oidc_provider: Arc::new(tokio::sync::OnceCell::new()),
            config: OidcClientConfig {
                issuer: "https://issuer.example.com".to_string(),
                client_id: "test-client".to_string(),
                redirect_uri: "http://localhost:3000/auth/callback".to_string(),
            },
            cookie_domain: None,
            secure_cookies: false,
        };

        let cookie = create_cookie("test_cookie", "test_value".to_string(), 3600, &state);
        assert_eq!(cookie.name(), "test_cookie");
        assert_eq!(cookie.value(), "test_value");
        assert!(cookie.http_only().unwrap_or(false));
        assert_eq!(cookie.path().unwrap_or(""), "/");
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
    }

    #[test]
    fn test_create_cookie_secure_flag() {
        let state = AuthState {
            oidc_provider: Arc::new(tokio::sync::OnceCell::new()),
            config: OidcClientConfig {
                issuer: "https://issuer.example.com".to_string(),
                client_id: "test-client".to_string(),
                redirect_uri: "http://localhost:3000/auth/callback".to_string(),
            },
            cookie_domain: None,
            secure_cookies: true,
        };

        let cookie = create_cookie("secure_cookie", "value".to_string(), 3600, &state);
        assert!(cookie.secure().unwrap_or(false));
    }

    #[test]
    fn test_create_cookie_with_domain() {
        let state = AuthState {
            oidc_provider: Arc::new(tokio::sync::OnceCell::new()),
            config: OidcClientConfig {
                issuer: "https://issuer.example.com".to_string(),
                client_id: "test-client".to_string(),
                redirect_uri: "http://localhost:3000/auth/callback".to_string(),
            },
            cookie_domain: Some(".example.com".to_string()),
            secure_cookies: false,
        };

        let cookie = create_cookie("domain_cookie", "value".to_string(), 3600, &state);
        // Cookie library strips leading dot from domain
        assert_eq!(cookie.domain(), Some("example.com"));
    }

    #[test]
    fn test_clear_cookie_expires_immediately() {
        let state = AuthState {
            oidc_provider: Arc::new(tokio::sync::OnceCell::new()),
            config: OidcClientConfig {
                issuer: "https://issuer.example.com".to_string(),
                client_id: "test-client".to_string(),
                redirect_uri: "http://localhost:3000/auth/callback".to_string(),
            },
            cookie_domain: None,
            secure_cookies: false,
        };

        let cookie = clear_cookie("expired_cookie", &state);
        assert_eq!(cookie.name(), "expired_cookie");
        assert_eq!(cookie.value(), "");
        assert_eq!(cookie.max_age(), Some(time::Duration::seconds(0)));
    }

    #[test]
    fn test_auth_api_error_status_codes() {
        use axum::response::IntoResponse;
        use http::StatusCode;

        let invalid_url_resp = AuthApiError::InvalidReturnUrl.into_response();
        assert_eq!(invalid_url_resp.status(), StatusCode::BAD_REQUEST);

        let invalid_state_resp = AuthApiError::InvalidState.into_response();
        assert_eq!(invalid_state_resp.status(), StatusCode::BAD_REQUEST);

        let token_failed_resp = AuthApiError::TokenExchangeFailed.into_response();
        assert_eq!(token_failed_resp.status(), StatusCode::UNAUTHORIZED);

        let unauthorized_resp = AuthApiError::Unauthorized.into_response();
        assert_eq!(unauthorized_resp.status(), StatusCode::UNAUTHORIZED);

        let invalid_token_resp = AuthApiError::InvalidToken.into_response();
        assert_eq!(invalid_token_resp.status(), StatusCode::UNAUTHORIZED);

        let internal_resp = AuthApiError::Internal("test error".to_string()).into_response();
        assert_eq!(internal_resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
