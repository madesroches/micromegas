//! Authentication endpoints for analytics-web-srv
//!
//! Implements OIDC authorization code flow with PKCE:
//! - /auth/login - Initiate OIDC login
//! - /auth/callback - Handle OIDC callback
//! - /auth/refresh - Refresh tokens
//! - /auth/logout - Clear session
//! - /auth/me - Get current user info
//!
//! Security: All JWT tokens are fully validated (signature + claims) at this tier
//! using the micromegas-auth crate with JWKS caching. Invalid tokens are rejected
//! before forwarding requests to FlightSQL.

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
use micromegas::tracing::prelude::*;
use micromegas_auth::oauth_state::{OAuthState, generate_nonce, sign_state, verify_state};
use micromegas_auth::oidc::{OidcAuthProvider, OidcConfig, create_http_client};
use micromegas_auth::types::{AuthContext, AuthProvider};
use micromegas_auth::url_validation::validate_return_url;
use openidconnect::{
    AuthenticationFlow, CsrfToken, Nonce, PkceCodeChallenge, Scope,
    core::{CoreProviderMetadata, CoreResponseType},
};
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
    /// Note: When multiple issuers are configured, the first issuer is used for
    /// the OAuth login flow (you can only redirect to one provider). Token
    /// validation via OidcAuthProvider will accept tokens from any configured issuer.
    pub fn from_env() -> Result<Self> {
        // Use the shared OidcConfig from micromegas-auth
        let config = micromegas_auth::oidc::OidcConfig::from_env()?;

        // Need at least one issuer
        if config.issuers.is_empty() {
            return Err(anyhow!(
                "MICROMEGAS_OIDC_CONFIG must contain at least one issuer in the 'issuers' array"
            ));
        }

        // Use the first issuer for OAuth login flow
        // (token validation via OidcAuthProvider supports all issuers)
        let issuer_config = &config.issuers[0];

        if config.issuers.len() > 1 {
            info!(
                "Multiple OIDC issuers configured ({}). Using '{}' for OAuth login flow. \
                 Token validation will accept tokens from all configured issuers.",
                config.issuers.len(),
                issuer_config.issuer
            );
        }

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
    /// OIDC provider info (lazy initialized) - for OAuth flow
    pub oidc_provider: Arc<tokio::sync::OnceCell<OidcProviderInfo>>,
    /// OIDC auth provider (lazy initialized) - for JWT validation
    pub auth_provider: Arc<tokio::sync::OnceCell<Arc<OidcAuthProvider>>>,
    /// OIDC client configuration
    pub config: OidcClientConfig,
    /// Cookie domain (optional)
    pub cookie_domain: Option<String>,
    /// Whether we're in production (secure cookies)
    pub secure_cookies: bool,
    /// Secret for signing OAuth state parameters (HMAC-SHA256)
    pub state_signing_secret: Vec<u8>,
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

    /// Get or initialize the OIDC auth provider for JWT validation
    ///
    /// The auth provider is lazy-initialized on first use and cached.
    /// It uses the MICROMEGAS_OIDC_CONFIG environment variable for configuration,
    /// which is the same format used by the FlightSQL server.
    pub async fn get_auth_provider(&self) -> Result<&Arc<OidcAuthProvider>> {
        self.auth_provider
            .get_or_try_init(|| async {
                let config = OidcConfig::from_env()?;
                let provider = OidcAuthProvider::new(config).await?;
                Ok(Arc::new(provider))
            })
            .await
    }
}

/// Query parameters for login endpoint
#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    /// Return URL after successful login
    return_url: Option<String>,
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

/// JWT claims for decoding (minimal) - used for auth_me name extraction
#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    #[serde(default)]
    name: Option<String>,
}

/// Validated user information extracted from JWT after signature verification
///
/// This struct is inserted into request extensions by the auth middleware
/// and can be used by handlers to access validated user information.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Fields are public for use by handlers accessing request extensions
pub struct ValidatedUser {
    /// Unique subject identifier (user ID)
    pub subject: String,
    /// Email address (if available)
    pub email: Option<String>,
    /// Token issuer URL
    pub issuer: String,
    /// Whether this user has admin privileges
    pub is_admin: bool,
}

impl From<&AuthContext> for ValidatedUser {
    fn from(ctx: &AuthContext) -> Self {
        Self {
            subject: ctx.subject.clone(),
            email: ctx.email.clone(),
            issuer: ctx.issuer.clone(),
            is_admin: ctx.is_admin,
        }
    }
}

