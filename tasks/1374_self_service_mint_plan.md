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
existing `is_admin` arm) — nothing regresses for the open/team deployment story. Both the
grant-based non-admin mint path and the lazy claim below are gated behind a single off-by-default
deployment knob, `MICROMEGAS_SELF_SERVICE_MINT` (§3): an existing deployment that upgrades to this
stage keeps today's admin-only mint behavior, unchanged, until an operator explicitly opts in, and
a pair of per-caller bounds (§4a) then limit how many audiences one caller may claim and how many
live keys one caller may hold once it's on. A companion Python setup script does an OIDC
interactive login, mints a personal key, and writes the OTLP exporter env vars a user needs to
point their own telemetry at the deployment.

**Audiences are created lazily, not pre-provisioned.** Consulting grants alone is not enough to
make minting actually self-service: the DB grant store (Stage 6a, #1489) has no write path a
non-admin can reach, so under a grants-only design every *first* personal audience
(`alice-laptop`) would still need an admin to `POST /api/audience-grants` before the matching
`micromegas-setup-telemetry` run could succeed — the exact manual step this stage exists to
remove. The AbAC master plan anticipated this directly:
`tasks/data_isolation/audience_based_access_control_plan.md:1360-1361` — "a per-user audience
needs an explicit grant entry... minting a personal key and creating its matching grant happen in
the same flow." §4a below makes that concrete: a non-admin caller who names a brand-new,
never-before-granted audience *and supplies the name explicitly* claims it — atomically, as part
of the same mint request — rather than being denied for lack of a pre-existing grant. Naming an
audience that already has any grant row in the DB store — admin-created, self-created by an
earlier claim, or someone else's in-flight claim — still requires a matching grant, exactly as
originally planned; nothing about the grants-only authorization decision changes for that case.

**Mint grants are DB-only; the env map is not wired into the mint policy at all.** §3 builds
`AudienceMintPolicy` from an *empty* env grant map plus the DB store — unlike the read side
(`AudienceReadPolicy`), which Stage 6a deliberately kept unioning `{prefix}_AUDIENCE_GRANTS` as
"the static/bootstrap layer" (`tasks/completed/1489_db_audience_grant_store_plan.md:15`). That
choice was right for reads (an open/team deployment's env-declared grants keep working
untouched) but is wrong for mint once claims are lazy: the existence check a claim relies on
("does this audience already have a grant?") can only see what the DB store can see, so a mint
audience declared *only* in someone's env map would look unclaimed and be squattable (§4a,
Security). Restricting mint grants to the DB closes that gap by construction — there is nothing
env-side left for the claim's existence check to miss.

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
    `caller.groups`). This stage wires `grants` to always be `AudienceGrants::empty()` (§3) — only
    the DB store side of this check is ever live for mint.
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
  `~/.micromegas/config.json` defaults. `ConnectionConfig.uri` (`cli/config.py:47-56`, default
  `grpc://localhost:50051`) is the **FlightSQL gRPC URI** — a different service and port from both
  `analytics-web-srv`'s HTTP `--url` and the ingestion service's OTLP endpoint below; an earlier
  draft of this plan mischaracterized it as "the analytics/web URL."
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
  revoke/import, no longer accurate for mint once this stage lands. Beyond those two ranges,
  `api-keys.md:7` ("`analytics-web-srv` is the sole admin HTTP surface"), `:323-326` ("Both route
  groups gate on the same ... admin check"), `:333-334`, and `:539-541` ("`AdminUser` extractor
  rejects any caller whose `is_admin` isn't `true` before a mint/list/revoke/import handler ever
  runs") all go stale the same way and need the same sweep (Implementation step 13).
- **`audience_grants` schema** (migration v7, `rust/ingestion/src/sql_migration.rs:190-201`):
  `CREATE TABLE audience_grants (audience VARCHAR(255), axis VARCHAR(4) CHECK (axis IN ('read',
  'mint')), selector VARCHAR(255), created_at, created_by, PRIMARY KEY (audience, axis,
  selector), CHECK (audience ~ '^[A-Za-z0-9_-]+$'), CHECK (selector = '*' OR selector ~
  '^(user|group):.+$'))`. The natural key is the full triple — there is **no** constraint on
  `audience` alone, so "this audience has zero grant rows of any kind" cannot be answered by an
  `INSERT ... ON CONFLICT` against the primary key: an `ON CONFLICT (audience, axis, selector) DO
  NOTHING` insert only detects a collision on the *exact same* triple, so it would silently
  co-exist with someone else's pre-existing grant on the same audience under a different selector
  or axis — exactly the case a claim must detect and refuse. §4a's claim needs an explicit lock,
  not this constraint, to make "does any row for this audience exist?" race-safe.

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

Both new types must be added to the crate's curated re-export list,
`rust/analytics-web-srv/src/auth/mod.rs:33-36` (`pub use handlers::{AdminRequired, AdminUser,
...}`) — `ingestion_keys.rs` reaches `AdminUser` today only via `crate::auth::AdminUser`, not
`crate::auth::handlers::AdminUser`, since `handlers` is a private module (`mod handlers;`,
`mod.rs:28`); without this, `AuthenticatedUser`/`Unauthenticated` are defined but unreachable from
`ingestion_keys.rs`, a compile error, not a runtime gap.

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

**Correction to this section's original motivation.** An earlier draft justified this fix by
claiming a non-admin extractor reading `AuthContext` "would break local/dev/test runs under
`--disable-auth`" without it. That's not what actually happens: under `--disable-auth`,
`build_protected_routes` merges the static `key_management_disabled_router` in place of the real
key-management routers entirely (`web_server.rs:386-397`), so `mint_key` — along with every other
mint/list/revoke/import handler — is structurally unreachable in that mode regardless of which
extractor it uses; a `--disable-auth` request to `POST /api/ingestion-api-keys` 503s before any
extractor runs, exactly as `disable_auth_ingestion_keys_base_route_returns_503`
(`tests/routing_tests.rs:453-463`) already asserts. Nothing would actually break without this
fix. The layer is added anyway, purely as **defensive parity**: `AuthenticatedUser` should not be
the one extractor in this crate that silently 500s (an unhandled `Extension<AuthContext>` miss)
the day someone changes the disabled-auth branch to merge the real routers for some other reason.
Self-service mint itself remains **unavailable under `--disable-auth`** either way — anyone
verifying this stage end-to-end needs auth enabled (Testing Strategy).

### 3. Wire a `MintPolicy` in `analytics-web-srv` — DB store only, no env grant map

`web_server.rs::run_web_server`, beside the existing `ingestion_keys_state` construction
(~line 667): build the first production `Arc<dyn MintPolicy>`, mirroring the `ReadPolicy` wiring
in §"Current State" but built from parts since `AudienceMintPolicy` has no `from_env`, and
**deliberately not** unioning `{prefix}_AUDIENCE_GRANTS` the way `AudienceReadPolicy::from_env`
does for reads:

```rust
let mint_policy: Arc<dyn MintPolicy> = Arc::new(
    AudienceMintPolicy::new(AudienceGrants::empty()).with_store(
        analytics_keys_pool.as_ref().map(|pool| {
            Arc::new(DbAudienceGrantsSource::new(
                dedicated_key_store_pool(pool),
                DbAudienceGrantsConfig::from_env_with_prefix(""),
            ))
        }),
    ),
);
```

`AudienceGrants::empty()` (infallible — no `?`, unlike `AudienceReadPolicy::from_env`), not
`AudienceGrants::from_env("")?`: mint grants are **DB-only** in this stage (see Overview). The
Stage 6a DB store is the source of truth for who may mint into what; the env map remains, on the
read side only, the static/bootstrap layer Stage 6a kept it as
(`tasks/completed/1489_db_audience_grant_store_plan.md:15`, "unioned with the store"). A
lazily-claiming self-service deployment manages every mint grant in the DB — first through §4a's
claim, thereafter (revocation, re-grants, admin-created team grants) through the existing
`/api/audience-grants` admin API — so there is no bootstrap need for mint the way there is for
read. The store is `None` only when `analytics_keys_pool` is `None`
(`MICROMEGAS_SQL_CONNECTION_STRING` unset) — same graceful-without-DB shape every other state in
this module already uses; with no store attached, `AudienceGrants::empty()` alone denies every
non-admin mint outright (no grant can ever be found), which is the correct fail-closed default
for a deployment that hasn't configured the telemetry DB at all.

Add `mint_policy: Arc<dyn MintPolicy>` as a new field on `IngestionKeysState`
(`ingestion_keys.rs:53-61`), set from the value above.

**Off by default: `MICROMEGAS_SELF_SERVICE_MINT`.** Everything above — building `mint_policy` and
having `mint_key` consult it for non-admin callers — is otherwise unconditional for every
deployment that upgrades to this stage: there is nothing else in this design that keeps an
existing admin-only deployment's authorization surface from silently widening to "every
authenticated user can mint standing credentials and create audiences" the moment it upgrades.
`IngestionKeysState` gains a `self_service_mint_enabled: bool` field, resolved once at startup
from `MICROMEGAS_SELF_SERVICE_MINT` (empty-prefix convention, parsed the same
`v == "true" || v == "1"` way `web_server.rs:123` already parses its own boolean knob), default
`false`. `mint_key` (§4) checks it before ever calling `state.mint_policy.resolve_audience` for a
non-admin caller: with the knob off, a non-admin gets exactly the denial this route always gave
before this stage existed, and the lazy claim (§4a) — which only ever triggers off a non-admin
denial — is never reached either. Admin minting is unaffected either way, since the knob only
gates the caller branches this stage adds. Two more knobs, resolved onto `IngestionKeysState` the
same way, bound the blast radius of turning it on: `MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER`
(`max_claims_per_caller: i64`, default `5`) and `MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER`
(`max_keys_per_caller: i64`, default `20`) — see §4a and §4 for where each is enforced.

**Monolith prefix parity is automatic — no plumbing needed.** An earlier draft of this plan
flagged that monolith namespaces the web role's other knobs under `MICROMEGAS_ANALYTICS`
(`admin_var_name`, `main.rs:295-300`) while `AudienceGrants::from_env("")` would have read only
the unprefixed `MICROMEGAS_AUDIENCE_GRANTS`. That mismatch is moot now: the mint policy never
reads `{prefix}_AUDIENCE_GRANTS` in any form (above), so there is no prefixed env var for a
monolith operator to set and have silently ignored. The only env knob the mint policy still reads
is the TTL, `DbAudienceGrantsConfig::from_env_with_prefix("")` — an empty-prefix call always
resolves to the unprefixed `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS`
(`resolve_prefixed_var`/`resolve_u64`, `env.rs:15-27`, `db_api_key.rs:81-91`), so a monolith
operator who sets `MICROMEGAS_ANALYTICS_AUDIENCE_GRANT_CACHE_TTL_SECONDS` for the flightsql
read-side store (which *does* check that prefixed name first, `main.rs:271-281`) won't have it
picked up by the mint-side store too; both then fall back to the same unprefixed default (60s).
Accepted without further plumbing — unlike the dropped `AUDIENCE_GRANTS` mismatch, getting this
one "wrong" only means the mint store's cache staleness window can silently differ from the read
store's under monolith, never an authorization decision, so it isn't worth the same threading
effort `admin_var_name` gets.

### 4. `mint_key` calls `MintPolicy::resolve_audience`, preserving today's 400s

`ingestion_keys.rs::mint_key` (lines 211-260). **Format/defaulting validation still runs first,
through the untouched free `resolve_audience` function (lines 172-193), before any authorization
decision** — this is what keeps the two existing 400 tests passing unchanged
(`tests/ingestion_keys_tests.rs::mint_400_for_invalid_audience`, lines 303-322, and
`::mint_400_names_the_default_audience_knob`, lines 324-345, which asserts the body names
`MICROMEGAS_DEFAULT_KEY_AUDIENCE`). Routing *every* `MintPolicy::resolve_audience` error straight
to a new `Forbidden` (403) — the naive version of this change — would turn both into 403s, since
`AudienceMintPolicy::resolve_audience`'s own `requested: None` and malformed-audience arms
(`policy.rs:548-592`) produce generic messages that don't match `mint_400_names_the_default_audience_knob`'s
assertion at all. Key material is generated up front, before the policy call, so both the ordinary
and the lazy-claim path (§4a) share one value to insert — no path re-derives or double-inserts it.
Instead:

```rust
async fn mint_key(
    Extension(state): Extension<IngestionKeysState>,
    AuthenticatedUser(caller): AuthenticatedUser,   // was: AdminUser(user): AdminUser
    Json(body): Json<MintRequest>,
) -> Result<(StatusCode, Json<MintResponse>), IngestionKeyError> {
    let pool = require_pool(&state)?;
    validate_name(&body.name)?;

    // Unchanged: format-validate + apply `state.default_audience`, exactly as today. `?` here
    // is what preserves the exact existing 400 bodies -- `MintPolicy::resolve_audience`'s own
    // `requested: None` / malformed-audience arms are never reached from this route at all.
    let candidate = resolve_audience(&state, body.audience.as_deref(), None)?;

    // Off-by-default gate (§3): with the knob unset, a non-admin gets exactly the denial this
    // route always gave before this stage existed, and the lazy claim below (which only ever
    // triggers off a non-admin denial) is never reached either.
    if !caller.is_admin && !state.self_service_mint_enabled {
        return Err(IngestionKeyError::Forbidden(
            "self-service minting is disabled".to_string(),
        ));
    }

    // Key material, generated here -- before the policy call, not after -- so both branches
    // below share one value to insert; the claim path (§4a) needs it to mint inside the same
    // transaction that writes the grant rows.
    let key = generate_key();
    let hash = hash_key(&key);
    let key_id = Uuid::new_v4();
    let created_at = Utc::now();

    // Per-caller bound (§4a): caps how many *live* keys one non-admin may hold, regardless of
    // which path below mints the next one. Admins are exempt -- this bounds self-service, not
    // administration. Note this cap, once hit, has no self-service way down: `list_keys`/
    // `revoke_key` stay `AdminUser`-gated (below), so a caller can neither see which of their own
    // keys are counted here nor revoke one themselves -- reducing the count always requires an
    // admin (Trade-offs).
    if !caller.is_admin {
        let caller_id = caller.email.as_deref().unwrap_or(&caller.subject);
        let key_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ingestion_api_keys WHERE created_by = $1 AND revoked_at IS NULL",
        )
        .bind(caller_id)
        .fetch_one(&pool)
        .await?;
        if key_count >= state.max_keys_per_caller {
            return Err(IngestionKeyError::Forbidden(format!(
                "caller already holds {key_count} live self-service keys (limit {})",
                state.max_keys_per_caller
            )));
        }
    }

    let audience = match state.mint_policy.resolve_audience(&caller, Some(&candidate)).await {
        Ok(aud) => aud,
        // The policy itself could not be evaluated (e.g. a grant-store cold-start outage,
        // `DbAudienceGrantsSource::current`) -- not a denial, and must not be treated as one:
        // surfacing it as `Forbidden` (or, worse, routing it into the claim path below) would
        // misattribute a transient store outage as "you have no grant" for up to the store's
        // cache TTL, and could even let a caller "claim" an audience whose real grant just
        // happened to be unreadable at that moment.
        Err(e) if e.downcast_ref::<ProviderUnavailable>().is_some() => {
            return Err(IngestionKeyError::Unavailable(e.to_string()));
        }
        Err(e) if caller.is_admin => return Err(IngestionKeyError::Forbidden(e.to_string())), // malformed-audience arm; `candidate` is already valid-format, so unreachable in practice
        Err(_) => {
            // Non-admin, genuinely no grant for `candidate` (a store outage is ruled out above)
            // -- try the lazy claim (§4a) only when the caller explicitly named this audience
            // (not merely `state.default_audience`).
            let explicit = body.audience.as_deref().filter(|s| !s.is_empty()).is_some();
            match (explicit, caller.email.as_deref()) {
                (true, Some(_email)) => {
                    // Commits its own grant + key rows (or, on a self-healing recheck, just the
                    // key row -- §4a) and returns the finished response directly -- `mint_key`
                    // never reaches the `INSERT` below for this path.
                    return try_claim_and_mint(
                        &pool, &state, &candidate, &caller, &body, key, key_id, &hash, created_at,
                    )
                    .await
                    .map(|resp| (StatusCode::CREATED, Json(resp)));
                }
                _ => return Err(IngestionKeyError::Forbidden(format!(
                    "audience {candidate:?} is not in the caller's mintable set"
                ))),
            }
        }
    };

    // Ordinary (non-claim) path: single INSERT, as today, using the `key`/`hash`/`key_id`/
    // `created_at` generated above.
    let created_by = caller.email.clone().unwrap_or_else(|| caller.subject.clone());
    // ...
}
```

The `state.default_audience` fallback stays — it doesn't bypass authorization, since the
(possibly-defaulted) audience still goes through `resolve_audience`; an admin-configured team
default that a non-admin caller has no grant for is still a 403, and (per the `explicit` check
above) never claimable, so a shared default audience can't be squatted out from under later
callers who rely on it (§4a, Trade-offs).

Add `IngestionKeyError::Forbidden(String)` (403, `{code: "FORBIDDEN", message}`) and
`IngestionKeyError::Unavailable(String)` (503, `{code: "UNAVAILABLE", message}` — distinct from
the existing `NotConfigured`, whose message is specifically about an unset
`MICROMEGAS_SQL_CONNECTION_STRING` and would mislead when the real cause is a *configured* store
that failed to answer). Update the enum's stale doc comment (currently: a `Forbidden` variant
would be "dead code" since the admin gate handled it — no longer true for `mint_key`).
`IngestionKeyError::BadRequest` needs no new variant or wiring — it is exactly the existing 400
path `resolve_audience`/`validate_name` already raise.

