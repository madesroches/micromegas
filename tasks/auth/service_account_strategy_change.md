# Service Account Authentication Strategy Change

## Date
2025-10-24

## Decision
After discussion with Julien, we're changing the service account authentication strategy from **self-signed JWTs with local JWKS** to **OAuth 2.0 client credentials flow**.

## Previous Strategy (Self-Signed JWTs)

**How it worked:**
1. Admin creates service account via SQL UDF
2. System generates RSA keypair (private key for service, public key in database)
3. Service generates self-signed JWTs offline using private key
4. Server loads public keys from database into local JWKS
5. Server validates service tokens using local JWKS (offline)

**Architecture:**
- `UnifiedJwtAuthProvider` with multiple JWKS sources:
  - Remote JWKS for OIDC providers (humans)
  - Local JWKS for service accounts (from database)
- Julien's proposal document (`julien_unified_jwks_architecture_proposal.md`) described this approach

**Benefits:**
- Offline token generation (no network calls)
- No dependency on external OAuth server
- Simple key management in database

**Drawbacks:**
- Duplicate JWT validation infrastructure (local + remote)
- Custom key management logic
- Database becomes a key store
- Need separate admin UDFs for key management
- More complex architecture

## New Strategy (Client Credentials Flow)

**How it works:**
1. Admin creates service account in OIDC provider (e.g., Google Cloud, Azure AD, Okta)
2. OIDC provider issues client_id + client_secret
3. Service authenticates using client credentials:
   ```
   POST /token
   grant_type=client_credentials
   client_id=<service-id>
   client_secret=<secret>
   ```
4. OIDC provider returns access token (standard OAuth JWT)
5. Service uses access token for API calls
6. Server validates using same OIDC/JWKS infrastructure as human users

**Architecture:**
- Single `OidcAuthProvider` for ALL authentication
- All tokens validated using remote JWKS from OIDC provider
- No local JWKS needed
- No custom service account database tables
- Service accounts managed in OIDC provider (not in micromegas)

**Benefits:**
- **Simpler architecture**: One authentication path for everyone
- **Standard OAuth**: No custom JWT logic
- **Leverages existing infrastructure**: OIDC provider handles key rotation, revocation, etc.
- **Less code**: No local JWKS, no service account registry, no admin UDFs
- **Better security**: OIDC providers have mature key management
- **Consistent**: Humans and services use same flow (different grant types)

**Trade-offs:**
- **Network dependency**: Services must call OIDC provider to get tokens
  - Mitigated by: Token caching (1 hour lifetime typically)
  - Services can cache tokens until expiration
- **External dependency**: Requires OIDC provider setup
  - Most organizations already have this (Google, Azure AD, Okta)
- **No offline operation**: Services need network access initially
  - Not a real issue in practice (services run in environments with network)

## Impact on Planning Documents

### Documents to Update
1. **`analytics_auth_plan.md`**:
   - Remove all service account self-signed JWT sections
   - Remove `ServiceAccountAuthProvider`
   - Remove `UnifiedJwtAuthProvider`
   - Keep simple `OidcAuthProvider` for all authentication
   - Remove service account database schema
   - Remove admin SQL UDFs
   - Update Phase 2 to describe client credentials flow setup

2. **`julien_unified_jwks_architecture_proposal.md`**:
   - Add note at top explaining this proposal is superseded
   - Keep for historical reference (documents the decision process)
   - Explain why we chose client credentials instead

### Documents to Keep Unchanged
- **`oidc_auth_subplan.md`**: Still accurate for human user OIDC flow
  - Server-side validation stays the same
  - Python client OIDC flow stays the same
  - Only addition: Document how services use client credentials

## Migration Path

**Phase 1: OIDC for Humans** (Current - ~90% complete)
- Implement `OidcAuthProvider` ✅
- Support human user authentication via OIDC ✅
- Keep API keys for backward compatibility ✅

**Phase 2: OIDC Client Credentials for Services** (New approach)
- Document how to create service accounts in OIDC provider
- Implement client credentials flow in Python/Rust clients
- Services use `client_id` + `client_secret` to get tokens
- Tokens validated same way as human tokens (same `OidcAuthProvider`)

**Phase 3: Deprecate API Keys**
- API keys still work (backward compatibility)
- Clear migration documentation
- Eventually remove API key support

## Example: Service Authentication

**Old approach (self-signed JWT)**:
```python
# Service loads credential file with private key
auth = ServiceAccountAuthProvider.from_file("my-service.json")
# Generates JWT locally, no network call
client = FlightSQLClient(uri, auth_provider=auth)
```

**New approach (client credentials)**:
```python
# Service uses client credentials
auth = OidcClientCredentialsProvider(
    issuer="https://accounts.google.com",
    client_id="my-service@project.iam.gserviceaccount.com",
    client_secret=os.environ["CLIENT_SECRET"],  # From secret manager
)
# First call: Gets token from OIDC provider (network call)
# Subsequent calls: Uses cached token until expiration
client = FlightSQLClient(uri, auth_provider=auth)
```

## Server Configuration

**Old approach**:
```bash
MICROMEGAS_AUTH_MODE=jwt
MICROMEGAS_OIDC_CONFIG='{"issuers": [...]}'  # For humans
# Service accounts loaded from database
```

**New approach**:
```bash
MICROMEGAS_AUTH_MODE=oidc
MICROMEGAS_OIDC_CONFIG='{"issuers": [...]}'
# Same config for humans and services - all use OIDC provider
```

## Conclusion

The client credentials flow simplifies the architecture significantly:
- One authentication provider instead of two
- No custom key management
- Leverages mature OIDC infrastructure
- Standard OAuth patterns throughout
- Less code to write, test, and maintain

The trade-off (network dependency for initial token fetch) is acceptable because:
- Services typically run in networked environments
- Tokens are cached for their lifetime (typically 1 hour)
- OIDC providers are highly available
- This is how most modern services authenticate (industry standard)

## References
- [OAuth 2.0 Client Credentials Grant](https://datatracker.ietf.org/doc/html/rfc6749#section-4.4)
- [Google Cloud Service Account Authentication](https://cloud.google.com/iam/docs/service-accounts)
- [Azure AD Client Credentials Flow](https://learn.microsoft.com/en-us/azure/active-directory/develop/v2-oauth2-client-creds-grant-flow)
- [Okta Client Credentials Flow](https://developer.okta.com/docs/guides/implement-grant-type/clientcreds/main/)
