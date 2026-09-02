mod test_utils;

use base64::Engine;
use micromegas_auth::oidc::{OidcAuthProvider, OidcConfig, OidcIssuer};
use micromegas_auth::types::{AuthProvider, HttpRequestParts, RequestParts};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::traits::PublicKeyParts;
use test_utils::{TestKeyPair, create_valid_token};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn test_oidc_config_parsing() {
    let json = r#"{
        "issuers": [
            {
                "issuer": "https://accounts.google.com",
                "audience": "test-client-id"
            }
        ]
    }"#;

    let config: OidcConfig = serde_json::from_str(json).expect("Failed to parse config");
    assert_eq!(config.issuers.len(), 1);
    assert_eq!(config.issuers[0].issuer, "https://accounts.google.com");
    assert_eq!(config.issuers[0].audience, "test-client-id");
    assert_eq!(config.jwks_refresh_interval_secs, 3600); // default
    assert_eq!(config.token_cache_size, 1000); // default
    assert_eq!(config.token_cache_ttl_secs, 300); // default
}

#[test]
fn test_oidc_config_with_custom_values() {
    let json = r#"{
        "issuers": [
            {
                "issuer": "https://accounts.google.com",
                "audience": "test-client-id"
            }
        ],
        "jwks_refresh_interval_secs": 7200,
        "token_cache_size": 5000,
        "token_cache_ttl_secs": 600
    }"#;

    let config: OidcConfig = serde_json::from_str(json).expect("Failed to parse config");
    assert_eq!(config.jwks_refresh_interval_secs, 7200);
    assert_eq!(config.token_cache_size, 5000);
    assert_eq!(config.token_cache_ttl_secs, 600);
}

#[tokio::test]
async fn test_oidc_provider_creation() {
    let config = OidcConfig {
        issuers: vec![OidcIssuer {
            issuer: "https://accounts.google.com".to_string(),
            audience: "test-client-id".to_string(),
        }],
        jwks_refresh_interval_secs: 3600,
        token_cache_size: 1000,
        token_cache_ttl_secs: 300,
    };

    let provider = OidcAuthProvider::new(config).await;
    assert!(provider.is_ok());
}

/// Starts a mock OIDC issuer serving OIDC discovery + a JWKS containing `keypair`'s public key,
/// so `OidcAuthProvider::validate_request` can be exercised end-to-end (real signature
/// verification) without a real IdP. Returns `(server, issuer_url)` -- `server` must be kept
/// alive for as long as the provider under test may still fetch its JWKS/discovery document.
async fn start_mock_issuer(keypair: &TestKeyPair) -> (MockServer, String) {
    let server = MockServer::start().await;
    let issuer = server.uri();

    let discovery = serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    });
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(discovery))
        .mount(&server)
        .await;

    let public_key = rsa::RsaPublicKey::from_pkcs1_pem(&keypair.public_key_pem)
        .expect("parsing test public key");
    let n = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());
    let jwks = serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "n": n,
            "e": e,
        }]
    });
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(jwks))
        .mount(&server)
        .await;

    (server, issuer)
}

fn bearer_request_parts(token: &str) -> HttpRequestParts {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        format!("Bearer {token}")
            .parse()
            .expect("valid header value"),
    );
    HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/".parse().expect("valid uri"),
    }
}

/// A token validates with `AuthContext.memberships` empty -- `OidcAuthProvider` no longer reads
/// a claim to fill it; that is `MembershipProvider`'s job, layered on top.
#[tokio::test]
async fn test_validated_context_has_no_memberships() {
    let keypair = TestKeyPair::generate();
    let (_server, issuer) = start_mock_issuer(&keypair).await;
    let config = OidcConfig {
        issuers: vec![OidcIssuer {
            issuer: issuer.clone(),
            audience: "test-audience".to_string(),
        }],
        jwks_refresh_interval_secs: 3600,
        token_cache_size: 1000,
        token_cache_ttl_secs: 300,
    };
    let provider = OidcAuthProvider::new(config)
        .await
        .expect("provider creation");

    let token = create_valid_token(
        &keypair,
        &issuer,
        "test-audience",
        "user123",
        Some("user@example.com"),
    );
    let parts = bearer_request_parts(&token);
    let auth_ctx = provider
        .validate_request(&parts as &dyn RequestParts)
        .await
        .expect("validate_request");

    assert!(auth_ctx.memberships.is_empty());
    assert!(!auth_ctx.is_admin());
}

#[tokio::test]
async fn test_oidc_provider_empty_issuers() {
    let config = OidcConfig {
        issuers: vec![],
        jwks_refresh_interval_secs: 3600,
        token_cache_size: 1000,
        token_cache_ttl_secs: 300,
    };

    let provider = OidcAuthProvider::new(config).await;
    assert!(provider.is_err());
    assert!(
        provider
            .unwrap_err()
            .to_string()
            .contains("At least one OIDC issuer")
    );
}
