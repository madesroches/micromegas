# Authentication

Micromegas supports unified authentication across all services using both API keys and OpenID Connect (OIDC).

## Overview

Both the analytics server (flight-sql-srv) and ingestion server (telemetry-ingestion-srv) support two authentication methods:

- **OIDC (OpenID Connect)** - For human users and service accounts via federated identity providers (Google, Azure AD, Okta, Auth0, etc.)
- **API Keys** - Legacy support for simple bearer token authentication

Both methods can be enabled simultaneously. When multiple providers are configured, they are tried in order until one succeeds (API key first for performance, then OIDC).

## Authentication Methods

### OIDC Authentication (Recommended)

OIDC provides secure federated authentication with automatic token refresh and support for multiple identity providers.

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

- **Env keyring (legacy/bootstrap)** — `MICROMEGAS_API_KEYS`, a JSON array parsed once
  at startup. Still the only option for `object-cache-srv` (which has no DB
  connection and stays env-only permanently); a transitional bootstrap path for
  ingestion and flight-sql.
- **DB-backed keys (steady state)** — `ingestion_api_keys` / `analytics_api_keys`
  rows, validated by hash lookup. Minted, listed, and revoked over HTTP without a
  redeploy. See [API Keys](api-keys.md) for the full reference.

**Benefits (both flavors):**

- Simple to configure
- Fast validation (HashMap lookup for the env keyring; cached hash lookup for
  DB-backed keys)
- No external identity provider dependency

**Limitations:**

- No automatic expiration (this design adds *revocation*, not expiry — a
  DB-backed key with no `revoked_at` is valid indefinitely)
- Manual key distribution and rotation for the env keyring; DB-backed keys add
  `created_by`/`revoked_by` audit trail and HTTP mint/list/revoke, but rotation
  is still a manual operator action, not automatic
- No user identity for audit logging with the env keyring; DB-backed keys record
  `created_by`/`revoked_by` (the OIDC identity that minted/revoked), but not a
  per-request identity beyond the key's own `name`

## Server Configuration

### OIDC Configuration

Configure OIDC authentication using environment variables:

```bash
# OIDC Configuration
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

**Admin Configuration:**

The `MICROMEGAS_ADMINS` environment variable is a JSON array of user identifiers (email or subject) that have administrative privileges. Admin users can perform operations like partition management.

### API Key Configuration

**Steady state: DB-backed keys.** Mint an ingestion key with
`POST /api/ingestion-api-keys`, or an analytics key with
`POST /api/analytics-api-keys` — both on `analytics-web-srv` (OIDC + admin
required) — see [API Keys](api-keys.md) for the full route reference and the
`mmk_`-prefixed key shape.

**Legacy/bootstrap: the env keyring.** Still the only option for
`object-cache-srv` (env-only permanently — see [Object Cache](object-cache.md)),
and still usable as a bootstrap path for ingestion/flight-sql before any
DB-backed key exists:

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

For local development and testing, authentication can be disabled:

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
    Configuring any authentication method above — OIDC, API keys, or both — does more than gate
    access to the server: it flips every session from the internal `ReadScope::All` marker to
    `ReadScope::Audiences`, which activates query-time audience filtering across both enforcement
    prongs. **Prong A** (`OwnershipRewrite`, AbAC Stage 2) injects an audience predicate into
    every `MaterializedView`-backed query plan, so a caller only sees processes whose
    (client-asserted) `micromegas.audience` property resolves to one of their own audiences.
    **Prong B** (`AudienceGuard`, AbAC Stage 3) covers the four functions Prong A structurally
    can't reach — a restricted caller's call to `process_spans`, `perfetto_trace_chunks`,
    `parse_block`, or `get_payload` fails with a not-found-shaped error unless the id argument
    names a process in one of their own audiences, and `list_partitions()` silently omits every
    row (including `'global'` rows) that isn't theirs to see.

    **`MICROMEGAS_UNSTAMPED_AUDIENCE` defaults to `public`** (the monolith's
    `MICROMEGAS_ANALYTICS_`-prefixed equivalent for a monolith deployment), which keeps
    pre-existing, never-stamped data visible without any operator action — see the matching
    env-var row and upgrade note on the [FlightSQL](flight-sql.md#environment-variables) and
    [Monolith](monolith.md#environment-variables) admin pages, and the CHANGELOG's AbAC Stage 2
    upgrade note, for the full mechanism. A deployment that wants the fail-closed behavior instead
    (unstamped data invisible to everyone) must set this var to an empty string explicitly. This
    one knob covers both prongs: Prong B's `'global'`-row rule and its four guarded functions all
    consult the same `MICROMEGAS_UNSTAMPED_AUDIENCE` value Prong A does.

    **API keys and no-`email`-claim OIDC tokens are covered by `public` alone, with no second
    knob.** Under the grant-map model (see [Audiences and Grants](#audiences-and-grants) below),
    every authenticated caller's readable set always includes `public`, regardless of identity —
    there is no caller kind whose resolved set is otherwise empty the way an API key's was under
    the identity-derived model this replaced. The `MICROMEGAS_UNSTAMPED_AUDIENCE` default of
    `public` restores visibility for every caller kind.

    **The two prongs read different copies of `micromegas.audience`, with different freshness.**
    Prong A reads a daemon-materialized parquet snapshot (unchanged from Stage 2 — a process the
    maintenance role hasn't caught up on is invisible to everyone, including its owner); Prong B
    reads Postgres directly, so it is fresher for a just-ingested process, but denies (rather than
    falls back to the stale snapshot) once retention has deleted a process's Postgres row even if
    a merged/compacted lakehouse partition of its data still exists. See the CHANGELOG's AbAC
    Stage 3 entry for the full mechanism and its accepted trade-offs.

    **Eight admin-gated lakehouse UDTFs/UDFs** (`retire_partitions`, `materialize_partitions`,
    `regenerate_partitions`, `retire_partition_by_file`, `retire_partition_by_metadata`, and the
    [query deny list](functions-reference.md#query-deny-list)'s `list_query_denials`,
    `deny_queries`, `remove_query_denial`) are gated on whether this *deployment* can ever
    produce an admin principal at all — not on a knob an operator sets. An OIDC provider can
    grant admin whenever it has at least one configured admin user; an API-key provider never
    can. At startup, the server asks every configured auth provider "can you ever produce an
    admin?" and, if none of them can, registers these eight functions for *any* authenticated
    caller instead of admin-only — otherwise an API-key-only deployment would have no path to
    them at all. **Deployment-wide, not per-audience**: none of the eight functions filters by
    audience, so on a deployment with no admin principal, every authenticated caller gets
    destructive access to every audience's partitions, not just their own, and can also deny
    every query in the deployment via `deny_queries` — safe only when no admin principal exists
    in the deployment, unsafe the moment it also has personal or per-team audiences.

### Write-Side Stamping (AbAC Stage 5)

The read-side filter above is only trustworthy because of what happens on the write side
(ingestion, #1373): `micromegas.audience` is now **server-written from the authenticated
ingestion credential**, never trusted from the client payload. Ingestion strips any
client-supplied `micromegas.*` property and, when the credential carries a bound audience (a
DB-backed `ingestion_api_keys` row), stamps `micromegas.audience` itself before the process's
first block is ever materialized. See [Ingestion → What gets stamped](ingestion.md#what-gets-stamped)
for the full mechanism, credential-by-credential.

Two consequences worth knowing before you flip this stage on:

- **OTLP `process_id` re-derivation.** OTLP-derived identity (`process_id`, and therefore
  `block_id`) is now audience-scoped, so two audiences posting identical resource attributes
  never collapse onto one process — but a deployment that starts stamping re-derives every OTLP
  producer's `process_id` the moment it does. The same logical process appears as a new row;
  its pre-upgrade data keeps the old id and stays unstamped (visible under the
  `MICROMEGAS_UNSTAMPED_AUDIENCE` default of `public` unless an operator has opted into
  fail-closed). Rotating an ingestion key to a different audience likewise splits a long-lived
  producer's history across two process ids — expected, since the data now genuinely belongs to
  two audiences.
- **Client self-stamping stops taking effect.** Before this stage, a native client setting its
  own `micromegas.audience` property was the *only* thing that stamped a process at all. That
  self-stamp is now stripped and replaced by the credential's authenticated audience — which is
  `None` for an env-keyring key or an OIDC token. A producer that relied on self-stamping while
  authenticating with one of those silently becomes **unstamped** on upgrade (visible under the
  `MICROMEGAS_UNSTAMPED_AUDIENCE` default's shared fallback label unless fail-closed is
  configured, and only ever widened to that shared label, never restored to its own). To keep its
  own label, move it onto a DB ingestion key bound to that audience.

!!! warning "Residual gap: cross-audience write injection (tracked, not yet closed)"
    Process *registration* (`insert_process`/`register_otel_process`) now rejects a
    same-`process_id`, different-audience re-registration outright (§6), closing the OTLP
    process-squatting confidentiality gap described below. What's still open is appending to an
    *existing* process a credential didn't create: `insert_stream`/`insert_block` accept any
    `process_id`/`stream_id` unconditionally, so a credential bound to audience A that discovers a
    `process_id`/`stream_id` belonging to audience B can still append events to B's process —
    events that then inherit B's stamped audience. This grants no *read* power (reading B still
    requires a read grant on B), so it is an integrity-only gap, tracked as a follow-up issue
    rather than closed by #1373 (it depends on the same cache layer AbAC Stage 3's Prong B needs).

    Process registration itself is confidentiality-sensitive, not merely an integrity concern:
    `processes` is a single table shared by the native and OTLP paths, and the OTLP `process_id`
    derivation formula is public (see [OTLP Ingestion](../otlp/index.md)). Before this guard, any
    ingestion credential could pre-register (via the native `insert_process` path) the exact
    `process_id` a victim audience's OTLP producer would later derive; the genuine producer's
    stream/blocks would then silently land on a row stamped with the squatter's audience, leaking
    that audience's data to the squatter. `insert_process` and `register_otel_process` both now
    reject such a conflicting re-registration with a 403.

    **A second, distinct residual gap: unstamped pre-registration (confidentiality, not
    integrity).** The conflict guard above only fires on a conflicting *re*-registration -- an
    existing row whose audience is still `NULL` is left alone by design, so a mid-migration
    re-registration doesn't lose its process. That `NULL`-tolerant branch has its own attack: a
    credential with no bound audience (an env-keyring key, OIDC, or `--disable-auth`) can
    pre-register a victim's future `process_id` (the OTLP derivation formula is public again)
    via `insert_process`, creating an unstamped row. The victim's genuine OTLP producer's later
    `register_otel_process` call for that same `process_id` then hits the same `NULL`→no-op
    branch and returns `Ok` -- but the row stays unstamped forever, permanently suppressing the
    victim's stamp. Since `MICROMEGAS_UNSTAMPED_AUDIENCE` now **defaults** to `public`, this makes
    the victim's data world-readable out of the box, not just under an opt-in migration setting.
    There is no in-product enforcement knob left for this specific scenario -- the mitigation is
    operational, not a knob: provision only audience-bound DB-backed ingestion credentials, and
    don't run ingestion with `MICROMEGAS_API_KEYS`, ingestion OIDC, or `--disable-auth` alongside
    them. A deployment that does this has no audience-less writer left to squat with in the
    first place. A deployment that wants this gap fully closed instead of mitigated should set
    `MICROMEGAS_UNSTAMPED_AUDIENCE` to an empty string (fail-closed) in addition to the credential
    hygiene above. Tracked as a follow-up stage of the AbAC epic (#1334) / to #1373 (Stage 5b),
    with no dedicated issue number of its own.

    **The two scenarios recover differently.** In the *first* scenario above (a stamped
    squatter), it is the victim's genuine, later registration that hits the conflict guard and is
    rejected with a 403 -- not the squatter's. Since a stamped process's audience is immutable
    (there is no `UPDATE processes` path anywhere in the codebase), the victim's producer can
    never successfully register that `process_id` again until an operator manually deletes the
    squatted row (e.g. `DELETE FROM processes WHERE process_id = ...`). The maintenance daemon's
    automatic `delete_empty_processes` sweep (`rust/analytics/src/delete.rs`) only reclaims it on
    its own once the squatted row has no streams and the retention window has elapsed -- a
    squatter that also writes a stream keeps the row alive indefinitely.

    The *second* scenario (unstamped pre-registration) is different: there is no 403 and no
    manual recovery step. As described above, the victim's later registration hits the
    `NULL`→no-op branch and returns `Ok` -- the row is simply never stamped and stays silently
    unstamped forever, which is exactly what makes it a confidentiality gap rather than an
    outage. `DELETE`-ing the squatted row doesn't help here either, since nothing rejected the
    write in the first place; preventing the unstamped pre-registration itself requires the
    operational mitigation above -- there is no in-product knob that closes it.

## Audiences and Grants

**A user sees their own grants, and can share/mint self-service, from the Audience Access page**
(`/audiences` in the web app, open to every authenticated user — see
[`web-app.md`](web-app.md#audience-access)) or from SQL via `list_audience_grants()`
(`micromegas-query --all "SELECT * FROM list_audience_grants()"`). The rest of this section
covers the underlying model the page and the CLI below both drive.

An audience is an **opaque label on data** — `public`, `team-alpha`,
`payments-svc` — not an encoding of any principal's identity. What determines
who can read or mint into an audience is separate, editable configuration: a
grant map, resolved once at startup from `{prefix}_AUDIENCE_GRANTS` (falling
back to the unprefixed `MICROMEGAS_AUDIENCE_GRANTS`). This is the model
`AudienceReadPolicy`/`AudienceMintPolicy` (`micromegas_auth::policy`) resolve
against; the [Audience Filtering Activation](#audience-filtering-activation)
section above is what actually consumes the *read* half of it.

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
when the audience should also grant *mint* authority (minting a new
`ingestion_api_keys` row stamped with that audience). An omitted `"mint"`
list is always empty, never derived from `"read"`: a read grant never confers
mint authority. Selectors:

| Selector | Matches |
|---|---|
| `*` | any authenticated principal |
| `user:<email>` | the caller's `email` claim |
| `group:<g>` | any value in the caller's `groups` claim |

**Two built-in rules, and nothing else:**

- **`public` is always readable**, by every authenticated principal, whether
  or not it appears in the map at all — writing `{"public": ["*"]}` changes
  nothing, but an operator who omits it doesn't accidentally hide legacy
  (unstamped, coalesced-to-`public`) data either.
- **There is no self-audience rule.** A caller is never granted an audience
  merely for being named like one — an API key named `team-alpha` does not
  thereby read the `team-alpha` audience. A personal audience is an ordinary
  audience with an ordinary grant entry (e.g. `"alice-laptop":
  ["user:alice@example.com"]`); provisioning one per user by hand this way is
  what self-service mint (#1374, below) removes the need for.

**Re-sharing already-ingested data is a grants edit, never a restamp.** Since
the audience *value* stamped on data never changes, widening who can see
`team-alpha` is a one-line config change — add a selector to its `"read"`
list — that takes effect for every already-ingested process immediately
(bounded by the mint-time key-store cache TTL for callers, not by anything
about the data itself).

**A malformed grant map fails startup, not silently**: an unknown-shaped
key, an unrecognized selector prefix, or a duplicate JSON key for the same
audience are all a startup `Err`, the same "typo fails fast" convention every
other knob on this page follows.

**Worked profiles**, open and privacy:

```bash
# Open deployment: everyone reads everything, no grant map needed at all.
# MICROMEGAS_UNSTAMPED_AUDIENCE left unset: it defaults to public.
export MICROMEGAS_DEFAULT_KEY_AUDIENCE=public