/// Request parts adapter for cookie-based tokens
///
/// Adapts a cookie-based token into the RequestParts trait expected by OidcAuthProvider.
/// Used by both cookie_auth_middleware and auth_me endpoint for token validation.
struct CookieTokenRequestParts {
    token: String,
}

impl micromegas_auth::types::RequestParts for CookieTokenRequestParts {
    fn authorization_header(&self) -> Option<&str> {
        None
    }

    fn bearer_token(&self) -> Option<&str> {
        Some(&self.token)
    }

    fn get_header(&self, _name: &str) -> Option<&str> {
        None
    }

    fn method(&self) -> Option<&str> {
        None
    }

    fn uri(&self) -> Option<&str> {
        None
    }
}

/// Cookie names
const ID_TOKEN_COOKIE: &str = "id_token"; // ID token (JWT) for user info and FlightSQL API authorization
const REFRESH_TOKEN_COOKIE: &str = "refresh_token";
const OAUTH_STATE_COOKIE: &str = "oauth_state";

/// Create a cookie with common settings
pub fn create_cookie<'a>(
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
pub fn clear_cookie<'a>(name: &'a str, state: &AuthState) -> Cookie<'a> {
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
    // Sign the state with HMAC-SHA256 to prevent tampering
    let state_signed = sign_state(&oauth_state, &state.state_signing_secret)
        .map_err(|e| AuthApiError::Internal(format!("Failed to sign state: {e:?}")))?;

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
            move || CsrfToken::new(state_signed.clone()),
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
    // Verify and decode signed state parameter
    let oauth_state = verify_state(&query.state, &state.state_signing_secret).map_err(|e| {
        warn!("[auth_failure] reason=invalid_state details={e:?}");
        AuthApiError::InvalidState
    })?;

    // Validate nonce from cookie
    let cookie_nonce = jar
        .get(OAUTH_STATE_COOKIE)
        .ok_or_else(|| {
            warn!("[auth_failure] reason=missing_oauth_state_cookie");
            AuthApiError::InvalidState
        })?
        .value();

    if cookie_nonce != oauth_state.nonce {
        warn!("[auth_failure] reason=nonce_mismatch");
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
            warn!("[auth_failure] reason=token_exchange_request_failed details={e:?}");
            AuthApiError::TokenExchangeFailed
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "unknown".to_string());
        warn!("[auth_failure] reason=token_exchange_failed status={status} body={body}");
        return Err(AuthApiError::TokenExchangeFailed);
    }

    let token_response: serde_json::Value = response.json().await.map_err(|e| {
        warn!("[auth_failure] reason=token_response_parse_failed details={e:?}");
        AuthApiError::TokenExchangeFailed
    })?;

    // Extract tokens from JSON response
    let id_token = token_response["id_token"]
        .as_str()
        .ok_or_else(|| {
            warn!("[auth_failure] reason=missing_id_token");
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

    // Log successful login (extract subject from token for audit trail)
    if let Some(sub) = extract_subject_from_token(&id_token) {
        info!(
            "[auth_success] event=login sub={sub} issuer={}",
            state.config.issuer
        );
    }

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
            warn!("[token_refresh_failure] reason=request_failed details={e:?}");
            AuthApiError::Unauthorized
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "unknown".to_string());
        warn!("[token_refresh_failure] reason=token_exchange_failed status={status} body={body}");
        return Err(AuthApiError::Unauthorized);
    }

    let token_response: serde_json::Value = response.json().await.map_err(|e| {
        warn!("[token_refresh_failure] reason=response_parse_failed details={e:?}");
        AuthApiError::Unauthorized
    })?;

    // Extract new tokens from JSON response
    let id_token = token_response["id_token"]
        .as_str()
        .ok_or_else(|| {
            warn!("[token_refresh_failure] reason=missing_id_token");
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

    // Log successful token refresh
    if let Some(sub) = extract_subject_from_token(&id_token) {
        info!("[auth_success] event=token_refresh sub={sub}");
    }

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
///
/// Returns user information from the validated JWT token.
/// This endpoint must be behind the cookie_auth_middleware to ensure
/// the token has been validated before extracting user info.
///
/// The user's `sub` (subject) and `email` come from the validated token claims.
/// The `name` field is extracted directly from the JWT payload for display purposes.
#[span_fn]
pub async fn auth_me(
    State(state): State<AuthState>,
    jar: CookieJar,
) -> Result<Json<UserInfo>, AuthApiError> {
    // Get ID token from cookie
    let id_token = jar
        .get(ID_TOKEN_COOKIE)
        .ok_or_else(|| {
            debug!("no id_token cookie found");
            AuthApiError::Unauthorized
        })?
        .value()
        .to_string();

    // Validate the token using the OIDC provider
    let auth_provider = state.get_auth_provider().await.map_err(|e| {
        warn!("[auth_failure] auth_provider_init_failed: {e:?}");
        AuthApiError::Internal("Failed to initialize auth provider".to_string())
    })?;

    let parts = CookieTokenRequestParts {
        token: id_token.clone(),
    };

    let auth_context = auth_provider.validate_request(&parts).await.map_err(|e| {
        warn!("[auth_failure] {e}");
        AuthApiError::InvalidToken
    })?;

    // Extract name from JWT payload (not in AuthContext)
    // This is safe since we just validated the token
    let name = extract_name_from_token(&id_token);

    Ok(Json(UserInfo {
        sub: auth_context.subject,
        email: auth_context.email,
        name,
    }))
}

/// Extract the 'name' claim from a JWT payload
///
/// This is used by auth_me() to get the display name which isn't in AuthContext
fn extract_name_from_token(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .ok()?;

    let claims: IdTokenClaims = serde_json::from_slice(&payload_bytes).ok()?;
    claims.name
}

/// Extract the 'sub' claim from a JWT payload for audit logging
fn extract_subject_from_token(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .ok()?;

    let claims: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    claims["sub"].as_str().map(|s| s.to_string())
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
                error!("Auth internal error: {msg}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
            }
        };

        (status, message).into_response()
    }
}

