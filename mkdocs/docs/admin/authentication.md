# Authentication

Micromegas supports unified authentication across all services using both API keys and OpenID Connect (OIDC).

## Overview

Both the analytics server (`flight-sql-srv`) and ingestion server
(`telemetry-ingestion-srv`) support two authentication methods:

- **OIDC (OpenID Connect)** — for human users and service accounts via
  federated identity providers (Google, Azure AD, Okta, Auth0, etc.)
- **API Keys** — bearer token authentication

Both methods can be enabled simultaneously. When multiple providers are
configured, they are tried in order until one succeeds (API key first for
performance, then OIDC).

## Authentication Methods

### OIDC Authentication (Recommended)

OIDC provides federated authentication with automatic token refresh and
support for multiple identity providers.

**Benefits:**

- Standards-based authentication (OAuth 2.0 / OpenID Connect)
- Centralized user management via identity provider
- Automatic token refresh with no manual intervention
- Support for multiple identity providers simultaneously
- Full audit trail with user identity (email, subject)
- Token revocation support via identity provider

**Supported Identity Providers:**

- Google OAuth
- Microsoft Azure AD
- Okta
- Auth0
- Any standards-compliant OIDC provider

### API Keys

Bearer token authentication. Two flavors coexist:

- **Env keyring** — `MICROMEGAS_API_KEYS`, a JSON array parsed once at
  startup. The only option for `object-cache-srv` (no DB connection);
  also usable as a bootstrap path for ingestion and flight-sql.
- **DB-backed keys** — `ingestion_api_keys` / `analytics_api_keys` rows,
  validated by hash lookup. Minted, listed, and revoked over HTTP without a
  redeploy. See [API Keys](api-keys.md) for the full reference.

**Benefits (both flavors):**

- Simple to configure
- Fast validation (HashMap lookup for the env keyring; cached hash lookup for
  DB-backed keys)
- No external identity provider dependency

**Limitations:**

- No automatic expiration — this design adds *revocation*, not expiry; a
  DB-backed key with no `revoked_at` is valid indefinitely
- Rotation is a manual operator action either way
- No per-request user identity beyond the key's own `name`; DB-backed keys
  additionally record `created_by`/`revoked_by`

## Server Configuration

### OIDC Configuration

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://accounts.google.com",
      "audience": "your-app-id.apps.googleusercontent.com"
    },
    {
      "issuer": "https://login.microsoftonline.com/{tenant}/v2.0",
      "audience": "api://your-api-id"
    }
  ],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}'

# Optional: Configure admin users
export MICROMEGAS_ADMINS='["alice@example.com", "bob@example.com"]'
```

**Configuration Fields:**

| Field | Description | Default |
|-------|-------------|---------|
| `issuers` | Array of OIDC issuer configurations | Required |
| `issuers[].issuer` | OIDC issuer URL | Required |
| `issuers[].audience` | Expected audience claim (client_id) | Required |
| `jwks_refresh_interval_secs` | JWKS cache TTL in seconds | 3600 |
| `token_cache_size` | Maximum validated tokens to cache | 1000 |
| `token_cache_ttl_secs` | Token cache TTL in seconds | 300 |

`MICROMEGAS_ADMINS` is a JSON array of user identifiers (email or subject)
with administrative privileges — partition management and other admin SQL
functions.

### API Key Configuration

**DB-backed keys.** Mint an ingestion key with `POST /api/ingestion-api-keys`,
or an analytics key with `POST /api/analytics-api-keys` — both on
`analytics-web-srv` (OIDC + admin required). See [API Keys](api-keys.md) for
the full route reference and the `mmk_`-prefixed key shape.

**Env keyring.** The only option for `object-cache-srv` (permanently — see
[Object Cache](object-cache.md)), and usable as a bootstrap path for
ingestion/flight-sql before any DB-backed key exists:

```bash
export MICROMEGAS_API_KEYS='[
  {"name": "service1", "key": "secret-key-123"},
  {"name": "service2", "key": "secret-key-456"}
]'
```

**Format:**
- JSON array of objects
- Each object has `name` (identifier for logging) and `key` (the actual API key)
- The `key` value is sent as the Bearer token by clients
- Generate keys with: `openssl rand -base64 512` (or, for a DB-backed key, use
  the mint route above instead)

### Disable Authentication (Development Only)

```bash
# Analytics server
flight-sql-srv --disable-auth

