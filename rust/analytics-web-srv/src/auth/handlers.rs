//! Axum request/response glue: the auth endpoints, middleware, and extractors.

use super::claims::{
    CookieTokenRequestParts, UserInfo, ValidatedUser, extract_name_from_token,
    extract_subject_from_token,
};
use super::cookies::{
    ID_TOKEN_COOKIE, OAUTH_STATE_COOKIE, REFRESH_TOKEN_COOKIE, clear_cookie, create_cookie,
};
use super::state::AuthState;
use axum::{
    Json,
    extract::{FromRequestParts, Query, Request, State},
    http::{StatusCode, request::Parts},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use micromegas::auth::oauth_state::{OAuthState, generate_nonce, sign_state, verify_state};
use micromegas::auth::oidc::create_http_client;
use micromegas::auth::types::AuthProvider;
use micromegas::auth::url_validation::validate_return_url;
use micromegas::tracing::prelude::*;
use openidconnect::{
    AuthenticationFlow, CsrfToken, Nonce, PkceCodeChallenge, Scope, core::CoreResponseType,
};
use serde::Deserialize;

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
    let client = provider.build_client();

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

    // Get OIDC provider
    let provider = state
        .get_oidc_provider()
        .await
        .map_err(|e| AuthApiError::Internal(format!("Failed to get OIDC provider: {e:?}")))?;

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

    // Get OIDC provider
    let provider = state
        .get_oidc_provider()
        .await
        .map_err(|e| AuthApiError::Internal(format!("Failed to get OIDC provider: {e:?}")))?;

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
        is_admin: auth_context.is_admin,
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
                error!("Auth internal error: {msg}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
            }
        };

        (status, message).into_response()
    }
}

/// Cookie-based authentication middleware with full JWT signature validation
///
/// Reads the ID token from the Authorization: Bearer header (preferred) or
/// httpOnly cookie (fallback), validates the signature using JWKS,
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
    // Check Authorization: Bearer header first, then fall back to cookie
    let id_token = if let Some(bearer) = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    {
        bearer.to_string()
    } else {
        // Fall back to cookie
        let jar = CookieJar::from_headers(req.headers());
        jar.get(ID_TOKEN_COOKIE)
            .ok_or_else(|| {
                debug!("no Authorization header or id_token cookie found");
                AuthApiError::Unauthorized
            })?
            .value()
            .to_string()
    };

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

    // Log successful authentication (trace level to avoid noise on every request)
    trace!(
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

/// Returned by `require_admin` when the user is not an admin.
///
/// Implements `IntoResponse` as 403 with a JSON `{ code, message }` body
/// matching the data-sources error shape, so handlers can `?`-propagate
/// it directly or convert into a domain error enum at the boundary.
#[derive(Debug)]
pub struct AdminRequired;

impl IntoResponse for AdminRequired {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "code": "FORBIDDEN",
            "message": "Admin access required",
        });
        (StatusCode::FORBIDDEN, Json(body)).into_response()
    }
}

/// Returns `Ok(())` when the user is an admin, otherwise an `AdminRequired`
/// error that renders as a 403 with `{ code: "FORBIDDEN", message: ... }`.
pub fn require_admin(user: &ValidatedUser) -> Result<(), AdminRequired> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AdminRequired)
    }
}

/// Extractor that yields the validated user only when `is_admin` is true.
///
/// Why: `FromRequestParts` runs before any body extractor (which is
/// `FromRequest`), so handlers that take a `Bytes` upload body can use
/// this to short-circuit with 403 *before* the body is buffered into
/// memory. Doing the admin check inside the handler body would mean a
/// non-admin authenticated user could force the server to buffer up to
/// `DefaultBodyLimit` bytes per request before getting rejected.
pub struct AdminUser(pub ValidatedUser);

impl<S: Send + Sync> FromRequestParts<S> for AdminUser {
    type Rejection = AdminRequired;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let user = parts
            .extensions
            .get::<ValidatedUser>()
            .cloned()
            .ok_or(AdminRequired)?;
        if user.is_admin {
            Ok(AdminUser(user))
        } else {
            Err(AdminRequired)
        }
    }
}
