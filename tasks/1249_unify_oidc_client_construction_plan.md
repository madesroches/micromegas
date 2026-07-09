# Unify OIDC Client Construction & Split `analytics-web-srv/auth.rs` Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1249

## Overview

Two OIDC code paths overlap and can drift. The shared `micromegas-auth` crate
(`rust/auth/src/oidc.rs`) owns JWKS discovery and JWT validation, but
`rust/analytics-web-srv/src/auth.rs` (909 lines — the largest single service
file) **reimplements** OIDC provider discovery and login-flow client
construction rather than consuming the crate.

This plan does two things, both behavior-preserving:

1. **Consolidate login-flow OIDC client construction into the `auth` crate.**
   Move the `ConfiguredCoreClient` type alias, the `discover_async` call, and
   `build_oidc_client` out of `analytics-web-srv` and into a new
   `rust/auth/src/oidc_client.rs` module, so there is exactly one canonical way
   to discover a provider and build a configured client.
2. **Split `analytics-web-srv/src/auth.rs`** (currently one file mixing client
   construction + cookies + claims + Axum handlers) into a focused
   `auth/` submodule directory, with a `mod.rs` that re-exports the same flat
   public API so no caller changes.

This is a pure refactor: identical public API (`analytics_web_srv::auth::*`
paths unchanged), identical runtime behavior, all existing tests passing, and
`cargo clippy --workspace -- -D warnings` clean. The issue's optional
suggestion — mock-OIDC-server tests — is included as a clearly-scoped,
deferrable addition (see [Testing Strategy](#testing-strategy)).

## Current State

### The crate side — `rust/auth/src/oidc.rs` (~600 lines)

Owns validation, not login-flow client construction:
- `create_http_client()` (`oidc.rs:42`) — SSRF-hardened `reqwest::Client`
  (no redirects). Already `pub`, already imported by `analytics-web-srv`.
- `fetch_jwks()` (`oidc.rs:50`) — internally calls
  `CoreProviderMetadata::discover_async` (`oidc.rs:54`) **only** to read
  `jwks_uri`, then fetches the JWKS. This discovery is an internal
  implementation detail of JWKS fetching for the validation path.
- `OidcConfig` / `OidcIssuer` (`oidc.rs:108`, `124`) — issuer/audience config,
  `from_env` / `from_env_var`. Already consumed by `analytics-web-srv`.
- `OidcAuthProvider` (`oidc.rs:318`) — the JWT-validation provider.

### The web side — `rust/analytics-web-srv/src/auth.rs` (909 lines)

Contains, in order:
- **Login-flow client construction (the duplication to move):**
  - `ConfiguredCoreClient` type alias (`auth.rs:37-55`) — the fully-parameterized
    `openidconnect::Client<...>` type.
  - `OidcProviderInfo` (`auth.rs:123-129`) — `Arc<CoreProviderMetadata>` +
    `client_id` + `redirect_uri`.
  - `AuthState::get_oidc_provider` (`auth.rs:166-190`) — lazily discovers via
    `CoreProviderMetadata::discover_async` (`auth.rs:175`) and builds an
    `OidcProviderInfo`.
  - `AuthState::build_oidc_client` (`auth.rs:192-199`) — builds a
    `ConfiguredCoreClient` from provider metadata.
- **Web config:** `OidcClientConfig` + `from_env` (`auth.rs:57-121`) —
  web-specific: reads `MICROMEGAS_AUTH_REDIRECT_URI`, selects the **first**
  issuer for the login flow (validation still accepts all issuers).
- **Shared state:** `AuthState` (`auth.rs:131-216`) — holds config, the two
  lazy `OnceCell`s (`oidc_provider`, `auth_provider`), cookie settings, state
  signing secret, base path, admin var name; methods `cookie_path`,
  `get_oidc_provider`, `build_oidc_client`, `get_auth_provider`.
- **Cookies:** `ID_TOKEN_COOKIE`/`REFRESH_TOKEN_COOKIE`/`OAUTH_STATE_COOKIE`
  consts (`auth.rs:309-311`), `create_cookie` (`auth.rs:314`),
  `clear_cookie` (`auth.rs:335`).
- **Claims / user types:** `IdTokenClaims` (`auth.rs:245`), `UserInfo`
  (`auth.rs:236`), `ValidatedUser` (`auth.rs:256`) + `From<&AuthContext>`
  (`auth.rs:267`), `CookieTokenRequestParts` (`auth.rs:282`) +
  `RequestParts` impl, `extract_name_from_token` (`auth.rs:714`),
  `extract_subject_from_token` (`auth.rs:729`).
- **Handlers / web glue:** `LoginQuery`/`CallbackQuery` (`auth.rs:220`, `227`),
  `auth_login` (`auth.rs:352`), `auth_callback` (`auth.rs:409`),
  `auth_refresh` (`auth.rs:545`), `auth_logout` (`auth.rs:653`),
  `auth_me` (`auth.rs:670`), `AuthApiError` (`auth.rs:745`) + `IntoResponse`,
  `cookie_auth_middleware` (`auth.rs:793`), `AuthToken` (`auth.rs:854`),
  `AdminRequired`/`require_admin`/`AdminUser` (`auth.rs:862-909`).

### Public API surface that MUST be preserved

`AuthState` is built as a **struct literal** in four places — every public
field and the flat `analytics_web_srv::auth::*` paths must stay valid:

| Consumer | Items used |
|----------|-----------|
| `analytics-web-srv/src/web_server.rs:4,59-104` | `AuthState` (literal, all fields), `AuthToken`, `OidcClientConfig` (+`from_env`), `ValidatedUser`, `auth_login`, `auth_callback`, `auth_refresh`, `auth_logout`, `auth_me`, `cookie_auth_middleware` |
| `analytics-web-srv/tests/auth_integration.rs:10,25` | `AuthState` (literal), `OidcClientConfig`, `auth_logout` |
| `analytics-web-srv/tests/auth_unit_tests.rs:3,14` | `AuthApiError`, `AuthState` (literal), `OidcClientConfig`, `clear_cookie`, `create_cookie` |
| `analytics-web-srv/tests/maps_tests.rs:13,52` | `AuthState` (literal), `AuthToken`, `OidcClientConfig`, `ValidatedUser` |

The monolith (`rust/monolith/src/main.rs:328-333`) builds a `WebServerConfig`,
not an `AuthState` — it is unaffected.

Note: the struct-literal construction is what makes it impossible to hide
`AuthState`'s fields; the split must keep every field `pub` and keep
`AuthState` constructible from outside the module.

### Dead-code observation (behavior-preserving cleanup)

`auth_callback` (`auth.rs:439`) and `auth_refresh` (`auth.rs:561`) each contain
`let _client = state.build_oidc_client(provider);` whose result is **never
used** — both handlers do a manual HTTP token exchange (`auth.rs:441-472`,
`auth.rs:563-587`) against `provider.metadata.token_endpoint()` rather than
using the openidconnect client (comment at `auth.rs:442-445` explains why:
Auth0 non-standard fields break the library's strict parsing). Only
`auth_login` (`auth.rs:385`) actually uses the built client (for
`authorize_url`). These two dead `_client` bindings will be dropped, reducing
`build_client` call sites to exactly one.

## Design

### Part 1 — new crate module `rust/auth/src/oidc_client.rs`

Holds the canonical login-flow client construction. Register in
`auth/src/lib.rs` with a doc comment: `pub mod oidc_client;` (placed after
`pub mod oidc;`).

```rust
use crate::oidc::create_http_client;
use anyhow::{Result, anyhow};
use openidconnect::core::{CoreClient, CoreProviderMetadata};
use openidconnect::{ClientId, IssuerUrl, RedirectUrl};
use std::sync::Arc;

/// Fully-parameterized openidconnect client with endpoints set from
/// discovered provider metadata (public client, PKCE — no client secret).
pub type ConfiguredCoreClient = openidconnect::Client< /* …16 type params, moved verbatim from auth.rs:37-55… */ >;

/// A discovered OIDC provider, ready to build login-flow clients from.
///
/// This is the single canonical home for provider discovery + client
/// construction. `analytics-web-srv` caches one of these in a `OnceCell`
/// and calls `build_client()` for the authorization-code flow.
pub struct DiscoveredProvider {
    pub metadata: Arc<CoreProviderMetadata>,
    pub client_id: ClientId,
    pub redirect_uri: RedirectUrl,
}

impl DiscoveredProvider {
    /// Discover provider metadata for `issuer` and remember the client id /
    /// redirect uri needed to build clients. Uses the SSRF-hardened HTTP client.
    pub async fn discover(issuer: &str, client_id: &str, redirect_uri: &str) -> Result<Self> {
        let issuer_url = IssuerUrl::new(issuer.to_string())
            .map_err(|e| anyhow!("Invalid issuer URL: {e:?}"))?;
        let http_client = create_http_client()?;
        let metadata = CoreProviderMetadata::discover_async(issuer_url, &http_client)
            .await
            .map_err(|e| anyhow!("Failed to discover OIDC provider: {e:?}"))?;
        let redirect_uri = RedirectUrl::new(redirect_uri.to_string())
            .map_err(|e| anyhow!("Invalid redirect URI: {e:?}"))?;
        Ok(Self {
            metadata: Arc::new(metadata),
            client_id: ClientId::new(client_id.to_string()),
            redirect_uri,
        })
    }

    /// Build a configured login-flow client (public client, PKCE) from the
    /// discovered metadata.
    pub fn build_client(&self) -> ConfiguredCoreClient {
        CoreClient::from_provider_metadata(
            (*self.metadata).clone(),
            self.client_id.clone(),
            None, // public client with PKCE — no secret
        )
        .set_redirect_uri(self.redirect_uri.clone())
    }
}
```

`DiscoveredProvider` is a drop-in replacement for the web crate's
`OidcProviderInfo`: same three public fields (so `auth_callback` /
`auth_refresh` keep reading `provider.metadata.token_endpoint()`), plus the
`build_client()` method that replaces `AuthState::build_oidc_client`.

**Scope boundary (deliberate):** we do **not** merge this discovery with
`fetch_jwks`'s internal `discover_async` (`oidc.rs:54`). They serve different
lazy caches for different purposes (login-flow client vs. JWKS for validation)
and the issue's target is the *web-srv duplication*, not the crate's internal
JWKS path. Merging them would change caching behavior and is out of scope.

### Part 2 — split `analytics-web-srv/src/auth.rs` into `auth/` directory

Delete the single `src/auth.rs`; create `src/auth/` with these files. Each file
holds one concern; `mod.rs` re-exports so `analytics_web_srv::auth::<Item>`
paths are unchanged for every caller in the table above.

```
analytics-web-srv/src/auth/
├── mod.rs          re-exports (flat public API), `mod` declarations
├── config.rs       OidcClientConfig + from_env  (web-specific config)
├── state.rs        AuthState + methods (get_oidc_provider, get_auth_provider, cookie_path)
├── cookies.rs      cookie name consts, create_cookie, clear_cookie
├── claims.rs       ValidatedUser, UserInfo, IdTokenClaims, CookieTokenRequestParts, token extractors
└── handlers.rs     the 5 handlers, cookie_auth_middleware, AuthApiError, AuthToken, Admin* extractors
```

**`config.rs`** — `OidcClientConfig` + `from_env` (moved verbatim from
`auth.rs:57-121`).

**`state.rs`** — `AuthState` (all fields kept `pub`; the `oidc_provider`
field's inner type changes from `OidcProviderInfo` to
`micromegas_auth::oidc_client::DiscoveredProvider`) and its methods:
- `cookie_path` — unchanged.
- `get_oidc_provider` — now thin: reads `self.config`, calls
  `DiscoveredProvider::discover(&config.issuer, &config.client_id,
  &config.redirect_uri)` inside the `OnceCell::get_or_try_init`.
- `get_auth_provider` — unchanged (builds `OidcAuthProvider`).
- `build_oidc_client` is **removed**; callers use
  `provider.build_client()` (the crate method) directly.

```rust
// state.rs, get_oidc_provider body (delegating to the crate)
self.oidc_provider
    .get_or_try_init(|| async move {
        DiscoveredProvider::discover(&config.issuer, &config.client_id, &config.redirect_uri)
            .await
            .map_err(|e| anyhow!("Failed to get OIDC provider: {e:?}"))
    })
    .await
```

Because all `AuthState` fields stay `pub` and the four struct-literal sites
never name the `oidc_provider` inner type (they write
`Arc::new(tokio::sync::OnceCell::new())`), the inner-type change is invisible to
callers — no test edits needed for construction.

**`cookies.rs`** — the three cookie-name consts, `create_cookie`,
`clear_cookie`. `create_cookie`/`clear_cookie` keep their `&AuthState`
parameter (`use super::state::AuthState`). Consts are `pub(crate)` (used by
handlers).

**`claims.rs`** — `ValidatedUser` (+ `From<&AuthContext>`), `UserInfo`,
`IdTokenClaims`, `CookieTokenRequestParts` (+ `RequestParts` impl),
`extract_name_from_token`, `extract_subject_from_token`.

**`handlers.rs`** — `LoginQuery`, `CallbackQuery`, the five handlers
(`auth_login`/`auth_callback`/`auth_refresh`/`auth_logout`/`auth_me`),
`cookie_auth_middleware`, `AuthApiError` (+ `IntoResponse`), `AuthToken`,
`AdminRequired`, `require_admin`, `AdminUser` (+ `FromRequestParts`). This file
remains the largest (~450 lines) but is cohesive: all Axum request/response
glue. In `auth_login`, `state.build_oidc_client(provider)` becomes
`provider.build_client()`; in `auth_callback`/`auth_refresh` the dead
`let _client = …` lines are removed.

**`mod.rs`** — declares the submodules and re-exports the flat API:

```rust
mod claims;
mod config;
mod cookies;
mod handlers;
mod state;

pub use claims::{UserInfo, ValidatedUser};
pub use config::OidcClientConfig;
pub use cookies::{clear_cookie, create_cookie};
pub use handlers::{
    AdminRequired, AdminUser, AuthApiError, AuthToken, auth_callback, auth_login, auth_logout,
    auth_me, auth_refresh, cookie_auth_middleware, require_admin,
};
pub use state::AuthState;
```

Only items actually referenced externally need `pub use`; internal-only types
(`IdTokenClaims`, `CookieTokenRequestParts`, `LoginQuery`, `CallbackQuery`,
cookie consts) stay module-private / `pub(crate)`. Cross-check the re-export
list against the four-consumer table so every previously-public name resolves;
`cargo build` + `cargo test --no-run` will confirm.

### Module dependency graph (no cycles)

```
config ── (leaf)
claims ── (leaf; uses micromegas_auth::types)
cookies ─→ state
state   ─→ config, micromegas_auth::{oidc, oidc_client}
handlers ─→ state, config, cookies, claims, micromegas_auth::{oidc,oidc_client,oauth_state,url_validation}
mod ─→ all
```

## Implementation Steps

### Phase 1 — crate: add `oidc_client` module
1. Create `rust/auth/src/oidc_client.rs` with `ConfiguredCoreClient` (type
   alias moved verbatim from `auth.rs:37-55`) and `DiscoveredProvider`
   (`discover` + `build_client`).
2. Add `pub mod oidc_client;` to `rust/auth/src/lib.rs` (after `pub mod oidc;`)
   with a one-line doc comment.
3. `cargo build -p micromegas-auth` — crate compiles standalone.

### Phase 2 — web: split `auth.rs` into `auth/`
4. Create `src/auth/` and move code into `config.rs`, `state.rs`, `cookies.rs`,
   `claims.rs`, `handlers.rs` as specified. Delete `src/auth.rs`.
5. In `state.rs`: change `oidc_provider` inner type to `DiscoveredProvider`,
   rewrite `get_oidc_provider` to delegate to `DiscoveredProvider::discover`,
   remove `build_oidc_client` and `OidcProviderInfo`.
6. In `handlers.rs`: replace `state.build_oidc_client(provider)` with
   `provider.build_client()` in `auth_login`; remove the dead `let _client`
   bindings in `auth_callback` and `auth_refresh`.
7. Write `src/auth/mod.rs` with `mod` declarations + `pub use` re-exports.
8. `cargo build -p analytics-web-srv` — resolve visibility until it compiles.

### Phase 3 — verify & tidy
9. `cargo test -p micromegas-auth -p analytics-web-srv` — all existing tests
   pass unchanged (no test edits expected; if any fail to *compile*, it means a
   `pub use` is missing — add it, do not change the test).
10. `cargo fmt` and `cargo clippy --workspace -- -D warnings`.

### Phase 4 (optional, deferrable) — mock-OIDC-server tests
11. See [Testing Strategy](#testing-strategy). If it grows beyond a tight,
    self-contained addition, split it into a follow-up PR and land Phases 1–3
    alone (they fully satisfy the acceptance criteria).

## Files to Modify

**Create**
- `rust/auth/src/oidc_client.rs`
- `rust/analytics-web-srv/src/auth/mod.rs`
- `rust/analytics-web-srv/src/auth/config.rs`
- `rust/analytics-web-srv/src/auth/state.rs`
- `rust/analytics-web-srv/src/auth/cookies.rs`
- `rust/analytics-web-srv/src/auth/claims.rs`
- `rust/analytics-web-srv/src/auth/handlers.rs`

**Modify**
- `rust/auth/src/lib.rs` (add `pub mod oidc_client;`)

**Delete**
- `rust/analytics-web-srv/src/auth.rs`

**Optional (Phase 4)**
- `rust/analytics-web-srv/Cargo.toml` (add `wiremock` dev-dep)
- `rust/analytics-web-srv/tests/auth_oidc_mock_tests.rs` (new)

**Unchanged (verified):** `web_server.rs`, `monolith/src/main.rs`, the three
existing auth test files — all continue to compile against the re-exported API.

## Trade-offs

- **`DiscoveredProvider` in the crate vs. keeping `OidcProviderInfo` in the web
  crate and only moving `build_oidc_client`.** Moving the whole discover+build
  pair is what the issue asks for ("discovery and client-building now live in
  two places") and gives one canonical entry point; leaving discovery in the
  web crate would only half-solve it. Chosen: move both.
- **`mod.rs` re-exports vs. updating every caller to nested paths.** Re-exports
  keep the public API flat and make this a zero-churn refactor for four
  consumers (incl. three test files). Chosen: re-exports.
- **Splitting `handlers.rs` further (errors/extractors into their own files).**
  Deferred — the acceptance criterion is only that construction/cookies/claims
  aren't mixed with handlers; over-splitting the cohesive Axum glue adds files
  without clear benefit. `handlers.rs` stays one file.
- **Not merging with `fetch_jwks` discovery.** Keeps caching behavior identical;
  merging is a separate concern outside this issue's scope.

## Documentation

No user-facing docs cover this internal module layout; `mkdocs/` needs no
changes. The module-level doc comment currently atop `auth.rs:1-12` moves to
`auth/mod.rs`. New `pub` crate items (`oidc_client`, `DiscoveredProvider`,
`ConfiguredCoreClient`) get doc comments (rendered in `cargo doc`).

## Testing Strategy

- **Existing tests are the regression net** and must pass unchanged:
  - `rust/auth/tests/oidc_tests.rs` (crate config/provider construction).
  - `rust/analytics-web-srv/tests/auth_unit_tests.rs` (cookie helpers,
    `AuthApiError` status codes, `AuthState` construction).
  - `rust/analytics-web-srv/tests/auth_integration.rs` (`auth_logout` cookie
    clearing, cookie flags).
  - `rust/analytics-web-srv/tests/maps_tests.rs` (constructs `AuthState`).
  Run: `cargo test -p micromegas-auth -p analytics-web-srv`.
- **Full gate:** `python3 build/rust_ci.py` (fmt check + clippy + tests) from
  `rust/`.
- **Optional new coverage (Phase 4)** — the mock-OIDC tests flagged at
  `auth_integration.rs:133`. Add `wiremock` (already a workspace dep at
  `rust/Cargo.toml:103`) as an `analytics-web-srv` dev-dep and a new
  `tests/auth_oidc_mock_tests.rs` that:
  1. Generates an RSA keypair (`rsa` + `jsonwebtoken` are already
     `analytics-web-srv` dev-deps) and signs an ID token, mirroring the helper
     pattern in `auth/tests/test_utils.rs` (that helper lives under `auth/tests/`
     and is not importable across crates, so replicate the minimal parts).
  2. Stands up a `wiremock::MockServer` serving
     `/.well-known/openid-configuration` and a `jwks_uri` whose JWKS contains
     the matching public key (`n`/`e` from the RSA public key).
  3. Points an `AuthState`'s `MICROMEGAS_OIDC_CONFIG` issuer at the mock server
     and asserts `cookie_auth_middleware` / `auth_me` accept a valid token and
     reject an expired/forged one.
  This is a genuine coverage gain but is **not** required by the acceptance
  criteria; if it expands the diff materially it becomes a follow-up PR.

## Acceptance Criteria (from the issue)
- [ ] OIDC provider discovery and client construction exist in exactly one
  place (the `auth` crate); `analytics-web-srv` no longer defines its own
  `build_oidc_client` / `ConfiguredCoreClient`.
- [ ] `analytics-web-srv/src/auth.rs` is split so no single file mixes client
  construction + cookies + claims + handlers.
- [ ] Existing auth tests still pass; behavior unchanged.

## Open Questions

None that block implementation. The only genuinely optional decision — whether
the mock-OIDC-server tests land in this PR or a follow-up — is resolved by a
size heuristic in Phase 4 (land here if tight and self-contained; otherwise
defer), and does not affect Phases 1–3, which fully satisfy the acceptance
criteria on their own.