# Ingestion server
telemetry-ingestion-srv --disable-auth
```

!!! danger "Security Warning"
    Never disable authentication in production environments. This flag is intended only for local development and testing.

### Audience Filtering Activation

!!! danger "Enabling auth can silently zero out every query result"
    Configuring any authentication method — OIDC, API keys, or both — flips
    every session from the internal `ReadScope::All` marker to
    `ReadScope::Audiences`, which activates query-time audience filtering
    across two enforcement layers.

    **Row-level filtering** (`OwnershipRewrite`) injects an audience
    predicate into every `MaterializedView`-backed query plan, so a caller
    only sees rows whose own per-row `audience` column (`processes`,
    `streams`, `blocks`, `log_entries`, `measures`, `log_stats`) resolves to
    one of their own audiences.

    **Call-level filtering** (`AudienceGuard`) covers five arg-addressed
    functions — `view_instance` joins `process_spans`, `perfetto_trace_chunks`,
    `parse_block`, and `get_payload`. For view sets carrying a physical
    `audience` column, row-level filtering already covers the underlying
    scan; for `net_spans`, `otel_spans`, `images`, `async_events`, and
    `thread_spans` — which don't carry that column and are reachable only
    through `view_instance(...)` — call-level filtering is their *only*
    enforcement.

    A restricted caller's call to any of the five fails with a
    not-found-shaped error unless the id argument names a process or stream
    in one of their own audiences, and `list_partitions()` silently omits
    every row (including `'global'` rows) that isn't theirs to see.
    `'global'` instances are exempt from `view_instance`'s guard entirely
    and stay readable for any scoped caller, since no `'global'` instance
    has anything to materialize and row-level filtering already covers its
    rows one at a time — a different rule from `list_partitions()`'s
    `'global'`-row visibility below, which gates *partition metadata about*
    a `'global'` file rather than the rows of the file itself. A view set on
    `MICROMEGAS_PUBLIC_VIEW_SETS` is also exempt from `view_instance`'s
    guard: any authenticated caller can trigger JIT materialization of any
    of its instances, since every row of that view set is already readable.

    **`audience` is a physical column on `processes`, `streams`, and
    `blocks`**, each carrying its own row's own stamp — the credential that
    wrote *that* row, never derived from the `process_id`/`stream_id` it
    points at. `log_entries`, `measures`, and `log_stats` carry the owning
    block's stamp straight through and are non-nullable at that point:
    `blocks_view`'s materialization resolves a legacy NULL column to
    `MICROMEGAS_DEFAULT_AUDIENCE` via `COALESCE` before it reaches those
    views. A credential with no bound audience is stamped with the resolved
    deployment default explicitly at write time (see [Ingestion → What gets
    stamped](ingestion.md#what-gets-stamped)); only a row registered before
    schema v8 keeps a NULL column, and every reader resolves that to
    `MICROMEGAS_DEFAULT_AUDIENCE`.

    **API keys and no-`email`-claim OIDC tokens are covered by `public`, via
    a grant like any other, with no second knob.** Under the grant-map model
    (see [Audiences and Grants](#audiences-and-grants) below), `public` has
    no built-in read grant — a fresh deployment ships with the seeded
    `('public', 'read', '*')` DB row, which makes every authenticated
    caller's readable set include `public` regardless of identity. Delete
    that row and `public` stops being universally readable for every caller
    kind alike. `MICROMEGAS_DEFAULT_AUDIENCE`'s default of `public` (see
    [Ingestion](ingestion.md#environment-variables)) restores visibility for
    every caller kind that never binds an audience of its own.

    **The two enforcement layers read different copies of the `audience`
    column, with different freshness.** Row-level filtering reads a
    daemon-materialized parquet snapshot (a process the maintenance role
    hasn't caught up on is invisible to everyone, including its owner);
    call-level filtering reads Postgres directly, so it is fresher for a
    just-ingested process, but denies (rather than falling back to the stale
    snapshot) once retention has deleted a process's Postgres row even if a
    merged/compacted lakehouse partition of its data still exists. Both
    copies resolve a real row's own stamp identically, so they can never
    disagree about the *value* for a row either has stamped — the remaining
    skew is purely this materialization-lag/retention-lag timing.

    **Eight admin-gated lakehouse UDTFs/UDFs** (`retire_partitions`,
    `materialize_partitions`, `regenerate_partitions`,
    `retire_partition_by_file`, `retire_partition_by_metadata`, and the
    [query deny list](functions-reference.md#query-deny-list)'s
    `list_query_denials`, `deny_queries`, `remove_query_denial`) are gated on
    whether this *deployment* can ever produce an admin principal at all —
    not on a knob an operator sets. An OIDC provider can grant admin whenever
    it has at least one configured admin user; an API-key provider never can.
    At startup, the server asks every configured auth provider "can you ever
    produce an admin?" and, if none can, registers these eight functions for
    *any* authenticated caller instead of admin-only — otherwise an
    API-key-only deployment would have no path to them at all.
    **Deployment-wide, not per-audience**: none of the eight functions
    filters by audience, so on a deployment with no admin principal, every
    authenticated caller gets destructive access to every audience's
    partitions, not just their own, and can also deny every query via
    `deny_queries` — safe only when no admin principal exists in the
    deployment.

### Audience stamping and the default {#audience-stamping-and-the-default}

The read-side filter above is only trustworthy because of the write side:
each of `processes`, `streams`, and `blocks` carries its own `audience`
**column**, server-written from the authenticated ingestion credential,
never trusted from the client payload. Ingestion strips any client-supplied
`micromegas.*` property and stamps every row it writes at insert time — a
block's or a stream's own stamp is the credential that wrote *that* row,
never derived from the `process_id`/`stream_id` it points at. A credential
with a bound audience (a DB-backed `ingestion_api_keys` row) is stamped with
that; one with none — an env-keyring key, an OIDC token, or no auth provider
at all — resolves to the deployment default and is stamped with it
explicitly.

**A pre-existing row with no stamp is read as the deployment default.**
Every process, stream, and block registered through the HTTP ingestion path
carries a real, non-NULL `audience` column; this applies only to a row
registered before its ingestion binary reached schema v8 (nullable, no
backfill) — admin `bulk_ingest`/replication now hard-fails on a missing
`audience` column rather than ever writing one with none.
`MICROMEGAS_DEFAULT_AUDIENCE` (default `public`) is applied at each place the
audience is read out of Postgres — the `blocks` view's materialization, the
per-process JIT path, and the call-level lookup — so such a row still has a
real, non-null audience everywhere it is enforced. Set it identically on
**every role that builds a lakehouse — including ingestion**: FlightSQL,
maintenance, the monolith, and ingestion. The maintenance role bakes the
value into partitions and ingestion stamps new rows with it, so a deployment
that sets the knob on only some of these roles gets new rows physically
stamped under one label while legacy rows read under another.

!!! warning "Changing the default is not a routine operation"
    A partition keeps the default that was configured when it was
    materialized. Changing `MICROMEGAS_DEFAULT_AUDIENCE` does **not**
    retroactively relabel already-written partitions — regenerate the six
    views (see the [Maintenance](maintenance.md) role's
    `regenerate_partitions`) over any range that should reflect the new
    value. This regeneration only relabels rows that carry **no** stamp (a
    legacy row registered before schema v8) — it never relabels an actually
    stamped row. Two consequences follow: partitions materialized on either
    side of a change disagree about such a row, and `FROM log_entries` can
    disagree with `view_instance('log_entries', <pid>)` for it, since the two
    are materialized at different times. Call-level filtering is not
    affected — it reads Postgres live, so it always uses the current
    default.

    **Flipping this knob also has a write-path effect.** The deployment
    default keeps the un-salted OTLP id namespace, so which label is live as
    the default determines which namespace is un-salted. A flip can leave
    unaudienced traffic presenting an un-salted `process_id` against rows
    stamped under the *old* default — registration then rejects that
    re-registration with a 403 (see the residual-gap warning below), with no
    retro-stamp to reconcile it, until those rows age out or are deleted.

Two consequences worth knowing before you flip this on:

- **OTLP `process_id` re-derivation.** OTLP-derived identity (`process_id`,
  and therefore `block_id`) is audience-scoped, so two audiences posting
  identical resource attributes never collapse onto one process. Each
  audience gets its own id namespace, and the deployment default keeps the
  pre-existing, un-salted namespace — so **no re-derivation** happens for
  traffic that carries no bound audience or resolves to the deployment
  default. What *does* re-derive, once: a DB-backed key **explicitly bound
  to a label equal to `MICROMEGAS_DEFAULT_AUDIENCE`** moves out of its own
  salted namespace into the un-salted one. The same logical process appears
  as a new row in that case; its earlier data keeps the old id. Rotating an
  ingestion key to a genuinely *different* audience likewise splits a
  long-lived producer's history across two process ids — expected, since the
  data now genuinely belongs to two audiences.
- **Client self-stamping has no effect.** A native client setting its own
  `micromegas.audience` property is stripped at ingestion (there is no
  property to re-assert — the stamp is a physical column) and replaced by
  the credential's authenticated audience, or the deployment default when
  the credential carries none. To get its own label, a producer needs a DB
  ingestion key bound to that audience.

**Process registration is confidentiality-sensitive, and this is closed.**
`processes` is a single table shared by the native and OTLP paths, and the
OTLP `process_id` derivation formula is public (see [OTLP
Ingestion](../otlp/index.md)). Any ingestion credential could otherwise
pre-register (via the native `insert_process` path) the exact `process_id` a
victim audience's OTLP producer would later derive; the genuine producer's
stream/blocks would then silently land on a row stamped with the squatter's
audience. `insert_process` and `register_otel_process` both reject a
same-`process_id`, different-audience re-registration with a 403 — since a
stamped process's audience is immutable (there is no `UPDATE processes` path
anywhere), the victim's producer can never successfully register that
`process_id` again until an operator manually deletes the squatted row
(`DELETE FROM processes WHERE process_id = ...`). The maintenance daemon's
`delete_empty_processes` sweep only reclaims it once the squatted row has no
streams and the retention window has elapsed — a squatter that also writes a
stream keeps the row alive indefinitely. The guard resolves an existing row
with a NULL `audience` column to the deployment default the same way every
reader does, then compares — so a squatter claiming a different audience
against a legacy or freshly pre-registered unstamped row is rejected the
same way as against a stamped one. `check_stream_audience_conflict` closes
the equivalent gap for `streams`: a stream re-pointed to a different
credential's audience is rejected at the next
`insert_stream`/`register_otel_stream` call for that `stream_id`.

**No retro-stamp, still.** Either guard compares against the existing row's
*resolved* audience but never writes anything back: a matching
re-registration of a row with a NULL `audience` column remains `Ok` and
leaves it unstamped. Since every process/stream registered through this HTTP
path is now stamped explicitly (see [Ingestion → What gets
stamped](ingestion.md#what-gets-stamped)), an unstamped row only arises from
data written before schema v8.

!!! warning "Residual gap: cross-audience write injection, narrower than before (tracked, not yet closed)"
    Every `blocks`/`streams`/`processes` row, and every view derived
    straight from them (`log_entries`, `measures`, `log_stats`,
    `processes_view`, `streams_view`), carries its own `audience` stamp — a
    block whose own stamp disagrees with the `streams`/`processes` row it
    points at is excluded from materialization entirely, so an attacker's
    block never surfaces under the victim's label, or under its own label
    pointing at the victim's `process_id`, in any of those views. What's
    still open:

    - **Five process/stream-anchored view sets** — `net_spans`, `otel_spans`,
      `images`, `async_events`, `thread_spans` — and **the per-process JIT
      `view_instance` path** still resolve their audience *label* through
      the owning process's/stream's row rather than a genuine per-row column
      of their own, via call-level filtering (the sole enforcement for a
      guarded `view_instance(...)` scan of these five). The cross-audience
      *injection* scenario is closed for both, as a side effect of where the
      materialization-time exclusion above lives — except against a victim
      whose `processes`/`streams` row is itself a legacy, pre-v8
      NULL-audience row (next bullet).
    - **The NULL-anchor window.** A `processes`/`streams` row registered
      before schema v8 keeps a NULL `audience` column for its entire
      remaining life (rows are immutable, and there is no backfill). The
      materialization-time exclusion's NULL-tolerant form lets a mismatched
      block through unchecked against such a row, deliberately — a strict
      comparison would instead permanently drop that row's legitimate
      post-upgrade telemetry. This is an accepted, bounded limitation over
      already-public legacy data (everything in the lake before this stage
      is public); it is bounded by bringing **every** ingestion replica to
      schema v8 before relying on audience separation during a rolling
      upgrade, and shrinks as legacy rows age out under retention.

    There is no in-product enforcement knob left for either surface; the
    mitigation is operational — provision only audience-bound DB-backed
    ingestion credentials, and don't run ingestion with an env-keyring key,
    OIDC, or `--disable-auth` alongside them.

## Audiences and Grants

**A user sees their own grants, and can share/mint self-service, from the
Audience Access page** (`/audiences` in the web app, open to every
authenticated user — see [`web-app.md`](web-app.md#audience-access)) or from
SQL via `list_audience_grants()`
(`micromegas-query --all "SELECT * FROM list_audience_grants()"`). The rest
of this section covers the underlying model.

An audience is an **opaque label on data** — `public`, `team-alpha`,
`payments-svc` — not an encoding of any principal's identity. Who can read or
mint into an audience is separate, editable configuration: a grant map,
resolved once at startup from `{prefix}_AUDIENCE_GRANTS` (falling back to
the unprefixed `MICROMEGAS_AUDIENCE_GRANTS`). This is the model
`AudienceReadPolicy`/`AudienceMintPolicy` (`micromegas_auth::policy`) resolve
against; the [Audience Filtering Activation](#audience-filtering-activation)
section above consumes the *read* half of it.

```json
{
  "public":       ["*"],
  "team-alpha":   ["group:eng", "user:alice@example.com"],
  "alice-laptop": {
    "read": ["user:alice@example.com", "group:leads"],
    "mint": ["user:alice@example.com"]
  }
}
```

Keys are audience names (`[A-Za-z0-9_-]{1,255}`, case-sensitive, no
normalization). Each value is either a bare array — read-only shorthand, the
common case — or an object with separate `"read"`/`"mint"` lists, needed only
when the audience should also grant *mint* authority. An omitted `"mint"`
list is always empty, never derived from `"read"`. Selectors:

| Selector | Matches |
|---|---|
| `*` | any authenticated principal |
| `user:<email>` | the caller's `email` claim |
| `group:<g>` | any value in the caller's `groups` claim |

**There is no self-audience rule.** A caller is never granted an audience
merely for being named like one — an API key named `team-alpha` does not
thereby read the `team-alpha` audience. A personal audience is an ordinary
audience with an ordinary grant entry (e.g. `"alice-laptop":
["user:alice@example.com"]`); self-service mint (below) removes the need to
provision one per user by hand.

**`public` has no built-in read grant of its own — it is a seeded row, not a
special case.** A fresh deployment's DB-backed grant store (below) ships
with `('public', 'read', '*')` already inserted, so `public` reads exactly
as if you had written `{"public": ["*"]}` yourself. Writing that entry in
the env-map grants above changes nothing on top of the seeded row — remove
it (`micromegas-grants delete public read '*'`, or the Audience Access page)
and `public` stops being universally readable, the same as removing any
other grant. This covers data that arrives without a bound audience only
while `MICROMEGAS_DEFAULT_AUDIENCE` is left at `public`: such traffic is
stamped with the knob's value at write time (see [Audience stamping and the
default](#audience-stamping-and-the-default)), with only a pre-existing
unstamped row still resolved to it at query time.

**Re-sharing already-ingested data is a grants edit, never a restamp.**
Since the audience *value* stamped on data never changes, widening who can
see `team-alpha` is a one-line config change — add a selector to its
`"read"` list — that takes effect for every already-ingested process
immediately (bounded by the mint-time key-store cache TTL for callers, not
by anything about the data itself).

**A malformed grant map fails startup, not silently**: an unknown-shaped
key, an unrecognized selector prefix, or a duplicate JSON key for the same
audience are all a startup `Err`.

**Worked profiles**, open and privacy:

```bash
# Open deployment: everyone reads everything, no grant map needed at all.
# MICROMEGAS_DEFAULT_AUDIENCE can be left unset -- it defaults to public.