# Privacy deployment: a team's data stays inside the team.
export MICROMEGAS_AUDIENCE_GRANTS='{"team-alpha": ["group:eng"]}'
export MICROMEGAS_DEFAULT_KEY_AUDIENCE=team-alpha
# MICROMEGAS_UNSTAMPED_AUDIENCE set to an empty string, not left unset: opts back into
# fail-closed so legacy/never-stamped data stays invisible instead of defaulting to public.
export MICROMEGAS_UNSTAMPED_AUDIENCE=
```

Worked **mint** profile, granting a non-admin caller mint authority for their
own personal audience — see [self-service mint](#self-service-ingestion-key-mint-abac-stage-6-1374)
below for the full picture (the knob that gates this, the per-caller bounds,
and `micromegas-setup-telemetry`):

```bash
# One admin-created grant per personal audience, mint only (read is granted
# separately, or via a claim -- see below):
micromegas-grants --url https://analytics.example.com create alice-laptop mint user:alice@example.com
micromegas-grants --url https://analytics.example.com create alice-laptop read user:alice@example.com
```

A non-admin caller with this grant can now mint their own `alice-laptop` key
directly (`POST /api/ingestion-api-keys`), once `MICROMEGAS_SELF_SERVICE_MINT`
is on (below) — no further admin step needed for that audience. **Self-service
mint grants must live in the DB `audience_grants` table, never in
`{prefix}_AUDIENCE_GRANTS`** — unlike the read axis (which still unions both
sources, above), the mint axis is DB-only once this stage lands: a mint
audience declared only in the env map is invisible to the lazy claim's
existence check (below) and so could be claimed out from under it by another
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

### Self-service ingestion key mint (AbAC Stage 6, #1374)

Given a `mint` grant already exists (the worked profile above), a non-admin
caller can mint their own ingestion key directly — `POST
{base_path}/api/ingestion-api-keys` is no longer purely admin-gated.
`MintPolicy::resolve_audience` (a per-request point query against
`audience_grants`, never a cached snapshot) is the authorization instead of
an admin gate; an admin's own mint is unaffected either way. This is gated
behind one off-by-default deployment knob:

| Variable | Default | Description |
|---|---|---|
| `MICROMEGAS_SELF_SERVICE_MINT` | `false` | Off by default, so a deployment that upgrades to this stage keeps its exact pre-stage mint authorization surface (admin-only) until an operator explicitly opts in. Also gates `GET {base_path}/api/audience-grants/my-audiences` (below) for non-admin callers. |
| `MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER` | `25` | Caps how many distinct audiences one non-admin caller may lazily claim (below). A backstop against a runaway/abusive caller, not a routine-use quota — reaching it is a pathological event. Best-effort under concurrency. |
| `MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER` | `100` | Caps how many *live* keys one non-admin caller may hold at once. `list_keys`/`revoke_key` stay `AdminUser`-gated, so a non-admin has no self-service way to free a slot once this is reached — reducing the count always requires an admin. |
| `MICROMEGAS_SELF_SERVICE_MAX_GRANTS_PER_CALLER` | `50` | Caps how many rows one non-admin caller may have created in `audience_grants` (counted across every audience/axis/selector, not just the pair being shared into). A backstop against a runaway/abusive caller, not a routine-use quota. Best-effort under concurrency. |

**Audiences are created lazily, not pre-provisioned.** A non-admin caller who
names a brand-new, never-before-granted audience *and supplies the name
explicitly* claims it atomically, as part of the same mint request, once
`MICROMEGAS_SELF_SERVICE_MINT` is on: the claim writes `user:<email>` grant
rows on **both** the `mint` and `read` axes (so the caller who just claimed
the audience can read back what their own new key uploads), inside the same
transaction that mints the key. Naming an audience that already has *any*
grant row — admin-created, self-claimed earlier, or someone else's
in-flight claim — still requires a matching grant exactly as above; only a
genuinely fresh, unowned name is claimable this way. `public` and the
deployment's own `MICROMEGAS_DEFAULT_KEY_AUDIENCE` can never be claimed.

**Before turning on `MICROMEGAS_SELF_SERVICE_MINT`, pre-create a placeholder
grant row — any selector, on either axis — for every audience name that
exists only outside the DB:** a custom `{prefix}_UNSTAMPED_AUDIENCE` override
(irrelevant if left at its `public` default, since `public` can never be
claimed), and *every* key of `{prefix}_AUDIENCE_GRANTS` (mint-relevant or
read-only alike; see the note above). Via the admin grants API:

```bash
micromegas-grants --url https://analytics.example.com create unstamped-legacy read '*'
# ...and one such row per audience named in {prefix}_AUDIENCE_GRANTS, e.g.:
micromegas-grants --url https://analytics.example.com create team-alpha read '*'
```

Without those placeholder rows, the lazy claim's existence check (which reads
only `audience_grants` and `ingestion_api_keys`, never a role-prefixed env
knob it has no reason to know about) would see any of these names as unowned
and let a non-admin claim exclusive mint+read rights over a name the operator
already believes is spoken for.

The setup script, `micromegas-setup-telemetry`, wraps all of this for an end
user — OIDC login, mint, and printing the `OTEL_EXPORTER_OTLP_*` env vars
needed to point their own telemetry at the deployment:

```bash
# Existing grant, or resolved automatically via GET .../my-audiences if omitted:
micromegas-setup-telemetry --url https://analytics.example.com --name my-laptop \
    --audience alice-laptop