`list_keys`, `revoke_key`, `import_key` stay `AdminUser`-gated, untouched — administration
operations over *other* users' keys are a different authorization question than minting your own;
this stage's decision (per the issue discussion) narrows only "who may call the mint route."

### 4a. Lazy audience claim on mint

**Trigger.** Only when: self-service minting is enabled (`state.self_service_mint_enabled`, §3),
`state.mint_policy.resolve_audience` denied a **non-admin** caller (never a store-outage `Err` —
ruled out in §4), the caller supplied `body.audience` explicitly (not merely the
`state.default_audience` fallback), and `caller.email` is `Some` (a claim writes `user:<email>`
selectors — a caller authenticated only by `subject`, e.g. a non-OIDC service credential, has no
selector form to claim with under today's model and is denied the ordinary way instead). This is
deliberately non-admin-only: an admin's mint always succeeds via `AudienceMintPolicy`'s `is_admin`
arm and so never falls into this branch — an admin minting a genuinely fresh audience gets **no**
grant rows written server-side, exactly like an admin minting an existing one, since the admin arm
is a pure authorization bypass, not a place this stage adds DB writes to (Current State). That
means an admin who wants to read what their own newly-minted key uploads needs a `read` grant some
other way — see §6, which has the setup script create one via the existing admin grants API when
the caller minting a fresh audience is itself an admin.

**Per-caller claim bound.** Nothing else in this design limits how many audiences one caller may
claim — `analytics-web-srv` has no rate-limiting or quota machinery of any kind today, and a claim
creates a new, standing audience plus a new key. `MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER`
(empty-prefix, default `5`), resolved once at startup onto `IngestionKeysState` alongside
`self_service_mint_enabled` (§3): the count of distinct audiences already carrying a `mint` row
for `selector = 'user:<caller email>'` is checked inside the same locked transaction, but only
*after* the existence check below has determined the audience is genuinely unowned — a caller who
turns out to already hold a matching grant (the self-healing recheck below) is minting, not
claiming, writes no new grant row, and so is never charged against this bound. At or above the
limit, the claim is refused with a `Forbidden` naming the limit, never a 500 or a silent no-op. An
admin can raise the knob, or free room by revoking one of the caller's existing claimed audiences
through the existing `DELETE /api/audience-grants` admin route (Trade-offs).

**Eligibility short-circuit (no DB round trip).** Before touching the DB, reject the claim outright
if `candidate == PUBLIC_AUDIENCE` — the one reserved name, matching `is_valid_audience`'s format
check plus this one extra rule. (`public` already can't be *read*-restricted since every
authenticated caller reads it regardless of grants, `AudienceReadPolicy::resolve`; reserving it
from mint-claims keeps a non-admin from ever claiming exclusive *mint* rights over the one
audience every reader can see.) The same short-circuit also rejects `candidate` when it equals
`state.default_audience` — analytics-web-srv's own knob, read under this process's own convention,
so no cross-process prefix question arises.

**The unstamped-audience label is *not* special-cased here — it is protected by a placeholder
grant instead, the same mechanism already used for reserving any other name (Trade-offs).** An
earlier draft of this design had `IngestionKeysState` read
`{prefix}_UNSTAMPED_AUDIENCE`/`MICROMEGAS_UNSTAMPED_AUDIENCE` directly to exempt that name too, but
that knob belongs to the analytics/flight-sql role's `IsolationConfig::from_env(prefix)`
(`rust/analytics/src/lakehouse/read_scope.rs`), resolved against *that* role's prefix (e.g.
monolith's analytics role reads `MICROMEGAS_ANALYTICS_UNSTAMPED_AUDIENCE` first,
`rust/monolith/src/main.rs:291`; `mkdocs/docs/admin/monolith.md:52`) — a prefix
`analytics-web-srv` has no reason to know and does not otherwise read (it uses the empty-prefix
convention for its own knobs, Current State). Re-deriving the name under the wrong prefix would
silently miss it in exactly the deployments that set it, letting a non-admin claim the
deployment-wide unstamped label. Rather than plumb a second prefix into `analytics-web-srv` to fix
that, this design drops the env-derived arm entirely: an operator whose deployment uses an
unstamped-audience label must pre-create a placeholder `audience_grants` row for that audience
(any selector — even one matching nobody, or `axis = 'read'` with `selector = '*'`, which is
already the effective read policy for `public`-like labels) via the existing admin
`POST /api/audience-grants` route before turning on `MICROMEGAS_SELF_SERVICE_MINT`. That row alone
makes the label read as already-owned to the DB existence check below — no separate env-derived
carve-out needed. Mint grants are otherwise DB-only (§3) — the DB existence check below (extended
to also cover `ingestion_api_keys`, not `audience_grants` alone) plus `state.default_audience`
plus this placeholder-grant convention together are the complete "does this audience have an
owner?" answer.