# Privacy deployment: a team's data stays inside the team. One knob covers
# both sides: keys minted without an explicit audience, and processes whose
# credential carried no bound audience, are both stamped with this label
# explicitly at write time. Point it at a label nobody is granted, so
# anything that omits an audience is invisible rather than published, and
# name the audience explicitly on every key you mint. Set it on every role
# that builds a lakehouse -- FlightSQL, maintenance, monolith, and
# ingestion -- since ingestion reads it too; a deployment that sets it on
# only the first three gets new processes physically stamped `public` while
# legacy rows still read as `unassigned`. Regenerating the six views only
# relabels rows that carry no stamp (legacy rows, and rows from the admin
# replication path) -- a stamped row's label is never regeneration's to
# change. Flipping the knob after data exists also has a write-path effect:
# see the "Changing the default is not a routine operation" warning above.
export MICROMEGAS_AUDIENCE_GRANTS='{"team-alpha": ["group:eng"]}'
export MICROMEGAS_DEFAULT_AUDIENCE=unassigned
```

Worked **mint** profile, granting a non-admin caller mint authority for
their own personal audience — see [self-service
mint](#self-service-ingestion-key-mint) below for the full picture (the
knob that gates this, the per-caller bounds, and
`micromegas-setup-telemetry`):

```bash
# One admin-created grant per personal audience, mint only (read is granted
# separately, or via a claim -- see below):
micromegas-grants --url https://analytics.example.com create alice-laptop mint user:alice@example.com
micromegas-grants --url https://analytics.example.com create alice-laptop read user:alice@example.com
```

A non-admin caller with this grant can now mint their own `alice-laptop` key
directly (`POST /api/ingestion-api-keys`), once
`MICROMEGAS_SELF_SERVICE_MINT` is on (below) — no further admin step needed
for that audience. **Self-service mint grants must live in the DB
`audience_grants` table, never in `{prefix}_AUDIENCE_GRANTS`** — unlike the
read axis (which still unions both sources), the mint axis is DB-only: a
mint audience declared only in the env map is invisible to the lazy claim's
existence check (below) and could be claimed out from under it by another
caller. Keep every mint-relevant audience's grants in the DB once
`MICROMEGAS_SELF_SERVICE_MINT` is on.

This isn't only a mint-axis concern: the lazy claim's existence check reads
only the DB (`audience_grants` and `ingestion_api_keys`), never
`{prefix}_AUDIENCE_GRANTS` at all, on *either* axis. An audience named only
in that env map for **read** — like `team-alpha` in the privacy-deployment
example above — is just as invisible to the check as a mint-only one, and so
just as squattable. Before turning on `MICROMEGAS_SELF_SERVICE_MINT`,
pre-create a placeholder DB row (any axis, any selector) for **every**
audience name that appears anywhere in `{prefix}_AUDIENCE_GRANTS`, not just
the ones with a `mint` entry — see the placeholder-row example below.

### Self-service ingestion key mint {#self-service-ingestion-key-mint}

Given a `mint` grant already exists (the worked profile above), a non-admin
caller can mint their own ingestion key directly — `POST
{base_path}/api/ingestion-api-keys` is not purely admin-gated.
`MintPolicy::resolve_audience` (a per-request point query against
`audience_grants`, never a cached snapshot) is the authorization instead of
an admin gate; an admin's own mint is unaffected either way. This is gated
behind one off-by-default deployment knob:

| Variable | Default | Description |
|---|---|---|
| `MICROMEGAS_SELF_SERVICE_MINT` | `false` | Off by default. Also gates `GET {base_path}/api/audience-grants/my-audiences` (below) for non-admin callers, plus non-admin audience-grant create/delete and `GET .../audience-grants/visible`'s non-admin narrowing (see below). |
| `MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER` | `25` | Caps how many distinct audiences one non-admin caller may lazily claim (below). A backstop against a runaway/abusive caller, not a routine-use quota. Best-effort under concurrency. |
| `MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER` | `100` | Caps how many *live* keys one non-admin caller may hold at once. `list_keys`/`revoke_key` stay `AdminUser`-gated, so a non-admin has no self-service way to free a slot once this is reached — reducing the count always requires an admin. |
| `MICROMEGAS_SELF_SERVICE_MAX_GRANTS_PER_CALLER` | `50` | Caps how many rows one non-admin caller may have created in `audience_grants` (counted across every audience/axis/selector, excluding the caller's own `user:<email>` rows, which are claim/self-access rows, not shares). Best-effort under concurrency. |

**`public` ships mintable, by a seeded grant.** Schema v9 seeds `('public',
'mint', '*')` into `audience_grants` (alongside the read-side default
described in [Audiences and Grants](#audiences-and-grants) above) — once
`MICROMEGAS_SELF_SERVICE_MINT` is on, any authenticated non-admin can mint
an ingestion key bound to `public` with no further admin step, since the row
already exists. The knob still gates this — the row confers nothing while
it is off. Remove the row to require an explicit per-caller grant for
`public` instead:

```bash
micromegas-grants --url https://analytics.example.com delete public mint '*'
```

or the Audience Access page's `public` → Mint → Remove. A deployment
running a custom `MICROMEGAS_DEFAULT_AUDIENCE` gets the same literal
`public` row, since the seed means one thing everywhere rather than
depending on the deployment's default.

**Audiences are created lazily, not pre-provisioned.** A non-admin caller
who names a brand-new, never-before-granted audience *and supplies the name
explicitly* claims it atomically, as part of the same mint request, once
`MICROMEGAS_SELF_SERVICE_MINT` is on: the claim writes `user:<email>` grant
rows on **both** the `mint` and `read` axes (so the caller who just claimed
the audience can read back what their own new key uploads), inside the same
transaction that mints the key. Naming an audience that already has *any*
grant row — admin-created, self-claimed earlier, or someone else's
in-flight claim — still requires a matching grant; only a genuinely fresh,
unowned name is claimable this way. `public` and the deployment's own
`MICROMEGAS_DEFAULT_AUDIENCE` can never be *claimed* — a distinct property
from being *mintable*: `public` ships mintable via its own seeded grant
precisely because it already has grant rows and so never reaches the
lazy-claim path.

**Before turning on `MICROMEGAS_SELF_SERVICE_MINT`, pre-create a placeholder
grant row — any selector, on either axis — for every audience name that
exists only outside the DB:** a custom `MICROMEGAS_DEFAULT_AUDIENCE` (the
seeded `public` row already covers leaving it at its default), and *every*
key of `{prefix}_AUDIENCE_GRANTS` (mint-relevant or read-only alike). Via
the admin grants API, prefer an identity selector over a blanket `'*'` — a
custom default should not become a second `public` merely to satisfy this
check:

```bash
micromegas-grants --url https://analytics.example.com create legacy-default read 'group:eng'
# ...and one such row per audience named in {prefix}_AUDIENCE_GRANTS, e.g.:
micromegas-grants --url https://analytics.example.com create team-alpha read 'group:eng'
```

Without those placeholder rows, the lazy claim's existence check (which
reads only `audience_grants` and `ingestion_api_keys`, never a role-prefixed
env knob) would see any of these names as unowned and let a non-admin claim
exclusive mint+read rights over a name the operator already believes is
spoken for.

The setup script, `micromegas-setup-telemetry`, wraps all of this for an end
user — OIDC login, mint, and printing the `OTEL_EXPORTER_OTLP_*` env vars
needed to point their own telemetry at the deployment:

```bash
# Existing grant, or resolved automatically via GET .../my-audiences if omitted:
micromegas-setup-telemetry --url https://analytics.example.com --name my-laptop \
    --audience alice-laptop