/// Cookie-based authentication middleware with full JWT signature validation
///
/// Reads the ID token from httpOnly cookie, validates the signature using JWKS,
/// and injects validated user info into request extensions.
///
/// Security: This middleware performs full JWT validation including:
/// - Signature verification using cached JWKS from the OIDC provider
/// - Issuer validation against configured issuers
/// - Audience validation
/// - Expiration check
///
/// Invalid or forged tokens are rejected before any downstream processing.
///
/// Note: We use the ID token (JWT) for FlightSQL API calls because:
/// - ID tokens can be validated locally by both web tier and FlightSQL
/// - This matches the Python API behavior which also uses ID tokens
/// - Access tokens (JWE) would require token introspection endpoints
#[span_fn]
pub async fn cookie_auth_middleware(
    State(state): State<AuthState>,
    req: Request,
    next: Next,
) -> Result<Response, AuthApiError> {
    // Extract cookies from request
    let jar = CookieJar::from_headers(req.headers());

    // Get ID token from cookie (JWT format for FlightSQL API calls)
    let id_token = jar
        .get(ID_TOKEN_COOKIE)
        .ok_or_else(|| {
            debug!("id_token cookie not found");
            AuthApiError::Unauthorized
        })?
        .value()
        .to_string();

    // Get the OIDC auth provider (lazy initialized with JWKS caching)
    let auth_provider = state.get_auth_provider().await.map_err(|e| {
        warn!("[auth_failure] auth_provider_init_failed: {e:?}");
        AuthApiError::Internal("Failed to initialize auth provider".to_string())
    })?;

    // Create request parts adapter for the cookie token
    let parts = CookieTokenRequestParts {
        token: id_token.clone(),
    };

    // Validate the token with full signature verification
    let auth_context = auth_provider.validate_request(&parts).await.map_err(|e| {
        warn!("[auth_failure] {e}");
        AuthApiError::InvalidToken
    })?;

    // Log successful authentication
    info!(
        "[auth_success] subject={} email={:?} issuer={} admin={}",
        auth_context.subject, auth_context.email, auth_context.issuer, auth_context.is_admin
    );

    // Store token and validated user info in request extensions
    let mut req = req;
    req.extensions_mut().insert(AuthToken(id_token));
    req.extensions_mut()
        .insert(ValidatedUser::from(&auth_context));

    // Continue to next middleware/handler
    Ok(next.run(req).await)
}

/// Wrapper for the authenticated user's token
/// Will be used to pass token to FlightSQL in future phases
#[derive(Clone, Debug)]
pub struct AuthToken(pub String);
