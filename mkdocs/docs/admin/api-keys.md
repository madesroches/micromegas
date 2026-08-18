# API Keys

Micromegas keys can live in two Postgres tables — `ingestion_api_keys` and
`analytics_api_keys` — instead of `MICROMEGAS_API_KEYS` (a plaintext JSON env
var, parsed once at startup). The tables hold only a SHA-256 hash of each key
plus a `created_at`/`created_by`/`last_used_at`/`revoked_at`/`revoked_by` audit
trail. **`analytics-web-srv`** is the sole admin HTTP surface for both
tables — its own `/api/ingestion-api-keys*` and `/api/analytics-api-keys*`
routes let an operator mint, list, revoke, and import either table without a
redeploy, writing directly to Postgres. **Ingestion exposes no key-management
HTTP surface at all** — it only validates incoming API keys against
`ingestion_api_keys`, it never mints, lists, revokes, or imports them. Both
key tables have an admin page in the web app (Admin → Ingestion API Keys /
Analytics API Keys), both calling `analytics-web-srv`'s own routes directly
(see [Web app admin pages](#web-app-admin-pages)).

The env keyring still works and is still checked; adopting the key store is an
operator decision, not an automatic upgrade — see
[Migrating from the env keyring](#migrating-from-the-env-keyring).

!!! warning "TLS is a prerequisite for minting and importing"
    Every mint route returns the cleartext key exactly once, over whatever
    transport the request arrives on; every import route carries a legacy
    key's cleartext **inbound** in the request body. Neither the ingestion
    service nor `analytics-web-srv` binds TLS itself — there is no rustls/TLS
    acceptor in either service. **Put a TLS-terminating ingress in front of
    both services before calling any of these routes in anything but a fully
    trusted local network**, or the cleartext key is exposed in flight. There
    is only one hop the cleartext crosses either way — browser or CLI directly
    to `analytics-web-srv` — since `analytics-web-srv` writes straight to
    Postgres for both tables.

## Why two tables

The security model is asymmetric: a stolen write (ingestion) key is an
integrity problem; a stolen read (analytics) credential is a confidentiality
one. A shared table with a `scopes` column would make every key a potential
read credential. With "never both" as a rule, the two tables are kept
separate — one behavior change this implies: **a key valid on both ingestion
and flight-sql today must become two distinct keys** once you migrate off the
env keyring (see [Migrating from the env keyring](#migrating-from-the-env-keyring)).
The code (not just the schema) enforces the split — see [Security](#security).

## Schema

```sql
CREATE TABLE ingestion_api_keys (
  key_id       UUID PRIMARY KEY,
  key_hash     BYTEA NOT NULL,          -- sha256 of the full key string, 32 bytes
  name         VARCHAR(255) NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL,
  created_by   VARCHAR(255) NOT NULL,   -- OIDC email/subject of the minting/importing caller
  last_used_at TIMESTAMPTZ,
  revoked_at   TIMESTAMPTZ,
  revoked_by   VARCHAR(255),
  audience     VARCHAR(255) NOT NULL    -- immutable write audience (migration v6, see below)
    CONSTRAINT ingestion_api_keys_audience_name CHECK (audience ~ '^[A-Za-z0-9_-]+$')
);
CREATE UNIQUE INDEX ingestion_api_keys_key_hash ON ingestion_api_keys(key_hash);

-- analytics_api_keys: identical shape apart from `audience` -- its read-side
-- mirror is a per-key `read_audiences` grant, not a column on this table
CREATE TABLE analytics_api_keys (
  key_id       UUID PRIMARY KEY,
  key_hash     BYTEA NOT NULL,
  name         VARCHAR(255) NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL,
  created_by   VARCHAR(255) NOT NULL,
  last_used_at TIMESTAMPTZ,
  revoked_at   TIMESTAMPTZ,
  revoked_by   VARCHAR(255)
);
CREATE UNIQUE INDEX analytics_api_keys_key_hash ON analytics_api_keys(key_hash);
```

`key_id` is a UUID handle, distinct from `key_hash`: `DELETE {base_path}/api/ingestion-api-keys/<id>`
needs something to key on, and `GET` must never hand out `key_hash` (there's no
reason to distribute the lookup value even though it isn't reversible).
`name` carries no uniqueness constraint — rotating a key under a stable name
means two live rows can legitimately share a `name` while the old one is being
phased out. **Every revoke path keys on `key_id`, never `name`.**

There is no cleartext column, and SHA-256 with no KDF is safe *only* because
these are high-entropy random keys, not passwords — rotate any imported legacy
key that wasn't actually random.

## HTTP routes (key management)

All key-management routes for **both** tables live on **`analytics-web-srv`**,
gated by the same cookie/bearer-auth admin check every other
`analytics-web-srv` admin route uses (`ValidatedUser.is_admin`, resolved from
`MICROMEGAS_ADMINS` or `MICROMEGAS_ANALYTICS_ADMINS` on the monolith — see
[Authentication](authentication.md)). Ingestion itself exposes no
key-management HTTP surface at all — issuing write credentials from the
fleet-facing ingestion service would be the wrong direction for the write/read
asymmetry this design is built on, and consolidating both tables' admin
surface onto one service means there is exactly one admin list to manage
(see [Security](#security)).

| Route | Body / result |
|---|---|
| `POST {base_path}/api/ingestion-api-keys` | `{"name","audience"?}` → 201 `{"key_id","name","created_at","key","audience"}` |
| `GET {base_path}/api/ingestion-api-keys?limit=&offset=&include_revoked=` | 200 `[{"key_id","name","created_at","created_by","last_used_at","revoked_at","revoked_by","audience"}]` |
| `DELETE {base_path}/api/ingestion-api-keys/{key_id}` | 200 `{"revoked_at"}` or 404 |
| `POST {base_path}/api/ingestion-api-keys/import` | `{"name","key","audience"?}` → 201/200 `{"key_id","name","created_at","created_by","revoked_at","imported","audience"}` |
| `POST {base_path}/api/analytics-api-keys` | `{"name"}` → 201 `{"key_id","name","created_at","key"}` |
| `GET {base_path}/api/analytics-api-keys?limit=&offset=&include_revoked=` | 200 `[{"key_id","name","created_at","created_by","last_used_at","revoked_at","revoked_by"}]` |
| `DELETE {base_path}/api/analytics-api-keys/{key_id}` | 200 `{"revoked_at"}` or 404 |
| `POST {base_path}/api/analytics-api-keys/import` | `{"name","key"}` → 201/200 `{"key_id","name","created_at","created_by","revoked_at","imported"}` |

Both route groups share the same request/response shapes and validation for
`name`/`key`/list/revoke; `audience` is an `ingestion_api_keys`-only field —
see [What audience does a key carry](#what-audience-does-a-key-carry) — that
`analytics_api_keys`'s routes never accept or return.

**Mint** (`POST .../{table}-api-keys`) — `{"name"}` (plus, for ingestion,
an optional `"audience"`) → **201**
`{"key_id","name","created_at","key"}` (plus `"audience"` for ingestion). The
`key` field is the cleartext key,
returned **exactly once** — it is never logged (only `key_id` is) and never
retrievable afterwards. `mmk_` marks the key as a Micromegas secret for
scanners; validation covers the whole string via its hash, so imported legacy
keys of any shape keep working. **400** if `name` is empty or exceeds 255
bytes (stricter than the `VARCHAR(255)` column, which bounds characters, not
bytes); for ingestion, also **400** if `audience` is neither a valid audience
name nor resolvable from `MICROMEGAS_DEFAULT_KEY_AUDIENCE` — see
[What audience does a key carry](#what-audience-does-a-key-carry).

**List** (`GET .../{table}-api-keys?limit=&offset=&include_revoked=`) — **200**,
newest first. `limit` defaults to `100`; values above `500` are silently
clamped to `500` (a read endpoint, so capping is safer than rejecting);
`limit <= 0` is **400**. `offset` defaults to `0`. `include_revoked` defaults
to `true` — an operator investigating an incident needs to see that a key
*was* revoked and when. **Never `key_hash`, never the key.**

**Revoke** (`DELETE .../{table}-api-keys/{key_id}`) — **200**
`{"revoked_at"}`, idempotent: a second `DELETE` returns the same `revoked_at`
rather than overwriting it. **404** for an unknown `key_id`. No
`effective_within_seconds` field on the response — `analytics-web-srv` runs no
`DbApiKeyAuthProvider` of its own, so there's no running cache TTL on *this*
process to report; the revocation latency is still bounded by whichever
ingestion/flight-sql process's cache TTL is validating the key (see
[Cache and audit env vars](#cache-and-audit-env-vars)).

**Import** (`POST .../{table}-api-keys/import`) — the one gap mint doesn't
cover: carrying a *pre-existing* key string forward, rather than generating a
fresh one. This is what lets an existing client keep presenting the same key
string after migrating off the env keyring — see
[Migrating from the env keyring](#migrating-from-the-env-keyring).
`{"name","key"}` (plus, for ingestion, an optional `"audience"`) →
`imported: true` and **201** on a fresh insert;
`imported: false` and **200** when the hash already exists (idempotent
re-run — importing the same legacy keyring twice has no side effects).
`revoked_at` is always present (`null` unless the existing row was itself
revoked), so a caller can distinguish "already present and usable" from
"already present but revoked". For ingestion, the response also carries
`audience`: on the fresh-insert path, whatever resolved from the request/knob
(see [What audience does a key carry](#what-audience-does-a-key-carry)); on
the already-present path, the **existing** row's audience, never the
request's — the binding is immutable, so a second import can never rewrite
it. `created_by` is the importing caller's own
OIDC identity (`email` or `subject`, the same resolution mint uses) — never
the literal string `"import"`. **400** if `name` is empty/too long (same rule
as mint) or `key` is empty. No format validation on `key` beyond non-empty —
`hash_key` covers the whole string regardless of shape, which is what lets a
legacy key of any format (including one that never had the `mmk_` prefix)
import cleanly. Never logs the key, same as mint. The
`micromegas-import-keys` CLI tool (see
[Migrating from the env keyring](#migrating-from-the-env-keyring)) is the
recommended way to call this route in bulk; it can also be called directly.

**Precondition: the telemetry DB must already have the v6 migration**
(v5 creates both `ingestion_api_keys` and `analytics_api_keys`; v6 adds
`ingestion_api_keys.audience`, `NOT NULL`), which only
ingestion or a lakehouse-role monolith runs — a standalone `analytics-web-srv`
or a `--roles web`-only monolith never runs it themselves. Run ingestion (or a
lakehouse-role monolith) against the target telemetry DB at least once before
relying on these routes, or every call fails at request time with an opaque
`500` — a missing table (short of v5) or a missing `audience` column (short
of v6) are the same symptom with different causes.

**The `NOT NULL` `audience` column (with no `DEFAULT`, deliberately) also
imposes a deploy-order requirement in the opposite direction.** Once the
schema reaches v6, a not-yet-upgraded `analytics-web-srv` process's
mint/import `INSERT`s (which list columns explicitly and, before this
column existed, omitted `audience`) start failing with a `NOT NULL`
violation (**500**) — same symptom as the missing-column case above, opposite
cause. **Upgrade `analytics-web-srv` to a version that writes `audience` in
the same deploy that runs the v6 migration** — running the migration first
without also rolling the web service, or the reverse, both produce an outage
window on these two routes until both sides catch up. Key *validation* is
unaffected either way: the ingestion/monolith binary that reads the column
back (via `DbApiKeyAuthProvider`) is always the same binary that just ran the
migration creating it, so it can never run ahead of the schema — this
deploy-order requirement is specific to `analytics-web-srv`'s two write
routes.

**One env var backs both route groups:**

| Variable | Description |
|---|---|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Telemetry-DB connection string `analytics-web-srv` opens its own small (`max_connections(2)`) pool from, backing **both** the ingestion-key and analytics-key routes (the same pool is reused for both tables — they live in the same database). `None` (both route groups 503) if unset. |

When `MICROMEGAS_SQL_CONNECTION_STRING` is unset, both route groups stay
registered and return **503** — same "always registered, 503 when
unconfigured" shape as `/api/maps/*`.

## What audience does a key carry

Every `ingestion_api_keys` row carries a single, **immutable** write audience
(migration v6) — the value every process that key ingests is stamped with
(`micromegas.audience`, server-written at ingestion time, #1373). `analytics_api_keys`
has no such column: its read-side equivalent is a per-key `read_audiences`
grant, in the opposite direction (which audiences a caller may *read*, not
which one it *writes*).

**An audience is an opaque label, not a principal encoding** — `public`,
`team-alpha`, `payments-svc`, `alice-laptop`. It carries no meaning by itself;
who may read or mint into it is separate, editable configuration (a grant
map, `{prefix}_AUDIENCE_GRANTS` — see [Audiences and Grants](authentication.md#audiences-and-grants)
for the full model). `public` is the one built-in: every authenticated
principal can read it, with no grant map entry needed.

**The binding is immutable by design.** Once a key is minted or imported with
an audience, that audience never changes for that key — not through a later
mint/import call, not through any route this page documents. Re-sharing
already-ingested data with a wider audience is a *grants* edit (add a
selector to the audience's entry in `{prefix}_AUDIENCE_GRANTS`), never a
restamping of the key or its already-ingested history.

**`mint` requires an explicit audience or a working
`MICROMEGAS_DEFAULT_KEY_AUDIENCE` — there is
no built-in default.** A new credential's *entire future* ingestion history
follows this one choice, so an unresolvable mint is a **400**, never a silent
`public`: defaulting a fresh write credential to a universally-readable
audience would publish everything it ever ingests. `import` is different — a
legacy key's already-ingested history is either unstamped (visible only
through `MICROMEGAS_UNSTAMPED_AUDIENCE`) or, once migration v6's backfill has
run, already `public` — so `import` falls back to `public` when neither the
request nor the knob supplies one, matching that continuity rather than
erroring.

**Data ingested through the env keyring (`MICROMEGAS_API_KEYS`) is never
stamped at all.** That keyring has no audience column to carry one, by
design (per the umbrella data-isolation plan) — its data stays visible only
through `MICROMEGAS_UNSTAMPED_AUDIENCE`, the same escape hatch that covers
any process minted before this stage existed.

**A hand-edited row takes effect within the key's cache TTL, not instantly** —
the audience is cached alongside the rest of the row
(`MICROMEGAS_API_KEY_CACHE_TTL_SECONDS`, default 60s; see
[Cache and audit env vars](#cache-and-audit-env-vars)), and since it's
immutable that caching is free — there's no invalidation to reason about.

### Minting an analytics key over HTTP

```bash
curl -X POST https://analytics.example.com/api/analytics-api-keys \
  -H 'Content-Type: application/json' -H "Cookie: id_token=$TOKEN" \
  -d '{"name": "grafana-datasource"}'
```

Or use the Admin → Analytics API Keys page in the web app, which shows the
minted key exactly once in a dismissable banner with a copy-to-clipboard
button — the browser never receives it a second time, and it's never
persisted client-side.

Minting an ingestion key uses the same shape, against
`/api/ingestion-api-keys` instead (or the Admin → Ingestion API Keys page) —
`analytics-web-srv` is the mint target for both tables — but the body must
supply an `audience`:

```bash
curl -X POST https://analytics.example.com/api/ingestion-api-keys \
  -H 'Content-Type: application/json' -H "Cookie: id_token=$TOKEN" \
  -d '{"name": "grafana-datasource", "audience": "team-alpha"}'
```

Omitting `audience` returns **400** unless `MICROMEGAS_DEFAULT_KEY_AUDIENCE`
is configured server-side — see [What audience does a key
carry](#what-audience-does-a-key-carry).

**Revoke** — `DELETE {base_path}/api/{ingestion,analytics}-api-keys/{key_id}`,
keyed **only** on `key_id` (`name` carries no uniqueness constraint — a
by-name revoke during rotation could revoke the freshly minted replacement
along with the row being retired). `GET {base_path}/api/{ingestion,analytics}-api-keys`
is now the way to discover a `key_id` to revoke — there is no more
"hand-SQL only" list step.

## Web app admin pages

Two admin pages in the web app, both under **Admin** in the sidebar, and both
requiring an OIDC admin session (`AuthGuard requireAdmin`):

- **Analytics API Keys** (`/admin/analytics-keys`) — calls
  `analytics-web-srv`'s own `/api/analytics-api-keys*` routes directly
  (above).
- **Ingestion API Keys** (`/admin/ingestion-keys`) — calls
  `analytics-web-srv`'s own `/api/ingestion-api-keys*` routes directly
  (above). No proxy, no forwarding to ingestion, no service credential:
  ingestion itself exposes no key-management HTTP surface at all.

Both pages stay visible even when the backing pool isn't configured; in that
case the page surfaces the 503 the route returns, same as `MapsPage` does for
an unconfigured maps store. Neither page has an "import" button — the import
routes exist for the `micromegas-import-keys` CLI tool only, since a browser
form for pasting a legacy key in would reintroduce the "key transits a
browser" exposure mint already avoids.

**`created_by`/`revoked_by` always reflect the acting admin's own OIDC
identity, for both key tables.** Every mint/revoke/import handler resolves
the caller from `analytics-web-srv`'s own `AdminUser` extractor
(`user.email` or `user.subject`) and writes that directly — there is no
service-credential hop for either table, so there is no attribution gap to
document. (An earlier design proxied ingestion-key calls through a dedicated
service credential, which meant every proxied row was attributed to that
credential instead of the acting admin; direct writes remove that gap
entirely rather than just documenting it.)

**Single admin list.** Both route groups gate on the same
`analytics-web-srv` admin check (`MICROMEGAS_ADMINS` /
`MICROMEGAS_ANALYTICS_ADMINS`) — there is exactly one admin list to manage
for key administration, not two lists that must be kept in sync.

**Upgrade note: this merge is unconditional, with no opt-out.** On the
previous, proxy-based design, ingestion-key mint/revoke/import from
`analytics-web-srv` only worked if an operator configured
`MICROMEGAS_INGESTION_ADMIN_URL` plus the `MICROMEGAS_INGESTION_PROXY_OIDC_*`
quartet; leaving the proxy unconfigured was the documented way to keep the
two admin lists separate. That knob is gone. `ingestion_api_keys` mint/list/
revoke/import now hang off the same `analytics_keys_pool` (resolved from
`MICROMEGAS_SQL_CONNECTION_STRING`) that already backs `analytics_api_keys`
administration, so every existing deployment's `MICROMEGAS_ADMINS`/
`MICROMEGAS_ANALYTICS_ADMINS` list silently and irreversibly becomes an
ingestion-key-admin list too the moment it upgrades — there is no
configuration left to turn that off short of unsetting
`MICROMEGAS_SQL_CONNECTION_STRING` entirely, which also disables
analytics-key administration. Operators who deliberately kept the two admin
lists separate (i.e. ran with the proxy unconfigured on purpose) must
re-audit `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS` **before**
upgrading, not after.

**Under `--disable-auth` on `analytics-web-srv`, both key-management route
groups are unavailable — not just gated, but not merged at all.** With auth
disabled, every request would otherwise be treated as an admin, which would
let an unauthenticated caller mint/revoke real keys; instead both path
prefixes are answered by a fixed 503 (`{"code": "AUTH_DISABLED", ...}`),
including any sub-path.

## Cache and audit env vars

| Variable | Default | Description |
|---|---|---|
| `MICROMEGAS_API_KEY_CACHE_SIZE` | `10000` | Max distinct live keys cached per process |
| `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS` | `60` | Positive-cache TTL — also the revocation-latency bound |
| `MICROMEGAS_API_KEY_UNKNOWN_CACHE_TTL_SECONDS` | `10` | Negative-cache TTL (shorter, so a freshly minted key isn't masked by an earlier probe) |
| `MICROMEGAS_API_KEY_UNKNOWN_CACHE_SIZE` | `10000` | Max distinct unknown tokens cached per process |

Each accepts a role prefix on the monolith — `MICROMEGAS_INGESTION_API_KEY_CACHE_TTL_SECONDS`
/ `MICROMEGAS_ANALYTICS_API_KEY_CACHE_TTL_SECONDS` — falling back to the
unprefixed name, the same convention `MICROMEGAS_API_KEYS` /
`MICROMEGAS_OIDC_CONFIG` / `MICROMEGAS_ADMINS` already use.

**Revocation takes effect within `cache_ttl_secs` (default 60s), not
instantly.** This is a stated property, not an oversight — raising the TTL
trades revocation latency for DB load.

## Monitoring a key-store outage

A Postgres outage on the key-lookup path is surfaced as **`503`** (or
`Status::unavailable` on the gRPC/FlightSQL path) — never a `401` — so clients
retry rather than treat it as a rejected credential. The signal to alert on is
the `db_api_key_error_count` metric (tagged `{table}`), emitted on **every**
DB error the key-lookup path hits, unconditionally — independent of the
`error!` log line for the same error, which is rate-limited to at most once per
`cache_ttl_secs` (floored at 60s) window per table to avoid flooding
`log_entries` with the outage's own noise during a sustained outage.

## Grant recipe (separated DB roles)

The umbrella data-isolation plan states "Postgres grants enforce the split, not
application logic." That is achievable, but **not true of a deployment as
shipped today** — every service shares one DB role via
`MICROMEGAS_SQL_CONNECTION_STRING`, and the schema migration runs as the owner.
What ships today is a *code*-level boundary: `analytics-web-srv`'s
mint/list/revoke/import routes each hardcode which table they target
(`ingestion_keys.rs` → `ingestion_api_keys`, `analytics_keys.rs` →
`analytics_api_keys`), and the ingestion/analytics key-validation providers
are each constructed bound to their own table — no parameter anywhere could
point one at the other. Operators who do separate DB roles per service can
additionally enforce the split at the grant level:

```sql
-- ingestion role: read + touch only (key *validation*, not administration —
-- analytics-web-srv is the only role that mints/revokes/imports)
GRANT SELECT ON ingestion_api_keys TO micromegas_ingestion;
GRANT UPDATE (last_used_at) ON ingestion_api_keys TO micromegas_ingestion;
-- and no grant of any kind on analytics_api_keys

-- analytics role: read + touch only (flight-sql's own key-validation identity)
GRANT SELECT ON analytics_api_keys TO micromegas_analytics;
GRANT UPDATE (last_used_at) ON analytics_api_keys TO micromegas_analytics;
-- and no grant of any kind on ingestion_api_keys

-- analytics-web-srv's own role: write + touch only, on BOTH key tables --
-- it is the sole admin surface for both
GRANT SELECT, INSERT ON ingestion_api_keys TO micromegas_web;
GRANT UPDATE (revoked_at, revoked_by) ON ingestion_api_keys TO micromegas_web;
GRANT SELECT, INSERT ON analytics_api_keys TO micromegas_web;
GRANT UPDATE (revoked_at, revoked_by) ON analytics_api_keys TO micromegas_web;
```

Note `micromegas_web` (`analytics-web-srv`'s role) is the only role granted
`INSERT` on either table in a fully separated-role deployment — mint/import
write fresh rows, revoke only ever touches `revoked_at`/`revoked_by` on an
existing one. `micromegas_ingestion` and `micromegas_analytics` are each
read + `last_used_at`-touch only, on their own table, with no overlap in
column grants and no grant of any kind on the other service's table.
**`analytics-web-srv`'s role does gain write access to `ingestion_api_keys`**
under this design — that is the point of removing ingestion's own admin
routes, not a gap: every ingestion-key mint/revoke/import now goes through
`analytics-web-srv`, so its role is the one that needs the write grant.

## Migrating from the env keyring

Carrying *existing* key strings forward is now an HTTP-backed operation via
the `import` route on `analytics-web-srv` for each table (above) and the
`micromegas-import-keys` CLI tool that drives it — no `psql`, no direct
Postgres network access, an HTTP-reachable workstation is enough. The tables
and the mint/list/revoke APIs stand on their own without the import tool; it
only affects carrying *existing* key strings forward.

1. **Deploy the new binaries.** The migration creates the tables (schema v5).
   Nothing changes yet: the env keyring still authenticates every existing key,
   and the DB tables start empty. In a split deployment, start ingestion (or
   the monolith) before flight-sql — flight-sql never runs the migration itself.
   Its startup check for a live key store only *aborts* startup when no other
   auth provider is configured; since the env keyring (or OIDC) is still
   authenticating at this stage, violating this ordering instead surfaces as a
   `warn!` log line naming the table, and flight-sql starts anyway. That
   warning is not safe to ignore: once step 3 removes `MICROMEGAS_API_KEYS`
   and OIDC isn't configured either, the DB-backed provider is the only thing
   left, and a schema still short of v5 (or a key store that's simply empty)
   then makes flight-sql **fail to start**, not serve `503`s. (If OIDC is
   still configured, a v5-short schema at that point doesn't fail startup —
   the existence check is only `warn!`-logged — but every subsequent key
   lookup through the DB provider hits the same missing relation and returns
   `503` at request time.) Fix the ordering as soon as the warning appears,
   rather than waiting for step 3 to surface it as a startup failure or an
   outage. For these key-management routes specifically, `analytics-web-srv`
   never runs this migration itself either — the target telemetry DB must
   already have had ingestion or a lakehouse-role monolith run against it at
   least once (see [HTTP routes (key management)](#http-routes-key-management)).
2. **Populate the tables with `micromegas-import-keys`** — installed
   alongside `micromegas-query`/`-screens`/`-logout` (`pip install micromegas`,
   or via poetry in `python/micromegas`):

   ```bash
   micromegas-import-keys --table ingestion --source env \
     --url https://analytics.example.com
   micromegas-import-keys --table analytics --source env \
     --url https://analytics.example.com
   ```

   `--table` selects the import route; `--url` always points at
   `analytics-web-srv`'s base URL, for **both** tables — ingestion itself
   exposes no import route (or any key-management route) to point at.
   With no explicit `--var`, `--source env` tries the table's own prefixed
   legacy var first (`MICROMEGAS_INGESTION_API_KEYS` /
   `MICROMEGAS_ANALYTICS_API_KEYS`) and falls back to the unprefixed
   `MICROMEGAS_API_KEYS` — the same `{PREFIX}_API_KEYS`-falls-back-to-
   `MICROMEGAS_API_KEYS` convention `ProviderBuilder` uses, so this recipe
   works unmodified whether the legacy keyring was populated by the monolith
   (which reads the prefixed name) or by split `telemetry-ingestion-srv` /
   `flight-sql-srv` (which only ever read the unprefixed name). Pass
   `--var NAME` explicitly to pin an exact source var, or `--source file
   --path ...` to read the legacy keyring's real shape — a JSON array of
   `{"name", "key"}` objects, each optionally carrying an `"audience"` field
   too (ingestion only) — from a file instead. `--audience AUD` sets the
   audience for every ingestion key imported in this run (valid only with
   `--table ingestion`; a per-entry `"audience"` in the keyring wins over it).
   This is a **different** audience than the one already threaded through
   this same tool for OIDC token validation (`MICROMEGAS_OIDC_AUDIENCE`) — the
   flag name coincidence is unrelated. Neither given, the server applies
   `MICROMEGAS_DEFAULT_KEY_AUDIENCE`, falling back to `public` — see
   [What audience does a key carry](api-keys.md#what-audience-does-a-key-carry).
   Auth
   follows the same OIDC setup as `micromegas-screens`/`-query`
   (`MICROMEGAS_OIDC_*` env vars for a service-account/non-interactive run,
   or an interactive/cached login via `--profile`); the OIDC identity used
   must be in the target service's admin list. The tool prints one line per
   key (`imported` / `already present (key_id=...)` / `already present
   (revoked)` / the error message on a 4xx), continues past individual
   failures, and exits non-zero if any key failed to import or came back
   revoked. Route each key explicitly with `--only NAME [NAME ...]` /
   `--exclude NAME [NAME ...]` (mutually exclusive):
   - An `object-cache-srv` client key stays env-only forever — never imported.
   - A key that is also a service's own ingestion self-telemetry credential
     (`MICROMEGAS_INGESTION_API_KEY`) goes into `ingestion_api_keys`.
   - **A key valid on both ingestion and flight-sql today must become two
     distinct key strings, one per table** — split it first, then import
     each half with `--only`/`--exclude` selecting disjoint entries per
     table run. This is the one client-visible change this migration
     doesn't avoid; the tool keeps no state file to catch this
     automatically.
3. **Remove `MICROMEGAS_API_KEYS`** (and prefixed variants) from ingestion and
   flight-sql once the tables are populated. A non-empty key store counts as
   "auth configured" on its own, so both services keep serving without OIDC —
   including a key-only flight-sql deployment (see
   [Grafana Authentication](../grafana/authentication.md)). `object-cache-srv`
   keeps `MICROMEGAS_API_KEYS` **permanently** — see
   [Object Cache](object-cache.md).

## Security

- **API keys can never manage keys**: `is_admin` is hardcoded `false` on every
  API-key auth context, and `analytics-web-srv`'s `AdminUser` extractor rejects
  any caller whose `is_admin` isn't `true` before a mint/list/revoke/import
  handler ever runs — an API-key caller can never satisfy that gate.
- **No cleartext at rest.** Only a SHA-256 hash is stored; the cleartext key
  exists only in the one-time `POST` response and in the client's own storage.
- **No timing side channel** on the DB path — the token is hashed and the hash
  used as an index key, so lookup time doesn't depend on how many leading
  bytes of a guess are correct.
- **Revocation latency is bounded, not zero** — see
  [Cache and audit env vars](#cache-and-audit-env-vars).
- **A key-store outage is never client-visible as a rejected credential** — see
  [Monitoring a key-store outage](#monitoring-a-key-store-outage).