# The seeded public mint grant (above) makes this work for any authenticated
# non-admin caller, with no admin action needed:
micromegas-setup-telemetry --url https://analytics.example.com --name my-laptop \
    --audience public

# A fresh claim: --claim claims the name verbatim, with no prefix applied --
# namespacing is a convention you carry in the name you pass, not something
# the tool does for you:
micromegas-setup-telemetry --url https://analytics.example.com --name ci-runner \
    --claim "$USER-ci-runner"

eval "$(micromegas-setup-telemetry --url https://analytics.example.com --name my-laptop)"
```

See [`python-api.md`](../query-guide/python-api.md#micromegas-setup-telemetry)
for the full CLI reference, and [`api-keys.md`](api-keys.md) for the mint
route's error shapes (`FORBIDDEN`, `UNAVAILABLE`, `UNAUTHENTICATED`,
`CLAIM_CONTENDED`).

**The same knob also governs sharing and removal on the Audience Access
page** (`/audiences`, open to every authenticated user — see
[`web-app.md`](web-app.md#audience-access)): a non-admin may create/delete a
grant row (per the write policy below) only when
`MICROMEGAS_SELF_SERVICE_MINT` is on, exactly as for minting.

**An admin minting into a brand-new audience is also claimed server-side**,
the same way a non-admin's lazy claim is: `mint_key` runs the same ownership
check as a pre-check for an admin caller, and if the audience looks
unclaimed, writes the admin's own `user:<email>` `mint`+`read` rows in the
same transaction as the key insert. The mint response's `claimed` field says
whether it happened. An admin with no email is unaffected.

### DB-backed audience grants

`{prefix}_AUDIENCE_GRANTS` is resolved once at startup, so creating one
per-user grant means editing an env var and restarting every service that
reads it. The `audience_grants` Postgres table is the same grant model,
minted, listed, and deleted over HTTP without a redeploy:

```sql
CREATE TABLE audience_grants (
  audience   VARCHAR(255) NOT NULL,
  axis       VARCHAR(4) NOT NULL CHECK (axis IN ('read', 'mint')),
  selector   VARCHAR(255) NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  created_by VARCHAR(255) NOT NULL,
  PRIMARY KEY (audience, axis, selector),
  CONSTRAINT audience_grants_audience_name CHECK (audience ~ '^[A-Za-z0-9_-]+$'),
  CONSTRAINT audience_grants_selector_shape
      CHECK (selector = '*' OR selector ~ '^(user|group):.+$')
);
```

One table with an `axis` column, not two — a 1:1 image of the env map's
single map covering both the read and mint axes.

**Additive, never a replacement.** Each flight-sql process — standalone
`flight-sql-srv`, or the monolith's flight-sql role — holds one whole-table
snapshot in memory, unioned with the env map before matching a caller's
selectors. `analytics-web-srv` is the write surface only (the HTTP admin
routes below) — it never constructs a `DbAudienceGrantsSource` and caches
nothing itself. For the **read** axis, a selector present in the env map,
the store, or both grants exactly the same access — no precedence to reason
about, and no forced migration off `{prefix}_AUDIENCE_GRANTS`. This is not
true for the **mint** axis: mint grants are DB-only, so an env-map-only
`"mint"` selector is inert — it is never consulted by `mint_key`'s
per-request authorization check.

**HTTP routes:**

| Route | Body / result |
|---|---|
| `POST {base_path}/api/audience-grants` | `{"audience","axis","selector"}` → 201 (created) or 200 (already existed) `{"audience","axis","selector","created_at","created_by"}` |
| `DELETE {base_path}/api/audience-grants?audience=&axis=&selector=` | 204, or 404/403 |
| `GET {base_path}/api/audience-grants/visible` | 200 `[{"audience","axis","selector","created_at","created_by"}]` — the caller-scoped read backing the Audience Access page's own list (below) |
| `GET {base_path}/api/audience-grants/my-audiences` | Any authenticated caller. 200 `{"is_admin","audiences","mint_prefix","email","held_pairs"}`: the audiences whose `mint` selector matches *this caller's own* identity today, plus the caller's own `is_admin` flag, the caller-derived namespace prefix used only to *suggest* a fresh audience name (the web app's Mint dialog composes it live before commit; the CLI renders a `--claim` suggestion from it, but never mints under it itself), the caller's own email, and `held_pairs` -- the `"{audience}:{axis}"` pairs the caller holds via an identity selector (drives the page's Share control; always empty for an admin). |

There is no paginated `GET` over the whole collection. Listing arbitrary rows
from SQL goes through the caller-scoped `list_audience_grants()` table
function (below); the page's own list reads `GET .../visible`.

**`POST`/`DELETE` gate**, `GrantGate`: an admin acts unconditionally. A
non-admin is admitted only when `MICROMEGAS_SELF_SERVICE_MINT` is on, and
then further constrained per call:

- **Create**: `selector` must be `user:…`/`group:…` (never `*` — a caller
  who can read an audience must not be able to open it to every
  authenticated principal), and the caller must **hold** `(audience, axis)`
  via an identity selector (`user:`/`group:`, not a `*` row). Delegation is
  per axis: a `read` grant lets you share `read`, a `mint` grant lets you
  share `mint`, and neither confers the other.
- **Delete**: the row must be the caller's own direct `user:<email>` row
  ("remove my access" — never offered for `group:`/`*` rows), or a row the
  caller themselves created — except their own `mint`/`user:<email>` row,
  which "remove my access" does not cover: that row is the self-service
  claim marker `max_claims_per_caller` counts from, so a non-admin can't
  delete it themselves (an admin still can). A row that doesn't exist at all
  is 404; one that exists but matches no condition is 403.

`DELETE` takes the natural key as query parameters rather than path
segments: a `group:<id>` selector can contain `/` or other
URL-significant characters a raw path segment can't safely carry.

**`GET .../visible`'s own visibility rule, by caller**: admin sees every
row; a non-admin with the knob on sees every grant on each pair they hold a
matching grant on (the same held-pair rule `list_audience_grants()` uses,
below); a non-admin with the knob off sees only their own rows.

The `micromegas-grants` CLI wraps the two write routes; listing goes through
`micromegas-query` instead:

```bash
micromegas-grants --url https://analytics.example.com create team-alpha read group:eng
micromegas-query --all "SELECT * FROM list_audience_grants()" --profile analytics
micromegas-grants --url https://analytics.example.com delete team-alpha read group:eng
```

### `list_audience_grants()`

A caller-scoped SQL table function over the `audience_grants` table,
registered for **every** authenticated caller (never admin-gated) — like
`list_query_denials()`, it is a SQL auditing surface, not a REST route. No
arguments; filter with `WHERE`. Columns: `audience`, `axis`, `selector`,
`created_at`, `created_by`.

**Visibility**: an admin sees every row. A non-admin sees every grant on
each `(audience, axis)` pair they hold a matching grant on — deliberately
wider than "rows whose selector matches me": if you may read `team-alpha`,
you may see who else may. Only a non-admin caller with an empty selector set
sees zero rows; a maintenance caller and a `--disable-auth` request are both
treated as admin.

**Unlike `GET .../visible`, this function always applies the held-pair rule
for a non-admin, knob or no knob** — it runs in
`flight-sql-srv`/`micromegas-analytics`, which has no visibility into
`analytics-web-srv`'s `MICROMEGAS_SELF_SERVICE_MINT` config. See [Admin
Functions Reference](functions-reference.md#list_audience_grants) for the
full schema and query shape.

```bash
micromegas-query --all "SELECT * FROM list_audience_grants() WHERE audience = 'team-alpha'"
```

**Cache-TTL knob:**

| Variable | Default | Description |
|---|---|---|
| `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS` | `60` | How long a process serves its in-memory snapshot before re-querying `audience_grants`. Accepts a role prefix on the monolith (`MICROMEGAS_ANALYTICS_AUDIENCE_GRANT_CACHE_TTL_SECONDS`), falling back to the unprefixed name. |

Revocation takes effect within the cache TTL (default 60s), not instantly: a
`DELETE` above removes the row immediately, but every flight-sql process
keeps serving its cached snapshot — and so keeps granting the removed access
— until its next refresh.

**Outage behavior is deliberately different from the DB-backed key store's.**
Once a process has loaded the table successfully at least once, a later
refresh failure keeps serving that last good snapshot, unbounded, for as
long as the outage lasts. A fresh process whose very first query hits a down
DB has no "last good" to serve, so that case fails closed — `resolve()`
denies, at a rate capped by the same cache-TTL knob. A sustained outage
surfaces on `audience_grant_refresh_error_count`, not on the request path.

**A malformed row still can't reach a policy decision.** The table's own
`CHECK` constraints are re-validated independently in Rust on every load, so
a row that somehow bypassed them fails the *whole* snapshot load loudly.

**Deploy-order requirement.** `audience_grants` is created by a migration
that only `telemetry-ingestion-srv` and the monolith run. A standalone
`flight-sql-srv` never creates the table itself. Roll that migration (by
upgrading ingestion or the monolith) before or in the same deploy as
upgrading `flight-sql-srv` to a build that wires `DbAudienceGrantsSource`.
Upgrading `flight-sql-srv` first against a still-old database is not a
startup failure: the process comes up fine and then fails every `resolve()`
call with "relation audience_grants does not exist" (throttled to one DB
attempt per cache-TTL window), failing every authenticated query with
`unavailable` — `public`-only queries included — until the migration lands.

**The same skew window applies to the seeded `public` rows.** `public` has
no built-in read grant — it reads only through the seeded `('public',
'read', '*')` row. A standalone `flight-sql-srv` (or `analytics-web-srv`)
that upgrades to newer code *before* `telemetry-ingestion-srv` or the
monolith has applied that migration sees a database with no seeded row:
every query returns nothing (fail-closed, not an error) until the migration
lands. Roll it (by upgrading ingestion or the monolith) before or in the
same deploy as upgrading `flight-sql-srv`/`analytics-web-srv`.

## Client Configuration

### Python Client with OIDC

The Python client supports automatic browser-based login with token persistence and refresh.

#### Interactive Use (Jupyter, Scripts)

```python
from micromegas.auth import OidcAuthProvider
from micromegas.flightsql.client import FlightSQLClient

