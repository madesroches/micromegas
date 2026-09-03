# API Keys

Micromegas keys live in two Postgres tables — `ingestion_api_keys` and
`analytics_api_keys` — or in `MICROMEGAS_API_KEYS` (a plaintext JSON env var,
parsed once at startup). The tables hold only a SHA-256 hash of each key plus
a `created_at`/`created_by`/`last_used_at`/`revoked_at`/`revoked_by` audit
trail.

**`analytics-web-srv`** is the sole HTTP surface for both tables — its
`/api/ingestion-api-keys*` and `/api/analytics-api-keys*` routes let an
operator mint, list, revoke, and import either table without a redeploy,
writing directly to Postgres. **Ingestion exposes no key-management HTTP
surface**: it only validates incoming keys against `ingestion_api_keys`.
Both tables have an admin page in the web app (Admin → Ingestion API Keys /
Analytics API Keys) that calls `analytics-web-srv`'s routes directly (see
[Web app admin pages](#web-app-admin-pages)).

Minting an ingestion key is not purely an admin operation: a non-admin caller
with a matching `mint` grant — or naming a brand-new audience explicitly,
which lazily claims it — can mint their own `ingestion_api_keys` row
directly, once an operator turns on `MICROMEGAS_SELF_SERVICE_MINT` (off by
default). See [Self-service mint](authorization.md#self-service-ingestion-key-mint)
for the full mechanism; every other route (list/revoke/import, and the
analytics-key table entirely) stays admin-only.

The env keyring still works and is still checked; adopting the DB-backed key
store is an operator decision — see [Migrating from the env
keyring](#migrating-from-the-env-keyring).

!!! warning "TLS is a prerequisite for minting and importing"
    Every mint route returns the cleartext key exactly once, over whatever
    transport the request arrives on; every import route carries a legacy
    key's cleartext **inbound** in the request body. Neither the ingestion
    service nor `analytics-web-srv` binds TLS itself. **Put a TLS-terminating
    ingress in front of both services before calling any of these routes in
    anything but a fully trusted local network.**

## Why two tables

The security model is asymmetric: a stolen write (ingestion) key is an
integrity problem; a stolen read (analytics) credential is a confidentiality
one. Keeping the tables separate means a key valid on both ingestion and
flight-sql must be two distinct keys — see [Migrating from the env
keyring](#migrating-from-the-env-keyring). The split is enforced in code, not
just schema — see [Security](#security).

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
  audience     VARCHAR(255) NOT NULL    -- immutable write audience
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

`key_id` is a UUID handle, distinct from `key_hash`: `DELETE
{base_path}/api/ingestion-api-keys/<id>` keys on it, and `GET` never returns
`key_hash`. `name` carries no uniqueness constraint — rotating a key under a
stable name means two live rows can share a `name` while the old one is
retired. **Every revoke path keys on `key_id`, never `name`.**

There is no cleartext column. SHA-256 with no KDF is safe only because these
are high-entropy random keys, not passwords — rotate any imported legacy key
that wasn't actually random.

## HTTP routes (key management)

All key-management routes for **both** tables live on `analytics-web-srv`.
Every route except ingestion's own mint is gated by the same admin check
every other `analytics-web-srv` admin route uses (`ValidatedUser.is_admin`,
resolved from membership in the reserved `admins` local group; see
[Groups](groups.md)). `POST {base_path}/api/ingestion-api-keys`
(mint) runs through a `MintGate`/`AuthenticatedUser` extractor instead, so a
non-admin caller with a matching grant (or a lazy claim) can reach it once
`MICROMEGAS_SELF_SERVICE_MINT` is on. Ingestion itself exposes no
key-management HTTP surface — consolidating both tables' admin surface onto
one service keeps a single admin list (see [Security](#security)).

| Route | Body / result |
|---|---|
| `POST {base_path}/api/ingestion-api-keys` | `{"name","audience"?}` → 201 `{"key_id","name","created_at","key","audience","claimed"}` |
| `GET {base_path}/api/ingestion-api-keys?limit=&offset=&include_revoked=` | 200 `[{"key_id","name","created_at","created_by","last_used_at","revoked_at","revoked_by","audience"}]` |
| `DELETE {base_path}/api/ingestion-api-keys/{key_id}` | 200 `{"revoked_at"}` or 404 |
| `POST {base_path}/api/ingestion-api-keys/import` | `{"name","key","audience"?}` → 201/200 `{"key_id","name","created_at","created_by","revoked_at","imported","audience"}` |
| `POST {base_path}/api/analytics-api-keys` | `{"name"}` → 201 `{"key_id","name","created_at","key"}` |
| `GET {base_path}/api/analytics-api-keys?limit=&offset=&include_revoked=` | 200 `[{"key_id","name","created_at","created_by","last_used_at","revoked_at","revoked_by"}]` |
| `DELETE {base_path}/api/analytics-api-keys/{key_id}` | 200 `{"revoked_at"}` or 404 |
| `POST {base_path}/api/analytics-api-keys/import` | `{"name","key"}` → 201/200 `{"key_id","name","created_at","created_by","revoked_at","imported"}` |

Both route groups share request/response shapes and validation for
`name`/`key`/list/revoke; `audience` is an `ingestion_api_keys`-only field —
see [What audience does a key carry](#what-audience-does-a-key-carry).

Ingestion's mint route has more error shapes than the rest of this table,
since it is the one route here a non-admin caller can reach: `400
BAD_REQUEST`, `503 NOT_CONFIGURED` (no DB pool), `403 FORBIDDEN`
(self-service off, no matching grant, or a per-caller bound reached), `503
UNAVAILABLE` (the audience-grant query itself failed), `401 UNAUTHENTICATED`
(no `AuthContext`, normally unreachable), and `409 CLAIM_CONTENDED` (two
concurrent lazy claims raced for the same audience name; retry).

**Mint** (`POST .../{table}-api-keys`) — `{"name"}` (plus, for ingestion, an
optional `"audience"`) → **201** `{"key_id","name","created_at","key"}` (plus
`"audience"` for ingestion). `key` is the cleartext key, returned **exactly
once** — never logged, never retrievable afterwards. `mmk_` marks the key as
a Micromegas secret for scanners; validation hashes the whole string, so
imported legacy keys of any shape keep working. **400** if `name` is empty or
exceeds 255 bytes; for ingestion, also **400** if an explicit `audience` is
invalid — an omitted one resolves to the deployment default (see [What
audience does a key carry](#what-audience-does-a-key-carry)).

**List** (`GET .../{table}-api-keys?limit=&offset=&include_revoked=`) —
**200**, newest first. `limit` defaults to `100`, clamps at `500`, and is
**400** if `<= 0`. `offset` defaults to `0`. `include_revoked` defaults to
`true`. Never returns `key_hash` or the key.

**Revoke** (`DELETE .../{table}-api-keys/{key_id}`) — **200**
`{"revoked_at"}`, idempotent (a second call returns the same value). **404**
for an unknown `key_id`. The revocation latency is bounded by whichever
ingestion/flight-sql process's cache TTL is validating the key — see [Cache
and audit env vars](#cache-and-audit-env-vars).

**Import** (`POST .../{table}-api-keys/import`) — carries a *pre-existing*
key string forward, for a client keeping the same key string after migrating
off the env keyring. `{"name","key"}` (plus, for ingestion, an optional
`"audience"`) → `imported: true` and **201** on a fresh insert; `imported:
false` and **200** when the hash already exists (idempotent). `revoked_at` is
always present (`null` unless the existing row was itself revoked). For
ingestion, the response also carries `audience`: on a fresh insert, whatever
resolved from the request/knob; on an already-present row, the row's
**existing** audience (the binding is immutable). `created_by` is the
importing caller's own OIDC identity. **400** if `name` is empty/too long or
`key` is empty; no other format validation. Never logs the key. The
`micromegas-import-keys` CLI (see [Migrating from the env
keyring](#migrating-from-the-env-keyring)) is the recommended way to call
this route in bulk.

**Precondition:** the telemetry DB must already have run the migration that
creates `ingestion_api_keys.audience` (schema v6), which only ingestion or a
lakehouse-role monolith runs. Run one of those against the target DB at least
once before relying on these routes, or every call fails with an opaque
`500`.

**Deploy ordering matters in the other direction too**, since `audience` is
`NOT NULL` with no default: once the schema is at v6, a not-yet-upgraded
`analytics-web-srv` whose `INSERT`s omit `audience` starts failing with a
`NOT NULL` violation (**500**). Upgrade `analytics-web-srv` to a version that
writes `audience` in the same deploy that runs the v6 migration.

**One env var backs both route groups:**

| Variable | Description |
|---|---|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Telemetry-DB connection string `analytics-web-srv` opens its own small (`max_connections(2)`) pool from, backing both route groups. Required whenever auth is enabled — `analytics-web-srv` bails at startup if it's unset. Under `--disable-auth`, both route groups instead return a fixed **503** (`AUTH_DISABLED`). |

## What audience does a key carry

Every `ingestion_api_keys` row carries a single, **immutable** write
audience — the value every process, stream, and block that key ingests is
stamped with. `analytics_api_keys` has no such column: its read-side
equivalent is a per-key `read_audiences` grant, in the opposite direction
(which audiences a caller may *read*, not which one it *writes*).

An audience is an opaque label, not a principal encoding — `public`,
`team-alpha`, `payments-svc`, `alice-laptop`. Who may read or mint into it is
separate, editable configuration: rows in the `audience_grants` table
(`POST`/`GET`/`DELETE {base_path}/api/audience-grants`, or the
`micromegas-grants` CLI). The deprecated `MICROMEGAS_AUDIENCE_GRANTS` env map
is still unioned in on the read axis where it is set, but never on the mint
axis. See [Audiences and Grants](authorization.md#audiences-and-grants) for the
full model. A fresh deployment ships with a seeded `('public', 'read', '*')`
row, which makes every authenticated principal able to read `public` with no
further grant; delete that row to change it.

**The binding is immutable by design.** Once a key is minted or imported with
an audience, that audience never changes for that key. Re-sharing
already-ingested data with a wider audience is a *grants* edit (add a `read`
selector for that audience), never a restamp.

**A request that names no audience gets the deployment default**
(`MICROMEGAS_DEFAULT_AUDIENCE`, `public` when unset) — the same value a
credential with no bound audience is stamped with at ingestion write time
(see [Audience stamping](authorization.md#audience-stamping)). Name the
audience explicitly when minting a key for anything that isn't
deployment-wide-public data, or set `MICROMEGAS_DEFAULT_AUDIENCE` to a label
no principal is granted so an omission fails visibly at read time instead of
publishing. An explicitly supplied but malformed `audience` is still a
**400**. Minting for the resolved audience still requires a matching `mint`
grant (or a lazy claim).

**A non-admin caller naming a brand-new audience explicitly claims it**, once
`MICROMEGAS_SELF_SERVICE_MINT` is on — a genuinely fresh, never-before-granted
name is minted *and* granted in the same request. `micromegas-setup-telemetry`
exposes this via a dedicated `--claim NAME` flag (distinct from `--audience`):
`--claim` claims `NAME` verbatim, with no prefix applied. The script suggests
(but does not enforce) a namespace derived from the caller's own email (e.g.
`--claim alice-ci-runner` for `alice@example.com`); the mint route itself
accepts any valid, unclaimed name from any authorized non-admin caller,
prefixed or not. See [Self-service mint](authorization.md#self-service-ingestion-key-mint)
for the full mechanism.

**An admin minting into a brand-new audience is also claimed server-side**:
the mint route runs the same ownership check for an admin as a pre-check, and
if the audience looks unclaimed, writes the admin's own `user:<email>`
`mint`+`read` rows in the same transaction as the key insert — best-effort,
never a mint failure if a concurrent claim wins the race. `MintResponse.claimed`
is `true` only when this call actually created the audience's first grant
rows. An admin with no email is unaffected — no `user:` row can be formed.

**Data ingested through the env keyring (`MICROMEGAS_API_KEYS`) carries no
bound audience of its own** — that keyring has no audience column — so its
processes are stamped with the deployment's `MICROMEGAS_DEFAULT_AUDIENCE`
(default `public`) explicitly. See [Authorization → Audience
stamping](authorization.md#audience-stamping).

**A hand-edited row takes effect within the key's cache TTL, not instantly**
(`MICROMEGAS_AUTH_CACHE_TTL_SECONDS`, default 60s; see [Cache and audit
env vars](#cache-and-audit-env-vars)) — since the audience is immutable, that
caching is free.

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

A freshly minted analytics key is consumed from Python via
`StaticTokenAuthProvider` — see [Static Analytics API
Keys](../query-guide/python-api.md#static-analytics-api-keys) and the
`api_key_file` profile setting in the [Python API
Reference](../query-guide/python-api.md#config-file-micromegasconfigjson). The
Grafana plugin consumes the same key the same way — see [Grafana
Authentication](../grafana/authentication.md).

Minting an ingestion key uses the same shape against
`/api/ingestion-api-keys` (or the Admin → Ingestion API Keys page), but the
body must supply an `audience`:

```bash
curl -X POST https://analytics.example.com/api/ingestion-api-keys \
  -H 'Content-Type: application/json' -H "Cookie: id_token=$TOKEN" \
  -d '{"name": "grafana-datasource", "audience": "team-alpha"}'
```

Omitting `audience` mints for `MICROMEGAS_DEFAULT_AUDIENCE` (`public` when
unset).

**Revoke** — `DELETE {base_path}/api/{ingestion,analytics}-api-keys/{key_id}`,
keyed only on `key_id`. `GET {base_path}/api/{ingestion,analytics}-api-keys`
is the way to discover a `key_id` to revoke.

## Web app admin pages

Two admin pages, both reachable from **Admin** (`/admin`) in the sidebar:

- **Analytics API Keys** (`/admin/analytics-keys`) — calls
  `analytics-web-srv`'s `/api/analytics-api-keys*` routes directly. Fully
  admin-gated (`AuthGuard requireAdmin`) — analytics keys have no
  self-service story.
- **Ingestion API Keys** (`/admin/ingestion-keys`) — calls
  `analytics-web-srv`'s `/api/ingestion-api-keys*` routes directly. No
  proxy, no forwarding to ingestion, no service credential.

**`/admin` and `/admin/ingestion-keys` are viewable by every authenticated
user, with role-filtered content.** `AuthGuard` on both routes carries no
`requireAdmin`; each branches on the caller's role:

- On `/admin/ingestion-keys`, an admin sees the full list/mint/revoke table
  (`ApiKeysAdminPage`); a non-admin sees a mint-only panel — the same
  self-service mint dialog Audience Access uses, no table, no revoke UI.
  `list_keys`/`revoke_key` stay admin-only server-side. Mint only appears
  once `MICROMEGAS_SELF_SERVICE_MINT` is on and the caller holds a matching
  `mint` grant (or names a fresh audience); with the knob off, the panel
  explains why and points to Audience Access.
- On `/admin`, see [Admin hub](web-app.md#admin-hub) for the role-filtered
  card grid.

Every other admin page under `/admin` — Data Sources, Export Screens, Import
Screens, Maps, Analytics API Keys, Query Deny List — stays fully gated by
`AuthGuard requireAdmin`.

Both ingestion/analytics pages stay visible even when the backing pool isn't
configured; the page surfaces the 503 the route returns, same as `MapsPage`
does for an unconfigured maps store. Neither page has an "import" button —
the import routes exist for the `micromegas-import-keys` CLI only, since a
browser form for pasting a legacy key in would reintroduce the "key transits
a browser" exposure mint already avoids.

**A third page, open to every authenticated user, not just admins**:
**Audience Access** (`/audiences`) is the self-service counterpart of the
ingestion-key mint flow — it drives the mint route's non-admin path
(claim-and-mint) from a browser dialog, plus the audience-grant read/write
routes covered in [Authorization → the grant store](authorization.md#the-grant-store). See
[`web-app.md`](web-app.md#audience-access) for the full page reference.

**`created_by`/`revoked_by` always reflect the acting admin's own OIDC
identity, for both key tables.** Every mint/revoke/import handler resolves
the caller's identity (`user.email` or `user.subject`) and writes that
directly — there is no service-credential hop, so no attribution gap to
document.

**Single admin group, for administration.** List/revoke/import for both
tables, plus ingestion's own mint when the caller is an admin, gate on the
same `analytics-web-srv` admin check (membership in the reserved `admins`
local group — see [Groups](groups.md)). Ingestion's mint route additionally
accepts a non-admin caller once `MICROMEGAS_SELF_SERVICE_MINT` is on,
authorized by a `mint` grant instead of `admins` membership.

**Under `--disable-auth` on `analytics-web-srv`, all three key/grant
route groups are unavailable — not just gated.** With auth disabled, every
request would otherwise be treated as an admin, which would let an
unauthenticated caller mint/revoke real keys or grants; instead all three
path prefixes (`/api/ingestion-api-keys`, `/api/analytics-api-keys`,
`/api/audience-grants`) return a fixed 503 (`{"code": "AUTH_DISABLED", ...}`),
including any sub-path.

## Cache and audit env vars

| Variable | Default | Description |
|---|---|---|
| `MICROMEGAS_API_KEY_CACHE_SIZE` | `10000` | Max distinct live keys cached per process |
| `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` | `60` | **Shared, flat** positive-cache TTL for the API-key, audience-grant, and group stores — also the API-key revocation-latency bound (see [Groups](groups.md) for the group-store latency this same knob governs) |
| `MICROMEGAS_API_KEY_UNKNOWN_CACHE_TTL_SECONDS` | `10` | Negative-cache TTL (shorter, so a freshly minted key isn't masked by an earlier probe) |
| `MICROMEGAS_API_KEY_UNKNOWN_CACHE_SIZE` | `10000` | Max distinct unknown tokens cached per process |

`MICROMEGAS_API_KEY_CACHE_SIZE`/`_UNKNOWN_CACHE_TTL_SECONDS`/`_UNKNOWN_CACHE_SIZE` each accept a
role prefix on the monolith — `MICROMEGAS_INGESTION_API_KEY_CACHE_SIZE` /
`MICROMEGAS_ANALYTICS_API_KEY_CACHE_SIZE`, and so on — falling back to the unprefixed name, the
same convention `MICROMEGAS_API_KEYS` / `MICROMEGAS_OIDC_CONFIG` use.
`MICROMEGAS_AUTH_CACHE_TTL_SECONDS` is the one exception: it is a single flat, unprefixed knob
with no role-scoped variant, since it governs the API-key, audience-grant, and group stores
together as one process-wide value.

Revocation takes effect within `cache_ttl_secs` (default 60s), not instantly
— raising the TTL trades revocation latency for DB load.

## Monitoring a key-store outage

A Postgres outage on the key-lookup path is surfaced as **`503`** (or
`Status::unavailable` on gRPC/FlightSQL) — never `401` — so clients retry
rather than treat it as a rejected credential. Alert on the
`db_api_key_error_count` metric (tagged `{table}`), emitted on every DB error
the key-lookup path hits, independent of the accompanying `error!` log line,
which is rate-limited to at most once per `cache_ttl_secs` (floored at 60s)
window per table.

## Grant recipe (separated DB roles)

By default every service shares one DB role via
`MICROMEGAS_SQL_CONNECTION_STRING`. The table split is still enforced in
code: `analytics-web-srv`'s mint/list/revoke/import routes each hardcode
which table they target, and the ingestion/analytics key-validation
providers are each constructed bound to their own table. Operators who run
separate DB roles per service can additionally enforce the split at the
grant level:

```sql
-- ingestion role: read + touch only (key *validation*, not administration --
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

-- analytics role: read-only on the audience grant store -- DbAudienceGrantsSource
-- re-queries this table on every snapshot refresh
GRANT SELECT ON audience_grants TO micromegas_analytics;

-- analytics-web-srv's own role: the sole admin surface for audience_grants too
GRANT SELECT, INSERT, DELETE ON audience_grants TO micromegas_web;

-- analytics role: read-only on the group store -- DbGroupsSource re-queries
-- groups/group_members on every snapshot refresh
GRANT SELECT ON groups TO micromegas_analytics;
GRANT SELECT ON group_members TO micromegas_analytics;

-- analytics-web-srv's own role: the sole admin surface for groups too
GRANT SELECT, INSERT, DELETE ON groups TO micromegas_web;
GRANT SELECT, INSERT, DELETE ON group_members TO micromegas_web;
```

The last grant is `DELETE`, not `UPDATE`: `audience_grants`/`group_members`
rows are hard-deleted on revocation (`groups` rows too, on delete).
`micromegas_web` (`analytics-web-srv`'s role) is the only role granted
`INSERT` on any of these tables in a fully separated-role deployment;
`micromegas_ingestion` is read + `last_used_at` touch only on its own table;
`micromegas_analytics` is read + `last_used_at` touch on `analytics_api_keys`,
plus read-only on `audience_grants`/`groups`/`group_members`. Neither service
role has any grant on the other service's table. `analytics-web-srv`'s role
does gain write access to `ingestion_api_keys` under this design, since every
ingestion-key mint/revoke/import goes through it.

## Migrating from the env keyring

Carrying *existing* key strings forward is an HTTP-backed operation via the
`import` route on `analytics-web-srv` for each table, and the
`micromegas-import-keys` CLI tool that drives it — no `psql`, no direct
Postgres network access needed.

1. **Deploy the new binaries.** The migration creates the tables (schema v5).
   Nothing changes yet: the env keyring still authenticates every existing
   key, and the DB tables start empty. In a split deployment, start ingestion
   (or the monolith) before flight-sql — flight-sql never runs the
   migration. Violating this ordering surfaces as a `warn!` log line naming
   the table (flight-sql still starts, since the env keyring or OIDC is
   still authenticating). Once `MICROMEGAS_API_KEYS` is removed (step 3) and
   OIDC isn't configured either, a schema still short of v5 makes flight-sql
   **fail to start**. For these key-management routes specifically,
   `analytics-web-srv` never runs this migration itself — the target
   telemetry DB must already have had ingestion or a lakehouse-role monolith
   run against it at least once.
2. **Populate the tables with `micromegas-import-keys`** — installed
   alongside `micromegas-query`/`-screens`/`-logout` (`pip install
   micromegas`, or via poetry in `python/micromegas`):

   ```bash
   micromegas-import-keys --table ingestion --source env \
     --url https://analytics.example.com
   micromegas-import-keys --table analytics --source env \
     --url https://analytics.example.com
   ```

   `--table` selects the import route; `--url` always points at
   `analytics-web-srv`'s base URL, for both tables. With no explicit `--var`,
   `--source env` tries the table's own prefixed legacy var first
   (`MICROMEGAS_INGESTION_API_KEYS` / `MICROMEGAS_ANALYTICS_API_KEYS`) and
   falls back to the unprefixed `MICROMEGAS_API_KEYS`. Pass `--var NAME`
   explicitly to pin an exact source var, or `--source file --path ...` to
   read the legacy keyring's shape — a JSON array of `{"name", "key"}`
   objects, each optionally carrying an `"audience"` field too (ingestion
   only) — from a file instead. `--audience AUD` sets the audience for every
   ingestion key imported in this run (valid only with `--table ingestion`; a
   per-entry `"audience"` in the keyring wins over it). This is a different
   flag than `MICROMEGAS_OIDC_AUDIENCE` used for this same tool's token
   validation — the name coincidence is unrelated. Neither given, the server
   applies `MICROMEGAS_DEFAULT_AUDIENCE` (`public` when unset). Auth follows
   the same OIDC setup as `micromegas-screens`/`-query`
   (`MICROMEGAS_OIDC_*` env vars for a service account, or an
   interactive/cached login via `--profile`); the OIDC identity used must be
   in the target service's admin list. The tool prints one line per key
   (`imported` / `already present (key_id=...)` / `already present
   (revoked)` / the error message), continues past individual failures, and
   exits non-zero if any key failed to import or came back revoked. Route
   each key explicitly with `--only NAME [NAME ...]` / `--exclude NAME [NAME
   ...]` (mutually exclusive):
   - An `object-cache-srv` client key stays env-only forever — never
     imported.
   - A key that is also a service's own ingestion self-telemetry credential
     (`MICROMEGAS_INGESTION_API_KEY`) goes into `ingestion_api_keys`.
   - **A key valid on both ingestion and flight-sql today must become two
     distinct key strings, one per table** — split it first, then import
     each half with `--only`/`--exclude` selecting disjoint entries per
     table run.
3. **Remove `MICROMEGAS_API_KEYS`** (and prefixed variants) from ingestion
   and flight-sql once the tables are populated. A non-empty key store counts
   as "auth configured" on its own, so both services keep serving without
   OIDC — including a key-only flight-sql deployment (see [Grafana
   Authentication](../grafana/authentication.md)). `object-cache-srv` keeps
   `MICROMEGAS_API_KEYS` **permanently** — see [Object Cache](object-cache.md).

## Security

- **API keys can never manage keys**: `is_admin` is hardcoded `false` on
  every API-key auth context, and `analytics-web-srv`'s `AdminUser`
  extractor rejects any caller whose `is_admin` isn't `true` before a
  list/revoke/import handler runs. There is no bearer-key authenticator on
  `analytics-web-srv`'s `/api/*` routes at all, so an ingestion API key has
  no code path to reach the mint route's extractors, admin or not.
  Self-service mint only ever widens *which browser-authenticated OIDC
  caller* may mint — never what kind of credential can.
- **No cleartext at rest.** Only a SHA-256 hash is stored; the cleartext key
  exists only in the one-time `POST` response and in the client's own
  storage.
- **No timing side channel** on the DB path — the token is hashed and the
  hash used as an index key, so lookup time doesn't depend on how many
  leading bytes of a guess are correct.
- **Revocation latency is bounded, not zero** — see [Cache and audit env
  vars](#cache-and-audit-env-vars).
- **A key-store outage is never client-visible as a rejected credential** —
  see [Monitoring a key-store outage](#monitoring-a-key-store-outage).