# A fresh claim: a non-admin caller's bare name is minted under a namespace
# derived from their own email (e.g. "alice-" + "ci-runner"), never the bare
# name itself -- printed to stderr so the caller sees the resolved name.
micromegas-setup-telemetry --url https://analytics.example.com --name ci-runner \
    --audience ci-runner

eval "$(micromegas-setup-telemetry --url https://analytics.example.com --name my-laptop)"
```

See [`python-api.md`](../query-guide/python-api.md#micromegas-setup-telemetry)
for the full CLI reference, and [`api-keys.md`](api-keys.md) for the mint
route's error shapes (`FORBIDDEN`, `UNAVAILABLE`, `UNAUTHENTICATED`,
`CLAIM_CONTENDED`).

**The same knob now also governs sharing and removal on the Audience Access page** (`/audiences`,
open to every authenticated user — see [`web-app.md`](web-app.md#audience-access)): a non-admin
may create/delete a grant row (per the write policy below) only when
`MICROMEGAS_SELF_SERVICE_MINT` is on, exactly as for minting. Sharing an audience you already
hold is the second half of the self-service feature this knob introduced — claim an audience,
then let your team in — so it rides the same switch rather than a second knob.

**An admin caller minting into a brand-new audience is now also claimed server-side**, the same
way a non-admin's lazy claim already is: `mint_key` runs the same ownership check
(`try_claim_and_mint` uses internally) as a pre-check for an admin caller, and if the audience
looks unclaimed, writes the admin's own `user:<email>` `mint`+`read` rows in the same transaction
as the key insert. This is exactly what `setup_telemetry.py`'s admin branch used to do
client-side (writing the grant via the admin API after minting); it is now the server's job, and
the mint response's new `claimed` field says whether it happened. An admin with no email is
unaffected either way — that caller was never eligible for the client-side grant either.

### DB-backed audience grants (#1489, AbAC Stage 6a)

`{prefix}_AUDIENCE_GRANTS` is resolved once at startup, so creating one
per-user grant means editing an env var and restarting every service that
reads it — workable for a handful of teams, not for a per-user privacy
profile where each new user needs one more grant row. The `audience_grants`
Postgres table (migration v7) is the same grant model, minted, listed, and
deleted over HTTP without a redeploy:

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
single map covering both the read and mint axes, kept splittable later
(without any change to `ReadPolicy`/`MintPolicy`) if the long-term
group-membership model ever needs it.

**Additive, never a replacement.** Each flight-sql process — standalone
`flight-sql-srv`, or the monolith's flight-sql role — holds one whole-table
snapshot in memory (small enough that the issue treats "cache the whole map"
as the right shape, unlike the per-key `moka` cache backing the [DB-backed
key store](api-keys.md)), unioned with the env map before matching a
caller's selectors. `analytics-web-srv` is the write surface only (the HTTP
admin routes below) — it never constructs a `DbAudienceGrantsSource` and
caches nothing itself. For the **read** axis, a selector present in
the env map, the store, or both grants exactly the same access — there is no
"the store wins" or "the env map wins" precedence to reason about, and no
forced migration off `{prefix}_AUDIENCE_GRANTS`: a deployment that never
touches the store keeps working exactly as documented above. This is no
longer true for the **mint** axis once self-service mint (#1374, below)
lands: mint grants are DB-only, so an env-map-only `"mint"` selector is
inert — it is never consulted by `mint_key`'s per-request authorization
check.

**HTTP routes** (#1510, AbAC Stage 6c widens these from admin-only to a
self-service policy — see [self-service mint](#self-service-ingestion-key-mint-abac-stage-6-1374)
below for the knob, and [`web-app.md`](web-app.md#audience-access) for the page that drives
them):

| Route | Body / result |
|---|---|
| `POST {base_path}/api/audience-grants` | `{"audience","axis","selector"}` → 201 (created) or 200 (already existed) `{"audience","axis","selector","created_at","created_by"}` |
| `DELETE {base_path}/api/audience-grants?audience=&axis=&selector=` | 204, or 404/403 |
| `GET {base_path}/api/audience-grants/visible` | 200 `[{"audience","axis","selector","created_at","created_by"}]` — the caller-scoped read backing the Audience Access page's own list (below) |
| `GET {base_path}/api/audience-grants/my-audiences` | Any authenticated caller. 200 `{"is_admin","audiences","mint_prefix","email"}`: the audiences whose `mint` selector matches *this caller's own* identity today, plus the caller's own `is_admin` flag, the caller-derived namespace prefix a fresh claim mints under, and the caller's own email. |

There is no more paginated `GET` over the whole collection — that route is
deleted outright. Listing arbitrary rows from SQL now goes through the
caller-scoped `list_audience_grants()` table function (below) instead, and
the page's own list reads `GET .../visible`.

**`POST`/`DELETE` gate**, `GrantGate` (a `FromRequestParts` extractor
modeled on the mint route's own `MintGate`): an admin acts unconditionally,
exactly as before. A non-admin is admitted only when
`MICROMEGAS_SELF_SERVICE_MINT` is on, and then further constrained per call:

- **Create**: `selector` must be `user:…`/`group:…` (never `*` — a caller
  who can read an audience must not be able to open it to every
  authenticated principal), and the caller must **hold** `(audience, axis)`
  via an identity selector (`user:`/`group:`, not a `*` row — a `*` grant is
  publicly readable/mintable but must not let every authenticated caller
  plant durable rows that would outlive it). Delegation is per axis: a
  `read` grant lets you share `read`, a `mint` grant lets you share `mint`,
  and neither confers the other.
- **Delete**: the row must be the caller's own direct `user:<email>` row
  ("remove my access" — never offered for `group:`/`*` rows, since those
  would affect other principals and there are no negative grants), or a row
  the caller themselves created (the revoke-a-share counterpart of
  sharing) — except their own `mint`/`user:<email>` row, which "remove my
  access" does not cover: that row is the self-service claim marker
  `max_claims_per_caller` counts from, so a non-admin can't delete it
  themselves (an admin still can). A row that doesn't exist at all is 404;
  one that exists but matches no condition is 403.

`DELETE` still takes the natural key as query parameters rather than path
segments: a `group:<id>` selector can contain `/` or other URL-significant
characters a raw path segment can't safely carry.

**`GET .../visible`'s own visibility rule, by caller**: admin sees every
row; a non-admin with the knob on sees every grant on each pair they hold a
matching grant on (the same held-pair rule `list_audience_grants()` uses,
below); a non-admin with the knob off sees only their own rows — never a
sibling's `selector`/`created_by`, since this route (unlike the table
function) *can* check the knob and a default knob-off deployment must not
hand a browsing non-admin that disclosure for free.

The `micromegas-grants` CLI wraps the two write routes, the same
HTTP-only-via-`analytics-web-srv` convention every CLI in this codebase
follows (never direct Postgres access); listing goes through
`micromegas-query` instead:

```bash
micromegas-grants --url https://analytics.example.com create team-alpha read group:eng
micromegas-query --all "SELECT * FROM list_audience_grants()" --profile analytics
micromegas-grants --url https://analytics.example.com delete team-alpha read group:eng
```

### `list_audience_grants()` (#1510, AbAC Stage 6b)

A caller-scoped SQL table function over the `audience_grants` table, registered for **every**
authenticated caller (never admin-gated) — like `list_query_denials()`, it is a SQL auditing
surface, not a REST route. No arguments; filter with `WHERE`. Columns: `audience`, `axis`,
`selector`, `created_at`, `created_by`.

**Visibility**: an admin sees every row. A non-admin sees every grant on each `(audience, axis)`
pair they hold a matching grant on — deliberately wider than "rows whose selector matches me":
if you may read `team-alpha`, you may see who else may, which is exactly the "who can see this
audience" question the function (and the Audience Access page) exists to answer, and it is the
same set a non-admin may modify via the write routes above. A caller with no email and no groups
(an internal/maintenance caller, or a request with no `AuthContext` at all) sees zero rows.

**Unlike `GET .../visible`, this function always applies the held-pair rule for a non-admin,
knob or no knob** — it runs in `flight-sql-srv`/`micromegas-analytics`, which has no visibility
into `analytics-web-srv`'s `MICROMEGAS_SELF_SERVICE_MINT` config, so it structurally cannot apply
the same knob-off narrowing `/visible` does. This is a deliberate, accepted asymmetry between the
two read paths, not a bug. See [Admin Functions Reference](functions-reference.md#list_audience_grants)
for the full schema and query shape.

```bash
micromegas-query --all "SELECT * FROM list_audience_grants() WHERE audience = 'team-alpha'"
```

**Cache-TTL knob**, following the same `{prefix}_` fallback shape as every
other knob on this page:

| Variable | Default | Description |
|---|---|---|
| `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS` | `60` | How long a process serves its in-memory snapshot before re-querying `audience_grants`. Accepts a role prefix on the monolith (`MICROMEGAS_ANALYTICS_AUDIENCE_GRANT_CACHE_TTL_SECONDS`), falling back to the unprefixed name. |

**Revocation takes effect within the cache TTL (default 60s), not
instantly.** This is a stated property, not an oversight: a `DELETE` above
removes the row from `audience_grants` immediately, but every flight-sql
process keeps serving its cached snapshot — and so keeps granting the removed
read access — until its next refresh, up to one full TTL window later.

**Outage behavior is deliberately different from the DB-backed key store's.**
Once a process has loaded the table successfully at least once, a later
refresh failure keeps serving that last good snapshot — unbounded, for as
long as the outage lasts, not just for one more TTL window the way the
per-key cache's TTL eviction bounds a key-lookup outage. The trade favors a
single, deployment-wide grant view degrading to *staleness* over degrading to
*everyone loses every grant simultaneously* on a transient DB blip. A fresh
process whose very first query hits a down DB has no "last good" to serve, so
that case still fails closed like everything else on this seam — `resolve()`
denies exactly as documented, at a rate capped by the same cache-TTL knob
rather than once per request. A sustained outage surfaces on an operator's
dashboard (`audience_grant_refresh_error_count`), not on the request path.

**A malformed row still can't reach a policy decision.** The table's own
`CHECK` constraints are re-validated independently in Rust on every load, so
a row that somehow bypassed them (a manual `psql` fix, a future migration)
fails the *whole* snapshot load loudly instead of silently producing an
unparseable or unreadable grant.

**Deploy-order requirement.** `audience_grants` is created by migration v7,
which only `telemetry-ingestion-srv` and the monolith run (via
`connect_to_remote_data_lake`/`migrate_db`). A standalone `flight-sql-srv`
builds its lakehouse context with `LakehouseContext::from_env()`, which runs
only `migrate_lakehouse` and never touches `migrate_db` — so it never creates
the table itself. Roll the v7 migration (by upgrading ingestion or the
monolith) before or in the same deploy as upgrading `flight-sql-srv` to a
build that wires `DbAudienceGrantsSource`. Upgrading `flight-sql-srv` first
against a still-v6 database is not a startup failure: the process comes up
fine and then fails every `resolve()` call with "relation audience_grants
does not exist" (throttled to one DB attempt per cache-TTL window), failing
every authenticated query with `unavailable` -- `public`-only queries
included -- until the migration lands (see the matching CHANGELOG entry for
this change).

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

### Python Client with API Keys (Legacy)

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

The telemetry ingestion service (telemetry-ingestion-srv) uses the same authentication infrastructure as the analytics service.

### Server Configuration

The ingestion server uses the same environment variables as the analytics server:

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

## Migration from Env API Keys to DB-Backed Keys and OIDC

OIDC is the destination for human/service-account identities. It is **not** the
destination for machine credentials — service keys and Firehose access keys
migrate to DB-backed `ingestion_api_keys` instead, not OIDC. See
[API Keys](api-keys.md) for the full picture (schema, HTTP routes, the
`micromegas-import-keys`-driven
[legacy-key import procedure](api-keys.md#migrating-from-the-env-keyring), and
the `object-cache-srv` exception, which never migrates).

1. **Deploy the new binaries.** The migration creates `ingestion_api_keys` /
   `analytics_api_keys` (schema v5). Nothing changes yet: the env keyring still
   authenticates every existing key, and the DB tables are empty.
2. **Populate the tables** from the existing env keyring using the
   `micromegas-import-keys` CLI tool (see
   [Migrating from the env keyring](api-keys.md#migrating-from-the-env-keyring)
   for the exact commands), or mint fresh keys via `POST /api/ingestion-api-keys`
   on `analytics-web-srv` for callers you can update. A key valid on both
   ingestion and flight-sql today must become two distinct keys — "never both"
   is enforced at the code level once the tables are in use.
3. **Set up an OIDC provider** (Google, Azure AD, etc.) for human/service-account
   identities, if you haven't already.
4. **Update clients**: machine credentials point at their new DB-backed key;
   human/service-account clients switch to OIDC.
5. **Remove `MICROMEGAS_API_KEYS`** (and prefixed variants) only after the tables
   are populated — a non-empty key store counts as "auth configured" on its own,
   so unsetting the env var is safe once step 2 is done, but unsetting it
   *before* the tables are populated leaves machine clients unauthenticated.
   `object-cache-srv` keeps its `MICROMEGAS_API_KEYS` permanently; do not remove
   it there.

**Example Migration:**

```bash
# Step 1/2: Add OIDC configuration and populate ingestion_api_keys (env keys
# still work during this window)
export MICROMEGAS_API_KEYS='[{"name": "service1", "key": "old-key"}]'
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [{
    "issuer": "https://accounts.google.com",
    "audience": "new-client-id.apps.googleusercontent.com"
  }]
}'
# See api-keys.md#migrating-from-the-env-keyring for the exact commands
micromegas-import-keys --table ingestion --source env --var MICROMEGAS_API_KEYS \
  --url https://analytics.example.com

# Step 3/4: Update clients to use OIDC (human/service-account) or their new
# DB-backed key (machine credentials). Test both authentication methods work.

# Step 5: Remove the env keyring once the tables are confirmed populated
unset MICROMEGAS_API_KEYS
# DB-backed keys and OIDC remain
```

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