# First use: Opens browser for authentication
auth = OidcAuthProvider.login(
    issuer="https://accounts.google.com",
    client_id="your-app-id.apps.googleusercontent.com",
    client_secret="your-client-secret",  # Optional for some providers
    token_file="~/.micromegas/tokens.json"  # Persists tokens
)

# Create authenticated client
client = FlightSQLClient(
    "grpc+tls://analytics.example.com:50051",
    auth_provider=auth
)

# Run queries - tokens auto-refresh before expiration
df = client.query("SELECT * FROM processes LIMIT 10")
```

**Parameters:**

| Parameter | Description | Default |
|-----------|-------------|---------|
| `issuer` | OIDC issuer URL | Required |
| `client_id` | OAuth client ID | Required |
| `client_secret` | OAuth client secret | Optional (for public clients) |
| `token_file` | Path to save tokens | `~/.micromegas/tokens.json` |
| `audience` | API audience/identifier | Optional (required by Auth0) |
| `scope` | OAuth scopes to request | `openid email profile offline_access` |

#### Subsequent Use (Token Reuse)

```python
from micromegas.auth import OidcAuthProvider
from micromegas.flightsql.client import FlightSQLClient

# Load existing tokens - no browser interaction needed
auth = OidcAuthProvider.from_file(
    "~/.micromegas/tokens.json",
    client_secret="your-client-secret"  # Optional
)

