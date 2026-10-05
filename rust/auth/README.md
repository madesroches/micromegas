# micromegas-auth

API key and OIDC (OpenID Connect) authentication providers for axum and tonic services, used by the Micromegas services. Providers implement a common `AuthProvider` trait that validates a request and returns the authenticated subject.

```rust
use micromegas_auth::api_key::parse_key_ring;

let keyring = parse_key_ring(r#"[{"name": "user1", "key": "secret-key-123"}]"#).unwrap();
# let _ = keyring;
```

See the crate documentation on docs.rs for complete API key and OIDC examples.

## Documentation

- [Authentication Guide](https://micromegas.info/docs/admin/authentication/)
- [API documentation on docs.rs](https://docs.rs/micromegas-auth)
- [GitHub Repository](https://github.com/madesroches/micromegas)
