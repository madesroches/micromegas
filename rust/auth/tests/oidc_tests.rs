use micromegas_auth::oidc::{OidcAuthProvider, OidcConfig, OidcIssuer};

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