client = FlightSQLClient(
    "grpc+tls://analytics.example.com:50051",
    auth_provider=auth
)

# Tokens automatically refresh when needed
import datetime
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(hours=1)
df = client.query("SELECT * FROM log_entries LIMIT 1000", begin, now)
```

#### Token Management

```python
# Clear saved tokens (logout)
import os
from pathlib import Path

token_file = Path.home() / ".micromegas" / "tokens.json"
if token_file.exists():
    token_file.unlink()
    print("Logged out - tokens cleared")
```

### CLI Tools with OIDC

CLI tools automatically support OIDC when environment variables are set:

```bash
# Configure OIDC
export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"
export MICROMEGAS_OIDC_CLIENT_ID="your-app-id.apps.googleusercontent.com"
export MICROMEGAS_OIDC_CLIENT_SECRET="your-client-secret"  # Optional
export MICROMEGAS_ANALYTICS_URI="grpc+tls://analytics.example.com:50051"

# First use: Opens browser for authentication
micromegas-query "SELECT process_id, exe, start_time FROM processes" --begin 1h

# Subsequent uses: No browser interaction, uses cached tokens
micromegas-query "SELECT time, level, msg FROM log_entries WHERE process_id = '<process_id>'" --begin 1h

# Logout (clear saved tokens)
micromegas-logout
```

**Environment Variables:**

| Variable | Description | Required |
|----------|-------------|----------|
| `MICROMEGAS_OIDC_ISSUER` | OIDC issuer URL | Yes |
| `MICROMEGAS_OIDC_CLIENT_ID` | OAuth client ID | Yes |
| `MICROMEGAS_OIDC_CLIENT_SECRET` | OAuth client secret | No* |
| `MICROMEGAS_OIDC_AUDIENCE` | API audience/identifier | No (for Auth0, Azure API) |
| `MICROMEGAS_OIDC_SCOPE` | OAuth scopes to request | No (default: openid email profile offline_access) |
| `MICROMEGAS_PROFILE` | Named connection profile to select from `~/.micromegas/config.json`'s `profiles` map | No (see [Named profiles](../query-guide/python-api.md)) |
| `MICROMEGAS_ANALYTICS_URI` | Analytics server URI | No (default: grpc://localhost:50051) |

*Required for some providers (e.g., Google) even with PKCE

### Python Client with API Keys

```python
from micromegas.flightsql.client import FlightSQLClient

client = FlightSQLClient(
    "grpc://localhost:50051",
    headers={"authorization": "Bearer your-api-key"}
)

