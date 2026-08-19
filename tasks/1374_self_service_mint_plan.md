# Self-Service Ingestion Key Mint + Setup Script Plan (#1374, AbAC Stage 6)

## Overview

Enables real per-user ingestion keys for privacy deployments. Stages 0–5 and 6a of the AbAC
rollout (`tasks/data_isolation/audience_based_access_control_plan.md`) are landed: the DB-backed
key store, the `audience` column, query enforcement (Prongs A+B), ingestion stamping, and the
DB-backed audience grant store. What's missing is the last mile: today `analytics-web-srv`'s mint
route (`POST {base_path}/api/ingestion-api-keys`) is `AdminUser`-gated only, and never calls the
`MintPolicy` trait that Stage 1 (#1369) already defined and Stage 4 (#1372) already implemented
and unit-tested (`AudienceMintPolicy`, `rust/auth/src/policy.rs`) — an operator has to mint every
personal key by hand.

Given the grant store already exists (Stage 6a, #1489), this stage opens a **non-admin
self-service path**: a caller with a matching `mint` grant selector (`user:<email>` or
`group:<g>`) can mint their own key directly, with `MintPolicy::resolve_audience` as the
authorization instead of an admin gate. Admins keep minting *any* audience unchanged (the policy's
existing `is_admin` arm) — nothing regresses for the open/team deployment story. A companion
Python setup script does an OIDC interactive login, mints a personal key, and writes the OTLP
exporter env vars a user needs to point their own telemetry at the deployment.

## Current State

- **`MintPolicy`/`AudienceMintPolicy`** (`rust/auth/src/policy.rs:398-592`) are fully implemented
  and unit-tested (`rust/auth/tests/policy_tests.rs:198-301`) but have **zero production call
  sites** — confirmed by a repo-wide search; only `policy.rs`, `lib.rs`, `types.rs`, and the test
  files reference them. `AudienceMintPolicy::resolve_audience` (`policy.rs:548-592`):
  - `requested: None` is always `Err` — there is no "myself" audience to default to under the
    opaque-label model (Stage 4 override).
  - `caller.is_admin` may mint **any** valid audience (format-checked via `is_valid_audience`),
    `public` included — this is what lets today's `AdminUser`-gated flow keep working once the
    gate is lifted.
  - A non-admin caller may mint `aud` only if a selector in `grants.mint_selectors(aud)` (env map)
    or the attached `DbAudienceGrantsSource` snapshot matches `caller` (`selector_matches`,
    `policy.rs:104-117`, matching `*`, `user:<email>`, or `group:<g>` against `caller.email`/
    `caller.groups`).
  - `AudienceMintPolicy::new(grants: AudienceGrants) -> Self` and
    `.with_store(Some(Arc<DbAudienceGrantsSource>)) -> Self` are the only constructors — there is
    **no `from_env`** (unlike `AudienceReadPolicy`, which has both `new` and `from_env`,
    `policy.rs:440-461`).
- **The mint route** (`rust/analytics-web-srv/src/ingestion_keys.rs::mint_key`, lines 211-260) is
  `AdminUser`-gated (`AdminUser(user): AdminUser`, line 213) and resolves the audience via a local
  free function `resolve_audience` (lines 172-193: `requested → state.default_audience →
  fallback`, format-validated, no policy/grant check at all — the admin gate *is* today's
  authorization). `list_keys`/`revoke_key`/`import_key` (lines 283-450) are also `AdminUser`-gated;
  `import_key`'s fallback is `PUBLIC_AUDIENCE`.
- **Caller identity is already threaded through for exactly this purpose.** Stage 1 anticipated
  this stage directly: `cookie_auth_middleware` (`rust/analytics-web-srv/src/auth/handlers.rs:459-
  516`) inserts *both* `ValidatedUser` (line 511, a groups-free browser-session view) and the full
  `AuthContext` (line 516) into request extensions, with the comment "Stage 6's `mint_key` needs
  `AuthContext` (groups included) to consult a `MintPolicy`." No handler reads the `AuthContext`
  extension yet.
- **`AdminUser`** (`handlers.rs:563-579`) is the only `FromRequestParts` impl in the crate — it
  pulls `ValidatedUser` from extensions and 403s unless `is_admin`. There is no "any authenticated
  caller" extractor to reuse.
- **Real gap: `--disable-auth` mode never populates `AuthContext`.** `build_protected_routes`
  (`web_server.rs:317-419`), disabled-auth branch (lines 409-418), layers only
  `Extension(AuthToken(...))` and `Extension(ValidatedUser { is_admin: true, .. })` — confirmed by
  a whole-crate search, `AuthContext` is inserted into extensions **only** by the enabled-auth path
  (`handlers.rs:516`). A non-admin extractor reading `AuthContext` would break local/dev/test runs
  under `--disable-auth` without a fix here.
- **`ReadPolicy` wiring is the pattern to mirror for `MintPolicy`**, but no `MintPolicy` wiring
  exists anywhere to copy verbatim. `public/src/servers/flight_sql_server.rs:306-314` and
  `monolith/src/main.rs:271-281` both do:
  ```rust
  let audience_grants_pool = dedicated_key_store_pool(&lake_pool);
  let audience_grants_config = DbAudienceGrantsConfig::from_env_with_prefix(prefix);
  let audience_grants_store = Arc::new(DbAudienceGrantsSource::new(audience_grants_pool, audience_grants_config));
  let policy: Arc<dyn ReadPolicy> = Arc::new(
      AudienceReadPolicy::from_env(prefix)?.with_store(Some(audience_grants_store)),
  );
  ```
  `dedicated_key_store_pool` (`auth/src/db_api_key.rs:135-141`) builds a small tuned pool (4
  connections, 2s acquire timeout) from an existing pool's connect options.
  `DbAudienceGrantsSource::new(pool: PgPool, config: DbAudienceGrantsConfig)`
  (`auth/src/db_audience_grants.rs:87-95`); `DbAudienceGrantsConfig::from_env_with_prefix(prefix)`
  (`db_audience_grants.rs:37-41`) reads `{prefix}_AUDIENCE_GRANT_CACHE_TTL_SECONDS` (fallback
  `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS`, default 60).
- **`analytics-web-srv` already opens the telemetry-DB pool this stage needs.**
  `web_server.rs::run_web_server` (~lines 634-670) builds `analytics_keys_pool: Option<PgPool>`
  (lazy, `max_connections(2)`, from `MICROMEGAS_SQL_CONNECTION_STRING`) and constructs
  `IngestionKeysState { pool: analytics_keys_pool.clone(), default_audience: ... }`
  (`ingestion_keys.rs:53-61`) plus the sibling `AudienceGrantsState`/`AnalyticsKeysState` off the
  same pool. `default_key_audience_from_env("")` and every other analytics-web-srv knob already
  use the empty-prefix (`""`) convention — the standalone-service convention, vs. monolith's
  per-role `"MICROMEGAS_ANALYTICS"` namespacing.
- **No mint method exists in the Python client.** `python/micromegas/micromegas/web_client.py`
  (206 lines) has `import_ingestion_api_key(self, name, key, audience=None)` (lines 99-124,
  `POST ingestion-api-keys/import`) but no `mint_ingestion_api_key` — confirmed by a repo-wide
  search for `mint_key`/`mint_ingestion_api_key` across `python/` and `rust/`.
- **No OIDC device-code flow exists anywhere in the repo** (confirmed: zero matches for
  `device_authorization`/`device_code`). A full loopback-redirect + PKCE interactive flow **does**
  already exist and is used by every interactive CLI:
  `python/micromegas/micromegas/auth/oidc.py`'s `OidcAuthProvider.login(...)` (spins up a one-shot
  `http.server` on the parsed `redirect_uri` port, opens a browser, exchanges the code with PKCE)
  plus `.save()`/`.from_file()` (token cache at `~/.micromegas/tokens[-<profile>].json`, `0o600`,
  written by `cli/config.py:default_token_file`, lines 36-41) and
  `python/micromegas/micromegas/oidc_connection.py::load_or_login(...)` (tries the cached token
  file first, falls back to interactive login). `cli/import_keys.py::build_auth_provider` (lines
  146-178) and `make_client` (lines 182-186) are the exact pattern every CLI in this repo follows:
  client-credentials env vars first, else `config.resolve_connection(profile=args.profile)` →
  `load_or_login(...)`. `config.resolve_connection` (`cli/config.py:131`) returns a
  `ConnectionConfig` (`uri`, `oidc_issuer`, `oidc_client_id`, `oidc_client_secret`, `oidc_audience`,
  `oidc_scope`, `token_file`) resolved with precedence env vars > active profile >
  `~/.micromegas/config.json` defaults.
- **CLI registration convention**: `python/micromegas/pyproject.toml:36-41`,
  `[tool.poetry.scripts]`, one `micromegas-<name> = "micromegas.cli.<module>:main"` line per
  script (`micromegas-grants`, `micromegas-import-keys`, `micromegas-logout`, `micromegas-query`,
  `micromegas-screens`), each an `argparse`-based `cli/<module>.py` with a `main()`.
- **Docs that explicitly defer to this stage**: `mkdocs/docs/admin/authentication.md:334-335`
  ("provisioning one per user is Stage 6 (#1374) territory, since that's the stage that lets a
  user mint their own key in the first place") and `:363-364` ("Worked mint profiles ... are
  deferred to Stage 6 (#1374), the first stage with a real non-admin mint consumer").
  `mkdocs/docs/admin/api-keys.md:90-91` and `:313-315` currently state that *every* mint/revoke/
  import handler is admin-gated and resolves the caller via `AdminUser` — accurate for
  revoke/import, no longer accurate for mint once this stage lands.

## Design

### 1. New extractor: `AuthenticatedUser`

Add to `rust/analytics-web-srv/src/auth/handlers.rs`, right beside `AdminUser` (line 563):

```rust
/// Extractor that yields the caller's full `AuthContext` for any authenticated request, with no
/// admin check — the mint route's authorization is `MintPolicy::resolve_audience` itself, not a
/// gate in front of it.
pub struct AuthenticatedUser(pub AuthContext);

impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedUser {
    type Rejection = Unauthenticated;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthContext>()
            .cloned()
            .map(AuthenticatedUser)
            .ok_or(Unauthenticated)
    }
}
```

`Unauthenticated` mirrors `AdminRequired`'s shape (401, `{code: "UNAUTHENTICATED", message}`)
rather than relying on axum's generic `Extension<T>` rejection (a 500), for a correct status code
in the — normally unreachable once the fix below lands — case where the extension is missing.

### 2. Fix the `--disable-auth` gap

`web_server.rs::build_protected_routes`, disabled-auth branch (lines 409-418), gains a matching
`AuthContext` layer next to the existing hardcoded admin `ValidatedUser`:

```rust
.layer(Extension(AuthContext {
    subject: "anonymous".to_string(),
    email: None,
    issuer: "local".to_string(),
    audience: None,
    expires_at: None,
    auth_type: AuthType::Oidc,
    is_admin: true,
    allow_delegation: false,
    bound_audience: None,
    read_audiences: vec![],
    groups: vec![],
}))
```

### 3. Wire a `MintPolicy` in `analytics-web-srv`

`web_server.rs::run_web_server`, beside the existing `ingestion_keys_state` construction
(~line 667): build the first production `Arc<dyn MintPolicy>`, mirroring the `ReadPolicy` wiring
in §"Current State" but built from parts since `AudienceMintPolicy` has no `from_env`:

```rust
let mint_policy: Arc<dyn MintPolicy> = Arc::new(
    AudienceMintPolicy::new(AudienceGrants::from_env("")?).with_store(
        analytics_keys_pool.as_ref().map(|pool| {
            Arc::new(DbAudienceGrantsSource::new(
                dedicated_key_store_pool(pool),
                DbAudienceGrantsConfig::from_env_with_prefix(""),
            ))
        }),
    ),
);
```

Empty prefix (`""`), matching `default_key_audience_from_env("")` and `flight_sql_server.rs`'s own
standalone-service convention. The store is `None` only when `analytics_keys_pool` is `None`
(`MICROMEGAS_SQL_CONNECTION_STRING` unset) — same graceful-without-DB shape every other state in
this module already uses; the env-map grants still work with no store attached.

Add `mint_policy: Arc<dyn MintPolicy>` as a new field on `IngestionKeysState`
(`ingestion_keys.rs:53-61`), set from the value above.

### 4. `mint_key` calls `MintPolicy::resolve_audience`

`ingestion_keys.rs::mint_key` (lines 211-260):

```rust
async fn mint_key(
    Extension(state): Extension<IngestionKeysState>,
    AuthenticatedUser(caller): AuthenticatedUser,   // was: AdminUser(user): AdminUser
    Json(body): Json<MintRequest>,
) -> Result<(StatusCode, Json<MintResponse>), IngestionKeyError> {
    let pool = require_pool(&state)?;
    validate_name(&body.name)?;

    let requested = body.audience.as_deref().filter(|s| !s.is_empty())
        .or(state.default_audience.as_deref());
    let audience = state.mint_policy
        .resolve_audience(&caller, requested)
        .await
        .map_err(|e| IngestionKeyError::Forbidden(e.to_string()))?;

    // ... unchanged INSERT ...
    let created_by = caller.email.clone().unwrap_or_else(|| caller.subject.clone());
    // ...
}
```

The `state.default_audience` fallback stays — it doesn't bypass authorization, since the
(possibly-defaulted) audience still goes through `resolve_audience`; an admin-configured team
default that a non-admin caller has no grant for is still a 403. The free `resolve_audience`
function (lines 172-193) is untouched, still used by `import_key`.

Add `IngestionKeyError::Forbidden(String)` (403, `{code: "FORBIDDEN", message}`) and update the
enum's stale doc comment (currently: a `Forbidden` variant would be "dead code" since the admin
gate handled it — no longer true for `mint_key`).

`list_keys`, `revoke_key`, `import_key` stay `AdminUser`-gated, untouched — administration
operations over *other* users' keys are a different authorization question than minting your own;
this stage's decision (per the issue discussion) narrows only "who may call the mint route."

### 5. Setup script

New module `python/micromegas/micromegas/cli/setup_ingestion.py`, registered as
`micromegas-setup-ingestion = "micromegas.cli.setup_ingestion:main"` in `pyproject.toml:36-41`.

- **Auth**: reuse `import_keys.py::build_auth_provider`/`make_client`'s exact shape verbatim —
  client-credentials env vars first, else `config.resolve_connection(profile=args.profile)` →
  `oidc_connection.load_or_login(...)`, which does the interactive loopback-redirect browser login
  on first run and reuses the cached token after that. No new OIDC code.
- **Mint**: add `WebClient.mint_ingestion_api_key(self, name, audience=None) -> dict` to
  `web_client.py`, mirroring `import_ingestion_api_key` (lines 99-124) — `POST
  ingestion-api-keys` with `{"name": name}` plus `"audience"` only when not `None`; returns the
  mint response including the one-time cleartext `key`.
- **CLI args** (argparse, `import_keys.py`'s style): `--url` (required, analytics-web-srv base
  URL), `--profile` (optional), `--name` (required, e.g. hostname), `--audience` (optional —
  omitted lets the server apply its default or reject if the caller has neither a default nor a
  grant), `--otlp-endpoint` (required — the ingestion service's own OTLP URL; a separate service
  from `--url`, no sane default), `--env-file PATH` (optional — write to a file instead of stdout).
- **Output**: `export OTEL_EXPORTER_OTLP_ENDPOINT=<--otlp-endpoint>` and
  `export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer <key>"` — capitalized `Authorization=`
  with `=`, matching the already-shipped format in `mkdocs/docs/otlp/index.md:42-59` (the issue
  text's lowercase `authorization=Bearer <key>` is stale relative to what OTLP ingestion actually
  parses). A one-line human-readable confirmation (key id, audience, name) goes to stderr, so
  stdout stays clean for `eval "$(micromegas-setup-ingestion ...)"`; with `--env-file`, print the
  file path instead.

## Implementation Steps

### Phase 1 — Rust: policy wiring + route change
1. `auth/handlers.rs`: add `AuthenticatedUser` extractor + `Unauthenticated` rejection type (§1).
2. `web_server.rs`: fix the `--disable-auth` `AuthContext` gap (§2); build `mint_policy` and add it
   to `IngestionKeysState` (§3).
3. `ingestion_keys.rs`: switch `mint_key` to `AuthenticatedUser` + `MintPolicy::resolve_audience`
   (§4); add `IngestionKeyError::Forbidden`.

### Phase 2 — Rust tests
4. `analytics-web-srv/tests/ingestion_keys_tests.rs`: add `mint_policy` to every `IngestionKeysState`
   construction (default: `Arc::new(AudienceMintPolicy::new(AudienceGrants::empty()))`, reproducing
   today's "admin only" behavior for tests that don't care); extend `build_handler_router_with_user`
   (line 68) to also layer an `AuthContext`; add an `AuthContext`-builder test helper (mirror
   `auth/tests/policy_tests.rs::caller`, lines 19-38, since it isn't exported). Update the existing
   `mint_403_for_non_admin`-style tests for the new denial path; add a positive test (non-admin
   with a matching `mint` grant succeeds) and a negative test (`requested: None`, no
   `default_audience`, still rejected for a non-admin caller).

### Phase 3 — Python setup script
5. `web_client.py`: add `mint_ingestion_api_key`.
6. New `cli/setup_ingestion.py` + `pyproject.toml` entry point.
7. Tests: `tests/test_web_client.py` (mint method), new `tests/cli/test_setup_ingestion.py`
   (arg parsing + output formatting, mocked `WebClient`).

### Phase 4 — Docs
8. `mkdocs/docs/admin/authentication.md`: replace the "deferred to Stage 6" placeholders
   (~334-335, ~363-364) with a worked mint-grant example and the new script's usage.
9. `mkdocs/docs/admin/api-keys.md`: narrow the "gated by ... `AdminUser`" language (~90-91,
   ~313-315) to describe mint's new self-service path.
10. `tasks/data_isolation/audience_based_access_control_plan.md`: append a "Stage 6 landed" status
    block, following the Stage 5/6a convention already in the doc.

## Files to Modify

- `rust/analytics-web-srv/src/auth/handlers.rs`: `AuthenticatedUser` extractor.
- `rust/analytics-web-srv/src/web_server.rs`: disable-auth `AuthContext` layer; `mint_policy`
  construction; `IngestionKeysState` field wiring.
- `rust/analytics-web-srv/src/ingestion_keys.rs`: `mint_key`, `IngestionKeysState`,
  `IngestionKeyError`.
- `rust/analytics-web-srv/tests/ingestion_keys_tests.rs`: state construction, router-building
  helper, new/updated tests.
- `python/micromegas/micromegas/web_client.py`: `mint_ingestion_api_key`.
- `python/micromegas/micromegas/cli/setup_ingestion.py` (new).
- `python/micromegas/pyproject.toml`: new script entry.
- `python/micromegas/tests/test_web_client.py`, `python/micromegas/tests/cli/test_setup_ingestion.py`
  (new).
- `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/admin/api-keys.md`.
- `tasks/data_isolation/audience_based_access_control_plan.md`.

## Trade-offs

- **Only the mint route becomes self-service; list/revoke/import stay admin-only.** Narrower than
  making the whole `ingestion-api-keys` surface non-admin, but that's what the issue/plan actually
  scoped ("who may call the route" — singular), and there's no use case yet for a non-admin
  listing or revoking arbitrary keys. Widening later is additive (a new extractor + per-key
  ownership check), not a rework.
- **`state.default_audience` fallback kept for non-admin callers**, rather than restricting the
  default to admin-only mints. Accepted because it can't escalate privilege — the (possibly
  defaulted) audience still goes through `MintPolicy`, so a non-admin without a grant for the
  default audience still gets denied.
- **One dedicated pool for the mint-side grant store** (`dedicated_key_store_pool` off the same
  `analytics_keys_pool`), not a shared `Arc<DbAudienceGrantsSource>` with the read side — there is
  no read-side `DbAudienceGrantsSource` in `analytics-web-srv` at all today (that lives in
  flight-sql-srv/monolith, a different process). Building analytics-web-srv's own store instance
  is simplest and matches the existing "each process builds its own" pattern; the two processes'
  independent cache TTLs are an accepted, already-documented property of the design (`AbAC plan`,
  "revocation takes effect within the cache TTL").
- **Setup script reuses the existing loopback-redirect flow, not a new device-code flow**, despite
  the original issue text saying "device-code/loopback." No device-code flow exists anywhere in
  the repo, and the loopback flow already does everything needed (works for any workstation that
  can open a local port and a browser, which is the actual target — this is a developer/operator
  setup script, not a headless/TV-remote scenario device-code flow exists for). Building a second,
  redundant interactive flow was rejected as unjustified scope.

## Security

- **No new confidentiality surface.** Mint remains write/integrity-only per the AbAC design's
  load-bearing property (`audience_based_access_control_plan.md`, "Load-bearing property
  preserved") — a self-service mint grants the ability to *write* under an audience, never to
  *read* it; reading still requires an independent `read` grant. A compromised self-service mint
  path can pollute an audience's data, not exfiltrate it.
- **`AudienceMintPolicy::resolve_audience` is the sole authorization decision** for the new path;
  it is already unit-tested (Stage 4) for the admin/non-admin/no-grant/malformed-audience cases.
  This stage adds no new authorization logic, only a new caller of already-vetted logic — the risk
  surface is the wiring (extractor, disable-auth parity), not the policy itself.
- **The `--disable-auth` fix must exactly mirror the existing `is_admin: true` `ValidatedUser`** —
  getting this wrong (e.g. a *non*-admin `AuthContext`) would silently change dev/CI behavior in a
  way live deployments wouldn't hit, since `--disable-auth` is never used in production per its own
  startup warning.

## Testing Strategy

- Rust: `cargo test -p micromegas-analytics-web-srv -p micromegas-auth`; `cargo clippy --workspace
  -- -D warnings`; `cargo fmt`.
- Python: `poetry run pytest tests/test_web_client.py tests/cli/test_setup_ingestion.py` from
  `python/micromegas/`.
- No automated test for the interactive browser login itself — no mock OIDC IdP exists in this
  repo (confirmed; every other interactive CLI has the same gap). Manual verification instead:
  start services (`python3 local_test_env/ai_scripts/start_services.py --monolith`), create a
  `mint` grant for a test user via the existing `audience-grants` admin API/`micromegas-grants`
  CLI, then confirm end-to-end: (a) a non-admin without a grant gets a clean 403 from `mint_key`,
  (b) a non-admin with a matching grant mints successfully and `micromegas-setup-ingestion` prints
  usable `OTEL_EXPORTER_OTLP_*` exports, (c) an admin can still mint any audience unchanged.

## Documentation

- `mkdocs/docs/admin/authentication.md` and `mkdocs/docs/admin/api-keys.md` (see Implementation
  Steps 8-9).
- `tasks/data_isolation/audience_based_access_control_plan.md` status block (step 10).

## Open Questions

- **Setup script name**: `micromegas-setup-ingestion` is a placeholder — open to a better name
  (e.g. `micromegas-mint-key`) if the maintainer prefers.
- **`--otlp-endpoint` default**: no existing config surface names the ingestion OTLP URL
  (`~/.micromegas/config.json`'s `ConnectionConfig` covers the analytics/web URL only), so this
  plan makes it a required flag. Worth a follow-up if a config-file field turns out to be wanted
  instead.