**The claim itself — one transaction, on the same pool `mint_key` already has.** The
`audience_grants` table's natural key is the full `(audience, axis, selector)` triple (Current
State) — there is no constraint that makes "this audience has zero rows, on any axis, under any
selector" atomic on its own. An `INSERT ... ON CONFLICT (audience, axis, selector) DO NOTHING`
only guards the exact triple being inserted; it would happily co-insert a second claimant's
`user:<their email>` mint/read rows alongside a first claimant's already-committed ones for the
same audience, silently creating two "owners" instead of refusing the second. Worse, `audience_grants`
alone cannot tell a genuinely-fresh audience from one that already carries live keys and live data
with zero grant rows: migration v6 backfilled every pre-existing key to `public` (a reserved name,
already excluded above), but any audience an admin minted straight through
`AudienceMintPolicy`'s `is_admin` arm before this stage's first grant ever existed for it —
exactly the shape the AbAC master plan's own privacy-deployment example describes
(`MICROMEGAS_AUDIENCE_GRANTS`+`MICROMEGAS_DEFAULT_KEY_AUDIENCE` naming an audience with live
admin-minted keys and no grant rows) — would look unclaimed to a check that only reads
`audience_grants`, letting a non-admin claim exclusive `read` access over that audience's
already-ingested data. The claim therefore takes an explicit lock and checks both tables:

```rust
async fn try_claim_and_mint(
    pool: &PgPool,
    state: &IngestionKeysState,
    audience: &str,
    caller: &AuthContext,
    body: &MintRequest,
    key: String, key_id: Uuid, hash: &[u8], created_at: DateTime<Utc>,
) -> Result<MintResponse, IngestionKeyError> {
    let mut tx = pool.begin().await?;

    // `pg_try_advisory_xact_lock` (non-blocking), not the blocking `pg_advisory_xact_lock`: this
    // transaction runs on `analytics_keys_pool`, a 2-connection pool shared with every other
    // admin route in this crate (`AnalyticsKeysState`/`AudienceGrantsState`, `web_server.rs:639-
    // 648`). A blocking acquire would park a pooled connection for as long as a concurrent claim
    // (this process or another) holds the lock; with only two connections, a third concurrent
    // request against any of those states would then fail pool acquisition after the pool's 2s
    // `acquire_timeout` -- a 500. The `_try_` form returns immediately instead. Postgres advisory
    // locks are server-instance-wide, not connection- or pool-scoped, so this still correctly
    // serializes two concurrent claims for the *same* audience name across different
    // pools/processes (mint_key's own pool here vs. the mint policy's dedicated
    // DbAudienceGrantsSource pool, §3) -- a plain row lock can't be taken here instead, since
    // there is no pre-existing row for a genuinely-fresh audience to lock. `_xact_lock` (not the
    // session-level `pg_advisory_lock`) releases automatically at COMMIT/ROLLBACK either way, so
    // a claim that errors out never leaks a held lock into the next request the pool hands this
    // connection to.
    let locked: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))",
    )
    .bind(audience)
    .fetch_one(&mut *tx)
    .await?;
    if !locked {
        tx.rollback().await?;
        return Err(IngestionKeyError::Forbidden(format!(
            "audience {audience:?} is being claimed by another request -- retry"
        )));
    }

    let caller_email = caller
        .email
        .as_deref()
        .expect("mint_key only calls try_claim_and_mint when caller.email is Some");
    let selector = format!("user:{caller_email}");

    // "Does this audience already have an owner?" -- true for any grant row (any axis/selector)
    // *or* any existing `ingestion_api_keys` row (see prose above: `audience_grants` alone misses
    // admin-minted, ungranted audiences).
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM audience_grants WHERE audience = $1)
            OR EXISTS(SELECT 1 FROM ingestion_api_keys WHERE audience = $1)",
    )
    .bind(audience)
    .fetch_one(&mut *tx)
    .await?;

    if exists {
        // Self-healing recheck, uncached -- unlike `state.mint_policy.resolve_audience`, which
        // answers from `DbAudienceGrantsSource`'s <=`cache_ttl_secs` snapshot (Cache TTL, below).
        // A mint-eligible caller lands in this function whenever a real `mint` grant already
        // exists in the DB but hasn't reached this process's cached snapshot yet -- an admin's
        // just-written grant, or the caller's own prior claim, both within the last
        // `cache_ttl_secs`. Re-reading `audience_grants` directly, inside this same locked
        // transaction, makes that window invisible to callers instead of turning it into a
        // spurious "already exists" 403.
        let mint_selectors: Vec<String> = sqlx::query_scalar(
            "SELECT selector FROM audience_grants WHERE audience = $1 AND axis = 'mint'",
        )
        .bind(audience)
        .fetch_all(&mut *tx)
        .await?;
        if !mint_selectors.iter().any(|s| selector_matches(s, caller)) {
            tx.rollback().await?;
            return Err(IngestionKeyError::Forbidden(format!(
                "audience {audience:?} already exists and the caller has no grant for it"
            )));
        }
        // Caller already holds a matching `mint` grant -- this is an ordinary mint that
        // `resolve_audience` would itself have approved on a fresher snapshot, not a claim: fall
        // through to the key insert below without writing any new grant row.
    } else {
        // Genuinely fresh audience: this is a real claim, so it counts against the per-caller
        // bound, and writes the claim's own grant rows.
        let claim_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT audience) FROM audience_grants WHERE axis = 'mint' AND selector = $1",
        )
        .bind(&selector)
        .fetch_one(&mut *tx)
        .await?;
        if claim_count >= state.max_claims_per_caller {
            tx.rollback().await?;
            return Err(IngestionKeyError::Forbidden(format!(
                "caller has already claimed {claim_count} audiences (limit {})",
                state.max_claims_per_caller
            )));
        }
        for axis in ["mint", "read"] {
            sqlx::query(
                "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
                 VALUES ($1, $2, $3, now(), $4)",
            )
            .bind(audience).bind(axis).bind(&selector).bind(caller_email)
            .execute(&mut *tx)
            .await?;
        }
    }

    sqlx::query(
        "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by, audience)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(key_id).bind(hash).bind(&body.name).bind(created_at).bind(caller_email).bind(audience)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(MintResponse {
        key_id,
        name: body.name.clone(),
        created_at,
        audience: audience.to_string(),
        key,
    })
}
```

Both grant rows land in the *same* table `audience_grants.rs`'s admin routes own — this is the
first place `ingestion_keys.rs` writes to another module's table directly rather than going
through it, a deliberate one-off: the two grant `INSERT`s and the key `INSERT` must commit or roll
back together, which rules out calling `audience_grants.rs::insert_or_get` (its own CTE is one
independent statement, with no transaction handle exposed across modules today). This follows the
same "duplication, accepted" stance both modules' doc comments already state for their
validation/SQL/error shapes.

**Why both axes.** The claim writes `user:<email>` to **both** `mint` and `read` — not mint alone
— so the caller who just claimed the audience can query the data their own new key uploads under
it. Without the read grant, a freshly self-served ingestion key would stamp data the creator could
never see through `AudienceReadPolicy` (Security).

**Cache TTL.** `DbAudienceGrantsSource` snapshots (this process's own store, and any other
process's) have a ~60s TTL (`db_audience_grants.rs:33-42`), and two different directions matter:

- *Into* the transaction: `state.mint_policy.resolve_audience`'s denial that routed this request
  into `try_claim_and_mint` at all may itself be stale -- an admin grant, or the caller's own prior
  claim, can already be committed in the DB without yet being visible to this process's cached
  snapshot. `try_claim_and_mint`'s existence check reads `audience_grants` directly, uncached, so
  this is a self-healing recheck rather than a real claim in that case (the code above) --
  `mint_key`'s caller-visible outcome (success) is the same either way, so this window is invisible
  to callers regardless of which arm actually ran.
- *Out of* the transaction: after a successful claim, `mint_key` returns the `MintResponse` built
  directly from the just-committed transaction, never re-calling
  `state.mint_policy.resolve_audience` to "confirm" it — so this request is never re-denied by a
  stale pre-claim snapshot. The TTL still matters for *other* processes: another
  `analytics-web-srv` replica or flight-sql-srv/monolith's own `DbAudienceGrantsSource` may take up
  to its own TTL to see the new read grant, so the creator's *next* FlightSQL query against the
  freshly-claimed audience could still 0-row (not error) for up to that window if it lands on a
  different process than the one that committed the claim.

**Race outcomes, concretely:**
- Two non-admin callers claim the same fresh audience name concurrently, same or different
  processes: `pg_try_advisory_xact_lock` lets exactly one of them proceed; the other gets `false`
  immediately (no blocking wait, so no pooled connection is held across the contention) and is
  refused with a "being claimed by another request" 403 -- a retry then sees the first claimant's
  now-committed row via the ordinary "no grant" 403. Never a duplicate-owner outcome, and never a
  500: with the wait removed, concurrent claims can no longer exhaust `analytics_keys_pool`'s
  2-connection pool (`web_server.rs:639-648`, shared with every other admin route in this crate)
  the way a blocking lock acquire could.
- A second caller (no grant) later requests the same, now-claimed audience: an ordinary
  `resolve_audience` denial, no claim attempted (the audience already has a `mint` selector row —
  just not one that matches this caller).
- A caller with a real, already-committed `mint` grant for `candidate` (admin-created, or their own
  earlier claim) requests it again before this process's `DbAudienceGrantsSource` snapshot has
  refreshed (within `cache_ttl_secs`, default 60s): `resolve_audience` denies on the stale
  snapshot, routing this into `try_claim_and_mint`, but the uncached existence-plus-match recheck
  there finds the caller's own grant and mints the key without writing a new claim — a plain
  success, never a "no grant" 403 and never counted against the per-caller claim bound.
- `public` requested by a non-admin, or `candidate` equal to `state.default_audience`: denied
  before any DB access (eligibility short-circuit). An unstamped-audience label, if the deployment
  uses one, is instead denied by the ordinary existence check once its placeholder grant row
  exists (§4a) — a DB round trip, but still denied, never claimable.
- A non-admin who already holds `state.max_claims_per_caller` claimed audiences requests a new,
  genuinely-fresh name: denied inside the transaction before either `INSERT` runs (per-caller
  claim bound), not a duplicate-owner outcome and not the ordinary "no grant" message.

### 5. Discoverability: `GET {base_path}/api/audience-grants/mine`

The setup script needs a way to ask "what can I mint into?" without being an admin — today's only
read route on this table, `list_grants` (`audience_grants.rs`), is `AdminUser`-gated (deliberately:
listing *who else* can read/mint an audience is confidentiality-sensitive about *other* callers).
A caller-scoped endpoint answering only "which audiences does *this* caller's own identity match"
carries none of that sensitivity, so it gets its own route and the new `AuthenticatedUser`
extractor (§1), not `AdminUser`:

```rust
/// `GET {base_path}/api/audience-grants/mine` -- audiences `caller` may mint into today, per the
/// DB store's current rows (no cache -- this reads `pool` directly, same as `list_grants`).
/// Caller-scoped, so `AuthenticatedUser` (any authenticated caller), not `AdminUser`: unlike
/// `list_grants`, this can never reveal another principal's selector, only whether *this*
/// caller's own email/groups match one.
async fn my_mint_grants(
    Extension(state): Extension<AudienceGrantsState>,
    AuthenticatedUser(caller): AuthenticatedUser,
) -> Result<Json<Vec<String>>, AudienceGrantError> {
    let pool = require_pool(&state)?;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT audience, selector FROM audience_grants WHERE axis = 'mint'",
    )
    .fetch_all(&pool)
    .await?;
    let mut audiences: Vec<String> = rows
        .into_iter()
        .filter(|(_, selector)| selector_matches(selector, &caller))
        .map(|(audience, _)| audience)
        .collect();
    audiences.sort();
    audiences.dedup();
    Ok(Json(audiences))
}
```

`selector_matches` (`policy.rs:104-117`) is today module-private to `policy.rs`; make it `pub`,
the same call already made for `valid_selector` at Stage 6a for this exact reason (`policy.rs`'s
own doc comment on `valid_selector`: "`analytics-web-srv`'s admin grant-write route ... needs to
run this exact same ... check"). No new selector-matching logic — this route is a filter over
data `AudienceMintPolicy::resolve_audience` already re-derives per-request; it just exposes the
membership test standalone, scoped to the caller's own identity, without exposing the full grant
table.

Deliberately not consulting the env grant map here: mint grants are DB-only in this stage (§3), so
there is nothing there to fold in.

**`/mine` has nothing useful to say for an admin caller.** An admin's mint authority never depends
on a grant row (`AudienceMintPolicy`'s `is_admin` arm, Current State), so this endpoint — a filter
over `audience_grants` — returns `[]` for every admin regardless of what they can mint or already
own. The setup script (§6) must not treat that `[]` the way it treats a non-admin's: for a
non-admin, `[]` really does mean "no mintable audience yet, claim one or ask an admin"; for an
admin it means nothing at all. The script tells the two apart using the caller's own `is_admin`
(already available from the auth response) before deciding which hint to print.

`micromegas-setup-telemetry` calls this when `--audience` is omitted **and the caller is not an
admin**: if exactly one audience comes back, use it silently; if more than one, print the list and
ask the caller to pick one via `--audience`; if none, print a message pointing at claiming a fresh
name via `--audience <new-name>` instead (§4a) or asking an admin for a grant. An admin caller with
`--audience` omitted is asked to pass one explicitly instead — `/mine`'s `[]` would otherwise
print exactly the non-admin "claim a fresh name" hint for a caller who doesn't need one.

### 6. Setup script

New module `python/micromegas/micromegas/cli/setup_telemetry.py`, registered as
`micromegas-setup-telemetry = "micromegas.cli.setup_telemetry:main"` in `pyproject.toml:36-41`.

- **Auth**: reuse `import_keys.py::build_auth_provider`/`make_client`'s exact shape verbatim —
  client-credentials env vars first, else `config.resolve_connection(profile=args.profile)` →
  `oidc_connection.load_or_login(...)`, which does the interactive loopback-redirect browser login
  on first run and reuses the cached token after that. No new OIDC code.
- **Mint**: add `WebClient.mint_ingestion_api_key(self, name, audience=None) -> dict` to
  `web_client.py`, mirroring `import_ingestion_api_key` (lines 99-124) — `POST
  ingestion-api-keys` with `{"name": name}` plus `"audience"` only when not `None`; returns the
  mint response including the one-time cleartext `key`.
- **CLI args** (argparse, `import_keys.py`'s style): `--url` (required, analytics-web-srv base
  URL), `--profile` (optional), `--name` (required, e.g. hostname), `--audience` (optional — a
  fresh name to claim per §4a, an existing audience the caller already has a grant for, or
  omitted entirely to use §5's `/mine` endpoint: exactly one match is used silently, more than one
  is printed for the caller to choose from with `--audience`, and none prints a hint to either
  claim a fresh name or ask an admin for a grant), `--otlp-endpoint` (see below),
  `--env-file PATH` (optional — write to a file instead of stdout).
- **`--otlp-endpoint` default.** `MICROMEGAS_TELEMETRY_URL` (default `http://localhost:9000`) is
  the repo's established ingestion-endpoint convention — `telemetry-sink/src/lib.rs:453` reads it
  directly, and `local_test_env/claude_code_otel.py:73-91` and `mkdocs/docs/otlp/index.md:26,30`
  both derive the OTLP endpoint from it the same way (`{base}/ingestion/otlp`). An earlier draft
  of this plan proposed deriving `--otlp-endpoint` from `--url` instead and, failing that, left it
  a required flag with no default at all — both wrong: `--url` is `analytics-web-srv`'s own base
  URL (a separate service, separate port, from the ingestion endpoint), and `MICROMEGAS_TELEMETRY_URL`
  already names exactly the URL needed. So: `--otlp-endpoint` defaults to
  `f"{os.environ['MICROMEGAS_TELEMETRY_URL'].rstrip('/')}/ingestion/otlp"` when that env var is
  set (mirroring `claude_code_otel.py`'s own derivation verbatim), and is a required flag only when
  it isn't. This closes the Open Question the earlier draft left here — no follow-up needed.
- **Admin callers get their own read grant, since the mint route never writes one for them
  (§4a, Trigger).** After a successful mint, if `--audience` named a brand-new audience and the
  caller's own `is_admin` (from the auth response) is `true`, the script also calls the existing
  admin `POST /api/audience-grants` (Stage 6a) to write `user:<caller email>` on both the `mint`
  and `read` axes for that audience — the same two rows a non-admin's lazy claim (§4a) writes
  server-side, just issued client-side through the admin API the caller's own session already has
  access to. "Brand-new" here means the mint response's `audience` equals the `--audience` the
  caller explicitly passed and no prior `GET /api/audience-grants?audience=<name>` row exists for
  it — the same signal §4a itself uses server-side to distinguish a claim from an ordinary mint.
  Without this step, an admin running the script against a fresh audience name mints successfully
  and can never read what their own new key uploads. A non-fresh `--audience` (one the admin
  already knows has a grant, or one resolved via `/mine`) needs no such step.
- **Output**: `export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf`,
  `export OTEL_EXPORTER_OTLP_ENDPOINT=<--otlp-endpoint>`, and
  `export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer <key>"` — the protocol export is
  required because micromegas exposes OTLP over HTTP only
  (`rust/public/src/servers/otlp.rs:227-229`; no gRPC OTLP receiver exists), so an SDK defaulting
  to gRPC would otherwise fail to reach the endpoint this script just handed it — the same pairing
  `local_test_env/claude_code_otel.py:92-93` and `mkdocs/docs/otlp/index.md:26-27,52-53` already
  use. Capitalized `Authorization=`
  with `=`, matching the already-documented format in `mkdocs/docs/otlp/index.md:42-59` (not, as
  an earlier draft claimed, because the issue text's lowercase `authorization=Bearer <key>` is
  "stale relative to what OTLP ingestion actually parses" — it isn't: the header *name* is parsed
  case-insensitively, `rust/auth/src/types.rs:115-120` (`HeaderMap::get`, whose `HeaderName`
  comparison is inherently case-insensitive), so `authorization=` would work identically; only the
  `Bearer ` value prefix is case-sensitive, `types.rs:90-93` (`strip_prefix("Bearer ")`, exact
  match). The chosen casing is simply consistency with existing docs, not a parsing requirement).
  A one-line human-readable confirmation (key id, audience, name) goes to stderr, so stdout stays
  clean for `eval "$(micromegas-setup-telemetry ...)"`; with `--env-file`, print the file path
  instead.

## Implementation Steps

### Phase 1 — Rust: policy wiring + route change
1. `auth/handlers.rs`: add `AuthenticatedUser` extractor + `Unauthenticated` rejection type (§1).
2. `auth/mod.rs`: add `AuthenticatedUser`/`Unauthenticated` to the `pub use handlers::{...}`
   re-export list, lines 33-36 (§1) — without this the new types are unreachable from
   `ingestion_keys.rs`/`audience_grants.rs`.
3. `web_server.rs`: fix the `--disable-auth` `AuthContext` gap, as defensive parity only (§2);
   build `mint_policy` from `AudienceGrants::empty()` + the DB store, no env map (§3), and add it
   to `IngestionKeysState`.
4. `policy.rs`: make `selector_matches` `pub` (§5, same precedent as `valid_selector` at Stage 6a).
5. `ingestion_keys.rs`: switch `mint_key` to `AuthenticatedUser`, pre-validate via the untouched
   free `resolve_audience` (preserving today's 400s), then consult
   `state.mint_policy.resolve_audience` for authorization only (§4); add the lazy-claim path,
   `try_claim_and_mint` (§4a); add `IngestionKeyError::Forbidden`.
6. `audience_grants.rs`: add the `GET {base_path}/api/audience-grants/mine` route and handler,
   gated by `AuthenticatedUser` (§5).

### Phase 2 — Rust tests
7. `analytics-web-srv/tests/ingestion_keys_tests.rs` **and**
   `analytics-web-srv/tests/routing_tests.rs:405-408` — both construct `IngestionKeysState`
   directly (22 sites total, not just the 21 in `ingestion_keys_tests.rs`) and need the new
   `mint_policy` field (default: `Arc::new(AudienceMintPolicy::new(AudienceGrants::empty()))`,
   reproducing today's "admin only, no store" behavior for tests that don't care). In
   `ingestion_keys_tests.rs`: extend `build_handler_router_with_user` (line 68) to also layer an
   `AuthContext`; add an `AuthContext`-builder test helper (mirror
   `auth/tests/policy_tests.rs::caller`, lines 19-38, since it isn't exported). Update the existing
   `mint_403_for_non_admin`-style tests for the new denial path; confirm
   `mint_400_for_invalid_audience` and `mint_400_names_the_default_audience_knob` (lines 303-345)
   still pass unchanged (§4's whole point). Add: a positive test (non-admin with a matching `mint`
   grant succeeds, no claim attempted); a negative test (`requested: None`, no `default_audience`,
   still rejected for a non-admin caller, still a 400 per §4 — not the claim path, since there's no
   explicit audience to claim); a **live-DB** claim test (fresh audience, non-admin, explicit
   `--audience` — succeeds, and both a `mint` and a `read` row for `user:<caller email>` land in
   `audience_grants`), following the existing `MICROMEGAS_SQL_CONNECTION_STRING`-gated live-DB
   pattern already used by this file (line 455) and `audience_grants_tests.rs`/
   `default_provider_tests.rs`; a second-caller-denied test (a different non-admin, no grant, mints
   against the now-claimed audience — ordinary 403, no claim attempted); a concurrent-claim test
   (two claims for the same fresh name issued concurrently — exactly one succeeds, the other gets
   the ordinary "no grant" 403, never a duplicate-owner state or a 500); `public` rejected for a
   non-admin claim attempt (no DB access, per the eligibility short-circuit).
8. `rust/analytics-web-srv/tests/audience_grants_tests.rs`: tests for `GET
   /api/audience-grants/mine` (§5) — a caller with a matching selector sees the audience, a caller
   without one doesn't, `AdminUser` is not required.

### Phase 3 — Python setup script
9. `web_client.py`: add `mint_ingestion_api_key` and a `my_audience_grants`/`list_mine` call for
   `GET .../audience-grants/mine` (§5).
10. New `cli/setup_telemetry.py` + `pyproject.toml` entry point; `--otlp-endpoint` defaults from
    `MICROMEGAS_TELEMETRY_URL` when set (§6).
11. Tests: `tests/test_web_client.py` (mint + `/mine` methods), new
    `tests/cli/test_setup_telemetry.py` (arg parsing + output formatting, mocked `WebClient`,
    including the `--audience`-omitted / `/mine` resolution paths).

### Phase 4 — Docs
12. `mkdocs/docs/admin/authentication.md`: replace the "deferred to Stage 6" placeholders
    (~334-335, ~363-364) with a worked mint-grant example and the new script's usage; state that
    self-service claims require all mint grants to live in the DB store (§3) — a mint audience
    declared only via `{prefix}_AUDIENCE_GRANTS` is invisible to the claim's existence check and
    could be squatted (Security); document that a deployment using an unstamped-audience label
    (`{prefix}_UNSTAMPED_AUDIENCE`) must pre-create a placeholder `audience_grants` row for it (any
    selector, on either axis) via the admin grants API before enabling self-service mint, so the
    label reads as already-owned to §4a's existence check — the label plays no separate role in
    that check otherwise (§4a); document `MICROMEGAS_SELF_SERVICE_MINT` and the two per-caller
    bound knobs (§3, §4a), including that reaching `MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER`
    requires an admin to revoke one of the caller's keys — `list_keys`/`revoke_key` stay
    `AdminUser`-gated, so a non-admin has no self-service way to reduce their own live-key count
    (Trade-offs); add a `/mine` row (noting it is non-admin-accessible, unlike every other
    row in the table) to the "**HTTP admin routes**, `AdminUser`-gated" route table (~408-416),
    since that header's blanket claim stops being accurate once this row exists.
13. `mkdocs/docs/admin/api-keys.md`: sweep for admin-gating claims that go stale once mint is
    non-admin, not just the two ranges an earlier draft named — `:7` ("sole admin HTTP surface"),
    `:90-91`, `:313-315`, `:323-326` ("Both route groups gate on the same ... admin check"),
    `:333-334`, and `:539-541` ("`AdminUser` extractor rejects any caller ... before a
    mint/list/revoke/import handler ever runs") all need the same correction; also correct
    `:218-222`'s claim that who may "read or mint into" an audience comes from "the
    `{prefix}_AUDIENCE_GRANTS` env map, unioned with the DB-backed `audience_grants` table" — true
    for read, false for mint under §3's DB-only decision.
14. `tasks/data_isolation/audience_based_access_control_plan.md`: append a "Stage 6 landed" status
    block, following the Stage 5/6a convention already in the doc.
15. `CHANGELOG.md`: add an `## Unreleased` entry, following the Stage 1/2/4/6a precedent (#1370,
    #1372, #1373, #1489) — a **Minor breaking change** clause for `IngestionKeysState` gaining
    required `mint_policy`/`self_service_mint_enabled`/`max_claims_per_caller`/
    `max_keys_per_caller` fields and `micromegas_auth::policy::selector_matches`
    becoming `pub`, plus a behavior-change upgrade note covering the new non-admin mint route
    behavior (off by default, `MICROMEGAS_SELF_SERVICE_MINT`), the new `/mine` route, and the new
    `micromegas-setup-telemetry` CLI.
16. `mkdocs/docs/query-guide/python-api.md`: add a `### micromegas-setup-telemetry` subsection
    under `## Command-Line Interface`, alongside `micromegas-logout`/`micromegas-import-keys`/
    `micromegas-grants` (the last added by Stage 6a for its own new CLI), and document
    `WebClient.mint_ingestion_api_key`/the `/mine` call under `## Client Methods`.

## Files to Modify

- `rust/analytics-web-srv/src/auth/handlers.rs`: `AuthenticatedUser` extractor.
- `rust/analytics-web-srv/src/auth/mod.rs`: re-export `AuthenticatedUser`/`Unauthenticated`.
- `rust/analytics-web-srv/src/web_server.rs`: disable-auth `AuthContext` layer; `mint_policy`
  construction (empty grants + DB store, no env map); `IngestionKeysState` field wiring.
- `rust/auth/src/policy.rs`: make `selector_matches` `pub`.
- `rust/analytics-web-srv/src/ingestion_keys.rs`: `mint_key`, `try_claim_and_mint`,
  `IngestionKeysState`, `IngestionKeyError`.
- `rust/analytics-web-srv/src/audience_grants.rs`: new `GET .../audience-grants/mine` route.
- `rust/analytics-web-srv/tests/ingestion_keys_tests.rs`: state construction, router-building
  helper, new/updated tests (including new live-DB claim tests).
- `rust/analytics-web-srv/tests/routing_tests.rs`: `IngestionKeysState` construction (line 406)
  needs the new `mint_policy` field.
- `rust/analytics-web-srv/tests/audience_grants_tests.rs`: `/mine` tests.
- `python/micromegas/micromegas/web_client.py`: `mint_ingestion_api_key`, `/mine` call.
- `python/micromegas/micromegas/cli/setup_telemetry.py` (new).
- `python/micromegas/pyproject.toml`: new script entry.
- `python/micromegas/tests/test_web_client.py`, `python/micromegas/tests/cli/test_setup_telemetry.py`
  (new).
- `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/admin/api-keys.md`.
- `tasks/data_isolation/audience_based_access_control_plan.md`.
- `CHANGELOG.md`: `## Unreleased` entry (minor breaking change + behavior-change upgrade note).
- `mkdocs/docs/query-guide/python-api.md`: `micromegas-setup-telemetry` CLI section + client
  methods.

## Trade-offs

- **Only the mint route becomes self-service; list/revoke/import stay admin-only.** Narrower than
  making the whole `ingestion-api-keys` surface non-admin, but that's what the issue/plan actually
  scoped ("who may call the route" — singular), and there's no use case yet for a non-admin
  listing or revoking arbitrary keys. Widening later is additive (a new extractor + per-key
  ownership check), not a rework. One concrete consequence of the narrower scope: a non-admin who
  reaches `MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER` (§3, §4) has no self-service way to free a
  slot — they can't list which of their own keys are live, let alone revoke one — so ordinary key
  rotation past the cap always requires an admin (below).
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
- **Naming is first-come-first-served** (§4a): any non-admin who can authenticate can claim any
  unclaimed, non-reserved audience name merely by minting against it first. This means squatting is
  possible in the literal sense — a caller can claim a name someone else expected to use (a
  teammate's planned `ci-runner` audience, say) before they do. Accepted, with two mitigations: (1)
  reserved names (`public`, at minimum — Design §4a) keep the one built-in, universally-readable
  audience un-squattable; an operator who wants more names reserved from self-service claiming
  achieves it the ordinary way, by pre-creating a DB grant for that name (any selector, even one
  matching nobody) so §4a's existence check already finds it and refuses the claim outright. (2)
  Claims are fully visible through the existing admin surface — `GET
  /api/audience-grants?audience=<name>` lists exactly who claimed what and when
  (`created_at`/`created_by`) — and correctable, though not by simply deleting the grant:
  `DELETE /api/audience-grants` (Stage 6a) removes only the wrongly-claimed grant rows, and
  §4a's existence check also treats any `ingestion_api_keys` row for the audience as an existing
  owner, with no delete path for key rows (`revoke_key` only sets `revoked_at`) — so a squatted
  name with a minted key can never be re-claimed by anyone, including its rightful owner.
  Recovery is instead: delete the wrong grant via `DELETE /api/audience-grants`, then grant the
  legitimate caller `user:<email>` on both axes for the same audience via the existing admin
  `POST /api/audience-grants` route — the same two rows §4a's own claim would have written,
  issued by an admin instead. Nothing about this stage removes an admin's ability to see or
  correct a claim; it just means correction, not a fresh claim, is the recovery path once a key
  has been minted.
- **Mint grants are DB-only in this stage** (§3) — a deliberate asymmetry with the read side, which
  still unions the env map as its bootstrap layer. §4a's existence check now also covers
  `ingestion_api_keys` and `state.default_audience` (Design §4a, Security), which closes the hazard
  for that configured name automatically; an unstamped-audience label is closed the same way only
  once an operator pre-creates its placeholder grant row (§4a) — a one-time manual step called out
  in the docs (Implementation step 12), not something the existence check does on its own. The
  residual hazard: an audience granted **only** via someone's
  `{prefix}_AUDIENCE_GRANTS` (read or mint) — one that is neither of those two configured names
  and has never had a key minted into it — is still invisible to §4a's checks and so still looks
  unclaimed; a non-admin could claim it out from under the env-declared grant, ending up with
  their own `user:<email>` mint/read rows for a name an operator already meant to govern
  differently. Mitigation/stance: `MICROMEGAS_SELF_SERVICE_MINT` (§3) defaults to `false`
  precisely so this hazard is opt-in, not automatic on upgrade; an operator turning it on must
  keep every mint-relevant audience's grants in the DB, never in the env map — the env map remains
  appropriate only for open/team bootstrap profiles that leave the knob off and don't use §4a at
  all. Documented in the same place as the worked mint-grant example (Implementation step 12).
- **Self-service mint (grant-based and claim-based) shares one knob, `MICROMEGAS_SELF_SERVICE_MINT`**
  (§3), rather than separate knobs per sub-feature. An operator who wants DB mint grants honored
  for non-admins but never wants a caller to create a brand-new audience via §4a cannot express
  that split with this one knob — they can still reach nearly the same outcome by never granting
  `mint` to a selector for an audience that doesn't already exist, since §4a's claim path is only
  ever reached after an ordinary denial. A second, claim-specific knob was rejected as additional
  surface for a distinction no deployment in scope for this stage needs yet.
- **Per-caller claim/key bounds (§4a) are fixed, conservative defaults (5 claims, 20 live keys),
  not derived from any existing quota system** — `analytics-web-srv` has none today. An operator
  who needs a different ceiling raises the corresponding env knob (§3); hitting the default is
  meant to be a rare, deliberate-abuse-shaped event for the developer/operator self-service
  audience this stage targets, not a limit ordinary usage should approach. The two bounds are not
  symmetric in how a caller recovers from hitting them: a claim can be freed by the caller's own
  admin revoking a grant (`DELETE /api/audience-grants`, already covered above), but
  `list_keys`/`revoke_key` stay `AdminUser`-gated (§4), so freeing a key slot always requires an
  admin to revoke one of the caller's keys on their behalf — there is no self-service path down for
  keys. This is accepted as a documented limitation, not fixed in this stage: widening
  `list_keys`/`revoke_key` to a caller's own keys is exactly the kind of additive follow-up the
  first bullet above already anticipates, and the default of 20 is set generously (well above what
  a normal multi-machine setup needs) precisely because reducing it is admin-only.

## Security

- **The entire mint API — including the lazy-claim path and the `/mine` discovery endpoint — stays
  on `analytics-web-srv`, never the ingestion service, and must not move there.** Minting a key is a
  materially riskier operation than accepting an already-authenticated ingestion request (it creates
  a new, standing credential, and — as of §4a — can create a new audience and its grants in the same
  call), so it belongs behind `analytics-web-srv`'s OIDC cookie-session auth, not behind the
  ingestion service's bearer-key/API-key path. The master AbAC plan already fixed this call site:
  "Mint surface moved 2026-08-12" (`audience_based_access_control_plan.md:58-65`, #1411/#1458) — key
  management shipped on ingestion in Stage 0 and was deliberately relocated to
  `analytics-web-srv`/`ingestion_keys.rs::mint_key`, on the grounds that "ingestion should only do
  ingestion" and that the most-exposed, fleet-facing process should hold no `INSERT` on any key
  table at all (`audience_based_access_control_plan.md:976-990`). This stage does not revisit that;
  it adds callers to the same, already-relocated route. The ingestion service's only key-related
  role remains validating a *presented* key against `ingestion_api_keys` and stamping
  `AuthContext.bound_audience` from it — it never mints. This is structural, not just policy: an
  ingestion API key's `AuthContext` is hardcoded `is_admin: false` (`auth/src/api_key.rs:124`) and
  carries `auth_type: AuthType::ApiKey`, but more fundamentally it never reaches
  `analytics-web-srv`'s mint route at all — that route's `AuthenticatedUser`/`AdminUser` extractors
  read the `AuthContext` `cookie_auth_middleware` inserts from a browser OIDC session (Current
  State); there is no bearer-key authenticator on `analytics-web-srv`'s `/api/*` routes for an
  ingestion key to authenticate through in the first place. An ingestion key mint-ing another key is
  not merely forbidden by policy — there is no code path for it to reach `resolve_audience` at all.
- **No new confidentiality surface for the ordinary (non-claim) path.** Mint remains
  write/integrity-only per the AbAC design's load-bearing property
  (`audience_based_access_control_plan.md`, "Load-bearing property preserved") — a self-service
  mint grants the ability to *write* under an audience, never to *read* it; reading still requires
  an independent `read` grant. A compromised self-service mint path can pollute an audience's data,
  not exfiltrate it. The one exception is the claim path's own read grant, addressed below.
- **The claim path (§4a) writes a `read` grant, and that cannot escalate.** A successful claim
  inserts `user:<email>` on *both* the `mint` and `read` axes for the newly-claimed audience — the
  one place this stage grants read access at all. This cannot be used to read anyone else's data,
  but the reason is narrower than "the audience had zero rows a moment earlier": the claim's
  existence check (§4a) requires all of zero `audience_grants` rows, zero `ingestion_api_keys`
  rows, and a name distinct from `state.default_audience` — i.e. an audience genuinely unowned
  by any grant, any admin-minted key, or the deployment's own default, not merely one that
  happens to be absent from `audience_grants` alone. (An operator using an unstamped-audience
  label protects it the same way any other reserved name is protected -- a pre-created
  placeholder grant row, §4a -- which then makes it visible to this same `audience_grants`
  check; there is no separate env-derived carve-out for it, and so no prefix-mismatch gap for
  that carve-out to have.)
  Because no key could have existed for such an audience, no data could have been ingested under
  it either; the claimant is granted read over data only they themselves can produce from here on.
  `mint` and `read` grants otherwise stay strictly independent — `read_audiences` (a key's per-key
  direct read grant) never enters the mintable set (`policy.rs:335-341`; unit-tested,
  `policy_tests.rs:264-274`) and, symmetrically, a `mint` grant never confers read power on its own
  (`policy_tests.rs:218-228`) — the claim is the sole place this stage ties the two axes together,
  and only for a caller's own brand-new audience.
- **`AudienceMintPolicy::resolve_audience` is the sole authorization decision for the non-claim
  path**; it is already unit-tested (Stage 4) for the admin/non-admin/no-grant/malformed-audience
  cases. Its `Err` means "not evaluable" (a store outage) as often as "denied" (Stage 1's own trait
  doc comment); `mint_key` (§4) must, and does, tell the two apart via `ProviderUnavailable` before
  treating an `Err` as authoritative — an outage is a 503, never a 403 and never a route into the
  claim path. This stage adds one new piece of authorization logic beyond wiring — the claim
  transaction itself (§4a) — which is new surface, not already-vetted logic; its correctness rests
  on the advisory-lock existence check being race-safe (§4a), not on `resolve_audience`.
- **Both new caller-facing behaviors are opt-in, not automatic on upgrade.**
  `MICROMEGAS_SELF_SERVICE_MINT` defaults to `false` (§3), so a deployment that upgrades to this
  stage keeps its exact pre-stage mint authorization surface — every non-admin request is denied
  the same way it was before this stage existed — until an operator explicitly turns it on. The
  per-caller claim/key bounds (§4a) then limit the blast radius of that opt-in: once enabled, a
  single credential can create at most `MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER` audiences
  and hold at most `MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER` live keys, never an unbounded
  number of either.
- **The `--disable-auth` fix must exactly mirror the existing `is_admin: true` `ValidatedUser`** —
  getting this wrong (e.g. a *non*-admin `AuthContext`) would silently change dev/CI behavior in a
  way live deployments wouldn't hit, since `--disable-auth` is never used in production per its own
  startup warning. In practice this fix is defensive parity only: self-service mint (and every
  other key-management route) is structurally unreachable under `--disable-auth` regardless
  (Design §2), so getting the layer wrong changes no live behavior either way today.

## Testing Strategy

- Rust: `cargo test -p micromegas-analytics-web-srv -p micromegas-auth`; `cargo clippy --workspace
  -- -D warnings`; `cargo fmt`.
- Python: `poetry run pytest tests/test_web_client.py tests/cli/test_setup_telemetry.py` from
  `python/micromegas/`.
- No automated test for the interactive browser login itself — no mock OIDC IdP exists in this
  repo (confirmed; every other interactive CLI has the same gap). Manual verification instead:
  start services with auth enabled and `MICROMEGAS_SELF_SERVICE_MINT=1` set (the knob defaults to
  `false`, §3, so (b)-(d) below need it explicitly on) — `--disable-auth` structurally cannot
  exercise this path (Design §2), so `python3 local_test_env/ai_scripts/start_services.py
  --monolith` must run against a real OIDC provider, not the disabled-auth default — create a
  `mint` grant for a test user via the existing `audience-grants` admin API/`micromegas-grants`
  CLI, then confirm end-to-end: (a) a non-admin without a grant, requesting an audience that
  already has a grant, gets a clean 403 from `mint_key`; (b) a non-admin with a matching grant
  mints successfully -- including immediately after the grant is created, before the mint-side
  `DbAudienceGrantsSource` snapshot would normally have refreshed, to exercise the uncached
  self-healing recheck (§4a) rather than depending on the cache TTL having already elapsed -- and
  `micromegas-setup-telemetry` prints usable `OTEL_EXPORTER_OTLP_*` exports; (c) a non-admin with
  no grant at all, requesting a brand-new audience name via
  `--audience`, mints successfully and claims it (§4a) — confirmed via `GET
  /api/audience-grants?audience=<name>` showing both the new `mint` and `read` rows; (d) a second
  non-admin then requesting that same, now-claimed audience gets the ordinary 403; (e) an admin
  minting an existing, already-granted audience is unchanged, and an admin minting a brand-new
  audience via `micromegas-setup-telemetry` ends up with a working `read` grant for it too (the
  script's own admin-grants call, §6) and can query the data their own new key uploads, not just
  receive a successful mint response; (f) with `MICROMEGAS_SELF_SERVICE_MINT` unset (the default),
  a non-admin with a matching grant still gets the pre-stage denial, confirming the knob is truly
  off by default.

## Documentation

- `mkdocs/docs/admin/authentication.md` and `mkdocs/docs/admin/api-keys.md` (see Implementation
  Steps 12-13).
- `tasks/data_isolation/audience_based_access_control_plan.md` status block (step 14).
- `CHANGELOG.md` `## Unreleased` entry (step 15).
- `mkdocs/docs/query-guide/python-api.md` `micromegas-setup-telemetry` CLI section (step 16).

## Open Questions

None. The setup script is named `micromegas-setup-telemetry` — from the user's point of view the
script sets up telemetry transmission ("send my data"), so the server-side term "ingestion" stays
out of the user-facing name.