df = client.query("SELECT * FROM processes LIMIT 10")
```

!!! warning "Deprecated API"
    The `headers` parameter is deprecated. Use `auth_provider` with `OidcAuthProvider` instead.

## Ingestion Service Authentication

The telemetry ingestion service (`telemetry-ingestion-srv`) uses the same authentication infrastructure as the analytics service.

### Server Configuration

```bash
# Start ingestion server with authentication
export MICROMEGAS_API_KEYS='[{"name": "service1", "key": "secret-key-123"}]'
export MICROMEGAS_OIDC_CONFIG='{"issuers": [...]}'
telemetry-ingestion-srv

# Or disable auth for development
telemetry-ingestion-srv --disable-auth
```

The OTLP/HTTP routes (`/ingestion/otlp/v1/{logs,metrics,traces}`) share this authentication chain. OTel SDKs attach the bearer token via `OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer <key>"`. See [OTLP Ingestion](../otlp/index.md#authentication) for SDK-side configuration.

### Rust Client Authentication

Rust applications sending telemetry can use either API keys or OIDC client credentials.

#### Automatic Configuration (Recommended)

Applications using `#[micromegas_main]` automatically configure authentication from environment variables:

```rust
use micromegas::micromegas_main;
use micromegas::tracing::prelude::*;

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    info!("Application starting");
    // Telemetry automatically authenticated based on environment variables
    Ok(())
}
```

**Environment Variables:**

| Variable | Authentication Method | Required |
|----------|----------------------|----------|
| `MICROMEGAS_INGESTION_API_KEY` | API key (simple) | For API key auth |
| `MICROMEGAS_OIDC_TOKEN_ENDPOINT` | OIDC client credentials | For OIDC auth |
| `MICROMEGAS_OIDC_CLIENT_ID` | OIDC client credentials | For OIDC auth |
| `MICROMEGAS_OIDC_CLIENT_SECRET` | OIDC client credentials | For OIDC auth |
| `MICROMEGAS_TELEMETRY_URL` | Ingestion server URL | Yes (e.g., http://localhost:9000) |

**Example (API Key):**
```bash
export MICROMEGAS_INGESTION_API_KEY=secret-key-123
export MICROMEGAS_TELEMETRY_URL=http://localhost:9000
cargo run
```

**Example (OIDC Client Credentials):**
```bash
export MICROMEGAS_OIDC_TOKEN_ENDPOINT=https://accounts.google.com/o/oauth2/token
export MICROMEGAS_OIDC_CLIENT_ID=my-service@project.iam.gserviceaccount.com
export MICROMEGAS_OIDC_CLIENT_SECRET=secret-from-secret-manager
export MICROMEGAS_TELEMETRY_URL=http://localhost:9000
cargo run
```

#### Manual Configuration

For applications not using `#[micromegas_main]`, configure authentication manually:

##### API Key Authentication (Simple)

```rust
use micromegas_telemetry_sink::http_event_sink::{HttpEventSink, HttpSinkConfig};
use micromegas_telemetry_sink::api_key_decorator::ApiKeyRequestDecorator;
use std::sync::Arc;

// From environment variable
std::env::set_var("MICROMEGAS_INGESTION_API_KEY", "secret-key-123");
let decorator = ApiKeyRequestDecorator::from_env().unwrap();

// Configure HttpEventSink with authentication
let sink = HttpEventSink::new(
    "http://localhost:9000",
    HttpSinkConfig::default(),
    Box::new(move || Arc::new(decorator.clone())),
);
```

##### OIDC Client Credentials (Production)

```rust
use micromegas_telemetry_sink::http_event_sink::{HttpEventSink, HttpSinkConfig};
use micromegas_telemetry_sink::oidc_client_credentials_decorator::OidcClientCredentialsDecorator;
use std::sync::Arc;

// Configure OIDC client credentials
std::env::set_var("MICROMEGAS_OIDC_TOKEN_ENDPOINT",
    "https://accounts.google.com/o/oauth2/token");
std::env::set_var("MICROMEGAS_OIDC_CLIENT_ID",
    "my-service@project.iam.gserviceaccount.com");
std::env::set_var("MICROMEGAS_OIDC_CLIENT_SECRET",
    "secret-from-secret-manager");

let decorator = OidcClientCredentialsDecorator::from_env().unwrap();

let sink = HttpEventSink::new(
    "http://localhost:9000",
    HttpSinkConfig::default(),
    Box::new(move || Arc::new(decorator.clone())),
);
```

**Authentication Methods Comparison:**

| Method | Use Case | Token Lifetime | Complexity |
|--------|----------|----------------|------------|
| API Key | Development, testing | No expiration | Low |
| Client Credentials | Production services | ~1 hour (auto-refresh) | Medium |

### Health Endpoint

The `/health` endpoint remains public for monitoring and liveness checks, even when authentication is enabled.

```bash
# Health check always works without authentication
curl http://localhost:9000/health
```

## Setting Up OIDC Providers

### Google OAuth Setup

1. Go to [Google Cloud Console](https://console.cloud.google.com/)
2. Create a new project or select existing
3. Navigate to **APIs & Services → OAuth consent screen**
   - Select "External" user type
   - Fill in app name and contact emails
   - Add test users (yourself and team members)
4. Navigate to **APIs & Services → Credentials**
   - Click "+ CREATE CREDENTIALS" → "OAuth client ID"
   - Application type: **"Desktop app"** (for CLI/local use)
   - Click "Create"
5. Copy both credentials:
   - **Client ID** (ends with `.apps.googleusercontent.com`)
   - **Client Secret**
6. Add authorized redirect URIs:
   - `http://localhost:48080/callback`

**Server Configuration:**

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://accounts.google.com",
      "audience": "123-abc.apps.googleusercontent.com"
    }
  ]
}'
```

**Client Configuration:**

```bash
export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"
export MICROMEGAS_OIDC_CLIENT_ID="123-abc.apps.googleusercontent.com"
export MICROMEGAS_OIDC_CLIENT_SECRET="GOCSPX-..."
```

### Azure AD Setup

1. Go to [Azure Portal](https://portal.azure.com/)
2. Navigate to **Azure Active Directory → App registrations**
3. Click **"New registration"**
   - Name: "Micromegas Analytics"
   - Supported account types: Choose based on your needs
   - Redirect URI: "Public client/native" - `http://localhost:48080/callback`
4. Note the **Application (client) ID**
5. Navigate to **Authentication**
   - Under "Advanced settings", set "Allow public client flows" to **Yes**
   - This enables PKCE without requiring a client secret
6. Navigate to **API permissions** (optional)
   - Add permissions if needed for your organization

**Server Configuration:**

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://login.microsoftonline.com/{tenant-id}/v2.0",
      "audience": "{application-id}"
    }
  ]
}'
```

**Client Configuration:**

```bash
export MICROMEGAS_OIDC_ISSUER="https://login.microsoftonline.com/{tenant-id}/v2.0"
export MICROMEGAS_OIDC_CLIENT_ID="{application-id}"
# No MICROMEGAS_OIDC_CLIENT_SECRET needed - Azure AD supports public clients with PKCE
```

### Auth0 Setup

1. Go to [Auth0 Dashboard](https://manage.auth0.com/)
2. Create application:
   - Applications → Create Application
   - Name: "Micromegas Analytics"
   - Application type: **"Native"** (for CLI/desktop)
3. Configure application:
   - Allowed Callback URLs: `http://localhost:48080/callback`
   - Allowed Web Origins: `http://localhost:48080`
