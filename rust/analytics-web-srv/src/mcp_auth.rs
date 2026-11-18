//! Bearer-token validation for the `/mcp` route.
//!
//! The web app's session validator is OIDC-only and cookie-based; the MCP route accepts the same
//! credentials as FlightSQL (OIDC tokens and analytics API keys) so that a token the agent holds
//! works on both hops.

use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use micromegas::auth::axum::{AuthError, authenticate};
use micromegas::auth::types::AuthProvider;
use std::sync::Arc;
use url::Url;

const CHALLENGE: &str = "Bearer realm=\"micromegas\"";
pub const METADATA_SUFFIX: &str = "/.well-known/oauth-protected-resource";

/// Where a client learns which authorization server to sign in with (RFC 9728).
#[derive(Clone, Debug)]
pub struct McpOAuthMetadata {
    metadata_url: String,
    body: serde_json::Value,
    challenge: HeaderValue,
    invalid_token_challenge: HeaderValue,
}

/// The externally visible URL of the MCP route. It comes from configuration because deriving it
/// from a request would mean trusting the `Host` header.
pub fn resolve_public_url(
    redirect_uri: &str,
    mcp_path: &str,
    override_url: Option<&str>,
) -> Result<String> {
    let url = match override_url {
        Some(u) => {
            let url = Url::parse(u).with_context(|| format!("invalid MCP public URL {u:?}"))?;
            ensure!(
                url.path().ends_with("/mcp"),
                "MCP public URL path {:?} does not end in /mcp",
                url.path()
            );
            url
        }
        None => {
            let redirect = Url::parse(redirect_uri)
                .with_context(|| format!("invalid redirect URI {redirect_uri:?}"))?;
            ensure!(
                matches!(redirect.scheme(), "http" | "https"),
                "redirect URI {redirect_uri:?} is not http(s)"
            );
            Url::parse(&format!(
                "{}{mcp_path}",
                redirect.origin().ascii_serialization()
            ))
            .context("building the MCP public URL")?
        }
    };
    ensure!(
        matches!(url.scheme(), "http" | "https") && url.host().is_some(),
        "MCP public URL {url} is not an http(s) URL"
    );
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "MCP public URL {url} must not carry a query or fragment"
    );
    Ok(url.to_string())
}

/// The metadata route for an MCP route path or URL (`{base}/mcp` -> `{base}/.well-known/...`).
pub fn metadata_path(mcp_path: &str) -> Option<String> {
    mcp_path
        .strip_suffix("/mcp")
        .map(|base| format!("{base}{METADATA_SUFFIX}"))
}

impl McpOAuthMetadata {
    pub fn new(public_url: &str, issuer: &str) -> Result<Self> {
        let Some(metadata_url) = metadata_path(public_url) else {
            bail!("MCP public URL {public_url:?} does not end in /mcp");
        };
        // The URL is embedded in a quoted header parameter.
        ensure!(
            !metadata_url.contains(['"', '\\']),
            "metadata URL {metadata_url:?} cannot be put in a challenge header"
        );
        let challenge = format!("{CHALLENGE}, resource_metadata=\"{metadata_url}\"");
        let challenge_value = HeaderValue::from_str(&challenge)
            .context("metadata URL is not a valid header value")?;
        let invalid_token_challenge =
            HeaderValue::from_str(&format!("{challenge}, error=\"invalid_token\""))
                .context("metadata URL is not a valid header value")?;
        Ok(Self {
            body: serde_json::json!({
                "resource": public_url,
                "authorization_servers": [issuer],
                "bearer_methods_supported": ["header"],
            }),
            metadata_url,
            challenge: challenge_value,
            invalid_token_challenge,
        })
    }

    pub fn metadata_url(&self) -> &str {
        &self.metadata_url
    }

    pub fn body(&self) -> &serde_json::Value {
        &self.body
    }

    fn challenge(&self, invalid_token: bool) -> HeaderValue {
        if invalid_token {
            self.invalid_token_challenge.clone()
        } else {
            self.challenge.clone()
        }
    }
}

/// Unauthenticated, as RFC 9728 requires; it holds only the issuer and resource URLs.
pub async fn metadata_handler(State(oauth): State<Arc<McpOAuthMetadata>>) -> Response {
    Json(oauth.body.clone()).into_response()
}

/// State of [`mcp_bearer_middleware`].
#[derive(Clone)]
pub struct McpAuthState {
    pub provider: Arc<dyn AuthProvider>,
    /// When absent the 401 carries the constant challenge, with no discovery pointer.
    pub oauth: Option<Arc<McpOAuthMetadata>>,
}

fn unauthorized(oauth: &Option<Arc<McpOAuthMetadata>>, invalid_token: bool) -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    let value = match oauth {
        Some(o) => o.challenge(invalid_token),
        None => HeaderValue::from_static(CHALLENGE),
    };
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, value);
    response
}

/// Rejects requests without a valid bearer token (401 with `WWW-Authenticate`, or 503 when a
/// credential store is unreachable) and puts the resulting `AuthContext` in the request
/// extensions for the MCP tools.
pub async fn mcp_bearer_middleware(
    State(state): State<McpAuthState>,
    mut req: Request,
    next: Next,
) -> Response {
    let has_bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Bearer ") && v.len() > "Bearer ".len());
    if !has_bearer {
        return unauthorized(&state.oauth, false);
    }
    match authenticate(state.provider.as_ref(), &mut req).await {
        Ok(()) => next.run(req).await,
        Err(AuthError::InvalidToken) => unauthorized(&state.oauth, true),
        Err(AuthError::Unavailable) => AuthError::Unavailable.into_response(),
    }
}