4. Note the **Domain** and **Client ID**
5. For Native apps, client secret is optional (true public client)

**Server Configuration:**

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://your-tenant.auth0.com/",
      "audience": "your-client-id"
    }
  ]
}'
```

**Client Configuration:**

```bash
export MICROMEGAS_OIDC_ISSUER="https://your-tenant.auth0.com/"
export MICROMEGAS_OIDC_CLIENT_ID="your-client-id"
# No client_secret needed for Native apps
```

## Security Considerations

### Token Storage

Tokens are stored at `~/.micromegas/tokens.json` with secure file permissions (0600 - owner read/write only). If `~/.micromegas/config.json` has a `profiles` map, the cache moves to a per-profile `~/.micromegas/tokens-<profile>.json` instead — see [Named profiles](../query-guide/python-api.md).

**Token File Contents:**

- Access token (JWT)
- Refresh token
- ID token
- Expiration time
- Issuer and client ID

!!! warning "Token File Security"
    Never commit token files to version control or share them. Tokens provide full access to your analytics data.

### Token Refresh

The Python client automatically refreshes tokens when they approach expiration (5-minute buffer). This ensures:
- No mid-query authentication failures
- Transparent token management
- Thread-safe concurrent query support

### Token Revocation

To revoke access:

1. **User accounts:** Disable the user in your identity provider (Google, Azure AD, etc.)
2. **Service accounts:** Disable or delete the service account in your identity provider
3. **Immediate revocation:** Restart the analytics server to clear the token validation cache

**Revocation Timing:**

- New tokens will be rejected immediately after disabling the account
- Existing cached tokens remain valid for up to 5 minutes (configurable via `token_cache_ttl_secs`)
- Total revocation time: Cache TTL (5 min) + Token lifetime (typically 60 min) = ~65 minutes worst case

For faster revocation, use shorter token cache TTL or restart the analytics server.

### Admin Privileges

Admin users (configured via `MICROMEGAS_ADMINS`) have elevated privileges for administrative operations. Only grant admin access to trusted users.

**Admin Capabilities:**

- Partition management functions
- Schema migration operations
- Administrative SQL functions

Admin status is reachable only through an OIDC identity matched against
`MICROMEGAS_ADMINS` (or the role-scoped `MICROMEGAS_ANALYTICS_ADMINS` in the
monolith) — never through `MICROMEGAS_API_KEYS`. API keys always resolve to
`is_admin: false`, so an API-key-only deployment has no admin principal and cannot
call the eight gated admin SQL functions (see
[Admin SQL Functions](functions-reference.md)).

### HTTPS/TLS

Always use TLS for production deployments:

```python
# Production: Use grpc+tls
client = FlightSQLClient(
    "grpc+tls://analytics.example.com:50051",
    auth_provider=auth
)

# Development only: Plain grpc
client = FlightSQLClient(
    "grpc://localhost:50051",
    auth_provider=auth
)
```

Configure your load balancer or reverse proxy to handle TLS termination.

### PKCE (Proof Key for Code Exchange)

The Python client uses PKCE for all OIDC flows, providing security for public clients (desktop apps, CLIs) that cannot securely store client secrets.

**How PKCE Works:**

1. Client generates random `code_verifier`
2. Client creates `code_challenge` (SHA256 hash of verifier)
3. Authorization request includes `code_challenge`
4. Token exchange includes original `code_verifier`
5. Identity provider validates the verifier matches the challenge

This prevents authorization code interception attacks even if the client secret is compromised or unavailable.

## Troubleshooting

### Authentication Failures

**Symptom:** "Invalid token" or "Authentication failed" errors

**Solutions:**

1. Check server logs: `tail -f /tmp/analytics.log | grep -i auth`
2. Verify OIDC configuration matches between server and client
3. Ensure Client ID and Issuer URL are correct
4. Check token expiration: `cat ~/.micromegas/tokens.json | jq .token.expires_at` (or `tokens-<profile>.json` if a `profiles` map is in use)
5. Clear tokens and re-authenticate: `micromegas-logout` (clears every cached token file; use `--profile <name>` to narrow to one)

### Token Refresh Failures

**Symptom:** Browser opens on every CLI invocation

**Solutions:**

1. Check if refresh token is present: `cat ~/.micromegas/tokens.json | jq .token.refresh_token` (or `tokens-<profile>.json` if a `profiles` map is in use)
2. Verify client secret matches (if required by provider)
3. Check token file permissions: `ls -la ~/.micromegas/tokens.json` (should be 600; or `tokens-<profile>.json` if a `profiles` map is in use)
4. Re-authenticate: `micromegas-logout` then retry (clears every cached token file; use `--profile <name>` to narrow to one)

### Server Configuration Issues

**Symptom:** Server fails to start or rejects all authentication

**Solutions:**

1. Validate OIDC config JSON syntax: `echo $MICROMEGAS_OIDC_CONFIG | jq .`
2. Check server can reach identity provider: `curl https://accounts.google.com/.well-known/openid-configuration`
3. Verify audience matches client ID exactly
4. Check server logs for OIDC discovery errors

### Multi-Provider Issues

**Symptom:** Only one identity provider works

**Solutions:**

1. Verify all issuers are in the configuration array
2. Check each issuer URL is correct and accessible
3. Ensure audience (client_id) matches for each provider
4. Review server logs for OIDC discovery failures per issuer

## Migrating Machine Credentials off the Env Keyring

OIDC is the destination for human/service-account identities, not machine
credentials — service keys migrate to DB-backed `ingestion_api_keys`/
`analytics_api_keys` instead. See [API Keys → Migrating from the env
keyring](api-keys.md#migrating-from-the-env-keyring) for the exact
procedure and commands; `object-cache-srv` is the one service that keeps
`MICROMEGAS_API_KEYS` permanently (see [Object Cache](object-cache.md)).

## Best Practices

1. **Use OIDC for all new deployments** - Better security and user management
2. **Enable admin privileges sparingly** - Only for users who need administrative access
3. **Use short token cache TTL** in high-security environments (60-300 seconds)
4. **Monitor authentication logs** - Track failed auth attempts and unusual patterns
5. **Rotate client secrets regularly** - Update in identity provider and redistribute
6. **Use separate OAuth clients** for different environments (dev, staging, prod)
7. **Document your identity provider setup** - Makes onboarding new team members easier

## Reference

- [OpenID Connect Core Specification](https://openid.net/specs/openid-connect-core-1_0.html)
- [OAuth 2.0 RFC 6749](https://datatracker.ietf.org/doc/html/rfc6749)
- [PKCE RFC 7636](https://datatracker.ietf.org/doc/html/rfc7636)
- [OAuth 2.0 Security Best Practices](https://datatracker.ietf.org/doc/html/draft-ietf-oauth-security-topics)
