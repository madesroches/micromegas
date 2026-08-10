# API Keys

Micromegas keys can live in two Postgres tables — `ingestion_api_keys` and
`analytics_api_keys` — instead of `MICROMEGAS_API_KEYS` (a plaintext JSON env
var, parsed once at startup). The tables hold only a SHA-256 hash of each key
plus a `created_at`/`created_by`/`last_used_at`/`revoked_at`/`revoked_by` audit
trail. Four OIDC-authenticated, admin-gated HTTP routes on the **ingestion**
service let an operator mint, list, revoke, and import `ingestion_api_keys`
without a redeploy; the same four operations for `analytics_api_keys` are
exposed by **`analytics-web-srv`**'s own routes instead (`/api/analytics-api-keys*`),
since issuing read credentials from the fleet-facing ingestion service would
be the wrong direction for the write/read asymmetry this design is built on.
Both key tables also have an admin page in the web app (Admin → Ingestion API
Keys / Analytics API Keys) — the ingestion-key page is a server-side proxy in
front of ingestion's routes, since the browser has no bearer token to call
ingestion directly with (see [Web app admin pages](#web-app-admin-pages)).

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
    trusted local network**, or the cleartext key is exposed in flight.
    The proxied ingestion mint/import routes (browser → `analytics-web-srv` →
    ingestion) add a second hop the cleartext crosses on top of the
    browser-facing leg: `MICROMEGAS_INGESTION_ADMIN_URL` should be `https://`,
    or the link between `analytics-web-srv` and ingestion confined to a
    trusted network (e.g. a private subnet or service mesh).

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
  revoked_by   VARCHAR(255)
);
CREATE UNIQUE INDEX ingestion_api_keys_key_hash ON ingestion_api_keys(key_hash);

-- analytics_api_keys: identical shape; never gains an audience column
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

`key_id` is a UUID handle, distinct from `key_hash`: `DELETE /auth/api_keys/<id>`
needs something to key on, and `GET` must never hand out `key_hash` (there's no
reason to distribute the lookup value even though it isn't reversible).
`name` carries no uniqueness constraint — rotating a key under a stable name
means two live rows can legitimately share a `name` while the old one is being
phased out. **Every revoke path keys on `key_id`, never `name`.**

There is no cleartext column, and SHA-256 with no KDF is safe *only* because
these are high-entropy random keys, not passwords — rotate any imported legacy
key that wasn't actually random.

## HTTP routes (ingestion keys)

All four routes below live on the **ingestion** service, gated by
`require_key_admin`: the caller must be an OIDC identity (never an API key —
`is_admin` is hardcoded `false` on every API-key context) and must be in the
configured admin list (`MICROMEGAS_ADMINS`, or `MICROMEGAS_INGESTION_ADMINS`
on the monolith). The equivalent routes for `analytics_api_keys` live on
`analytics-web-srv` instead — see
[HTTP routes (analytics keys)](#http-routes-analytics-keys).

### `POST /auth/api_keys`

```json
// Request
{"name": "game-client-42"}
```

```json
// 201 Created
{
  "key_id": "b3f6...-uuid",
  "name": "game-client-42",
  "created_at": "2026-01-01T00:00:00Z",
  "key": "mmk_2f8c...base64url..."
}
```

The `key` field is the cleartext key, returned **exactly once** — it is never
logged (only `key_id` is) and never retrievable afterwards. `mmk_` marks the
key as a Micromegas secret for scanners; validation covers the whole string via
its hash, so imported legacy keys of any shape keep working.

**400** if `name` is empty or exceeds 255 bytes (stricter than the
`VARCHAR(255)` column, which bounds characters, not bytes).

**Analytics keys are never minted through this route.** Minting an analytics
key happens through `analytics-web-srv`'s own routes instead — see
[Minting an analytics key over HTTP](#minting-an-analytics-key-over-http).

### `GET /auth/api_keys?limit=&offset=&include_revoked=`

```json
// 200 OK
[
  {
    "key_id": "b3f6...-uuid",
    "name": "game-client-42",
    "created_at": "2026-01-01T00:00:00Z",
    "created_by": "alice@example.com",
    "last_used_at": "2026-01-02T03:04:05Z",
    "revoked_at": null,
    "revoked_by": null
  }
]
```

Newest first. `limit` defaults to `100`; values above `500` are silently
clamped to `500` (a read endpoint, so capping is safer than rejecting);
`limit <= 0` is **400**. `offset` defaults to `0`. `include_revoked` defaults
to `true` — an operator investigating an incident needs to see that a key
*was* revoked and when. **Never `key_hash`, never the key.**

### `DELETE /auth/api_keys/{key_id}`

```json
// 200 OK
{"revoked_at": "2026-01-03T00:00:00Z", "effective_within_seconds": 60}
```

Idempotent: a second `DELETE` returns the same `revoked_at` rather than
overwriting it. **404** for an unknown `key_id`.

`effective_within_seconds` reports this process's configured cache TTL (see
[Cache and audit env vars](#cache-and-audit-env-vars)) — a revoked key can keep
authenticating on other processes for up to that long, since revocation writes
`revoked_at` in the DB but cannot reach into a remote process's cache. A fleet
with mixed configuration takes the longest configured TTL to fully revoke a
key everywhere.

### `POST /auth/api_keys/import`

The one gap `mint_key` doesn't cover: carrying a *pre-existing* key string
forward, rather than generating a fresh one. This is what lets an existing
client keep presenting the same key string after migrating off the env
keyring — see [Migrating from the env keyring](#migrating-from-the-env-keyring).

```json
// Request
{"name": "legacy-game-client", "key": "<the existing key string, verbatim>"}
```

```json
// 201 Created — fresh insert
{
  "key_id": "b3f6...-uuid",
  "name": "legacy-game-client",
  "created_at": "2026-01-01T00:00:00Z",
  "created_by": "alice@example.com",
  "revoked_at": null,
  "imported": true
}
```

`imported: true` and **201** on a fresh insert; `imported: false` and **200**
when the hash already exists (idempotent re-run — importing the same legacy
keyring twice has no side effects). `revoked_at` is always present (`null`
unless the existing row was itself revoked), so a caller can distinguish
"already present and usable" from "already present but revoked".
`created_by` is the importing caller's own OIDC identity (`email` or
`subject`, the same resolution `mint_key` uses) — never the literal string
`"import"`.

**400** if `name` is empty/too long (same rule as mint) or `key` is empty. No
format validation on `key` beyond non-empty — `hash_key` covers the whole
string regardless of shape, which is what lets a legacy key of any format
(including one that never had the `mmk_` prefix) import cleanly. Never
logs the key, same as `mint_key`.

The `micromegas-import-keys` CLI tool (see
[Migrating from the env keyring](#migrating-from-the-env-keyring)) is the
recommended way to call this route in bulk; it can also be called directly.

## HTTP routes (analytics keys)

`analytics_api_keys` has its own mint/list/revoke/import routes, hosted on
**`analytics-web-srv`** rather than ingestion — issuing read credentials from
the fleet-facing ingestion service would be the wrong direction for the
write/read asymmetry this design is built on. Routes live under
`{base_path}/api/analytics-api-keys`, gated by the same cookie/bearer-auth
admin check every other `analytics-web-srv` admin route uses (`ValidatedUser.is_admin`,
resolved from `MICROMEGAS_ADMINS` or `MICROMEGAS_ANALYTICS_ADMINS` on the
monolith — see [Authentication](authentication.md)):

| Route | Body / result |
|---|---|
| `POST {base_path}/api/analytics-api-keys` | `{"name"}` → 201 `{"key_id","name","created_at","key"}` |
| `GET {base_path}/api/analytics-api-keys?limit=&offset=&include_revoked=` | 200 `[{"key_id","name","created_at","created_by","last_used_at","revoked_at","revoked_by"}]` |
| `DELETE {base_path}/api/analytics-api-keys/{key_id}` | 200 `{"revoked_at"}` or 404 |
| `POST {base_path}/api/analytics-api-keys/import` | `{"name","key"}` → 201/200 `{"key_id","name","created_at","created_by","revoked_at","imported"}` |

Same validation and semantics as the ingestion routes above (400 on an empty/
oversized `name` or empty `key`, idempotent revoke, idempotent import). One
difference: the revoke response has no `effective_within_seconds` field —
`analytics-web-srv` runs no `DbApiKeyAuthProvider` of its own, so there's no
running cache TTL on *this* process to report; the revocation latency is
still bounded by whichever `flight-sql` process's `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS`
is validating the key.

**Precondition: the telemetry DB must already have the v5 migration**
(`analytics_api_keys`), which only ingestion or a lakehouse-role monolith
runs — a standalone `analytics-web-srv` or a `--roles web`-only monolith never
runs it themselves. Run ingestion (or a lakehouse-role monolith) against the
target telemetry DB at least once before relying on these routes, or every
call fails at request time with an opaque `500`.

**New env vars, both optional (503 when unset — see below):**

| Variable | Description |
|---|---|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Telemetry-DB connection string `analytics-web-srv` opens its own small (`max_connections(2)`) pool from, backing the analytics-key routes. `None` (routes 503) if unset. |
| `MICROMEGAS_INGESTION_PROXY_OIDC_CLIENT_ID` / `_CLIENT_SECRET` / `_TOKEN_ENDPOINT` / `_AUDIENCE` (optional) | Service credential the ingestion-key proxy (below) authenticates to ingestion with. |
| `MICROMEGAS_INGESTION_ADMIN_URL` | Ingestion's base URL the proxy forwards to, e.g. `http://127.0.0.1:8081` for the monolith. |

When `MICROMEGAS_SQL_CONNECTION_STRING` is unset, the analytics-key routes
stay registered and return **503** — same "always registered, 503 when
unconfigured" shape as `/api/maps/*`.

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

**Revoke** — `DELETE {base_path}/api/analytics-api-keys/{key_id}`, keyed
**only** on `key_id` (`name` carries no uniqueness constraint — a by-name
revoke during rotation could revoke the freshly minted replacement along with
the row being retired). `GET {base_path}/api/analytics-api-keys` is now the
way to discover a `key_id` to revoke — there is no more "hand-SQL only" list
step.

## Web app admin pages

Two admin pages in the web app, both under **Admin** in the sidebar, and both
requiring an OIDC admin session (`AuthGuard requireAdmin`):

- **Analytics API Keys** (`/admin/analytics-keys`) — calls
  `analytics-web-srv`'s own routes directly (above).
- **Ingestion API Keys** (`/admin/ingestion-keys`) — calls a server-side proxy
  on `analytics-web-srv` (`/api/ingestion-api-keys*`), which forwards to
  ingestion's `/auth/api_keys*` routes under `analytics-web-srv`'s own service
  credential (`MICROMEGAS_INGESTION_PROXY_OIDC_*` above). A proxy is
  necessary here because the browser has no bearer token to call ingestion
  directly with: the `id_token` cookie is `http_only`.

Both pages stay visible even when their backing pool/proxy isn't configured;
in that case the page surfaces the 503 the route returns, same as `MapsPage`
does for an unconfigured maps store. Neither page has an "import" button —
the import routes exist for the `micromegas-import-keys` CLI tool only, since
a browser form for pasting a legacy key in would reintroduce the
"key transits a browser" exposure mint already avoids.

**Enabling the ingestion-key proxy makes `analytics-web-srv`'s admin list a
de-facto ingestion-key-admin list.** The proxy gates only on its own admin
check and then forwards under its privileged service credential — anyone in
`analytics-web-srv`'s admin list gets mint/list/revoke on `ingestion_api_keys`
even if deliberately excluded from `MICROMEGAS_INGESTION_ADMINS`. Operators
who intentionally keep the two admin lists separate must either keep them
aligned or leave the proxy unconfigured.

**Known limitation: every ingestion key minted or revoked through the web UI
is attributed to the proxy's own service credential, not the admin who
performed it.** Ingestion has no way to see which admin is behind a proxied
call — it only ever authenticates the proxy's own
`MICROMEGAS_INGESTION_PROXY_OIDC_*` identity — so `created_by`/`revoked_by`
on every proxied row records that service identity. Minting/revoking directly
against ingestion's own routes (not through the proxy) still attributes
correctly to the caller's own OIDC identity, as does every analytics-key
route on `analytics-web-srv` (§ above), since neither goes through a
service-credential hop. Accepted for now: closing this gap would mean
ingestion trusting a caller-supplied "acting on behalf of" identity, which
only helps if it's restricted to this proxy specifically — a second
admin-adjacent allowlist to provision and keep in sync, to close a narrow
accountability gap that only exists between admins who are already equally
privileged. Revisit if per-admin attribution through the proxy becomes an
operational need.

**Under `--disable-auth` on `analytics-web-srv`, both key-management route
groups are unavailable — not just gated, but not merged at all.** With auth
disabled, every request would otherwise be treated as an admin, which would
let an unauthenticated caller mint/revoke real keys through the proxy's
privileged credential; instead both path prefixes are answered by a fixed
503 (`{"code": "AUTH_DISABLED", ...}`), including any sub-path.

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
What ships today is a *code*-level boundary: the mint/list/revoke routes
hardcode `ingestion_api_keys`, and the analytics provider is constructed bound
to `analytics_api_keys`, with no parameter either could point at the other
table. Operators who do separate DB roles per service can additionally enforce
the split at the grant level:

```sql
-- ingestion role: its own table only
GRANT SELECT, INSERT ON ingestion_api_keys TO micromegas_ingestion;
GRANT UPDATE (last_used_at, revoked_at, revoked_by) ON ingestion_api_keys TO micromegas_ingestion;
-- and no grant of any kind on analytics_api_keys

-- analytics role: read + touch only (flight-sql's own key-validation identity)
GRANT SELECT ON analytics_api_keys TO micromegas_analytics;
GRANT UPDATE (last_used_at) ON analytics_api_keys TO micromegas_analytics;
-- and no grant of any kind on ingestion_api_keys

-- analytics-web-srv's own role: write + touch only, on analytics_api_keys alone
GRANT SELECT, INSERT ON analytics_api_keys TO micromegas_web;
GRANT UPDATE (revoked_at, revoked_by) ON analytics_api_keys TO micromegas_web;
-- and no grant of any kind on ingestion_api_keys
```

Note this is **two** distinct roles both writing `analytics_api_keys` in a
fully separated-role deployment — `micromegas_web` (mint/import/revoke) and
`micromegas_analytics` (read + `last_used_at` touch) — with no overlap in
their column grants. `analytics-web-srv` still never gains write access to
`ingestion_api_keys`: all ingestion writes go through ingestion's own HTTP
API, exactly the asymmetry this design is built to preserve.

## Migrating from the env keyring

Carrying *existing* key strings forward is now an HTTP-backed operation via
the `import` route on each service (above) and the `micromegas-import-keys`
CLI tool that drives it — no `psql`, no direct Postgres network access, an
HTTP-reachable workstation is enough. The tables and the mint/list/revoke
APIs stand on their own without the import tool; it only affects carrying
*existing* key strings forward.

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
   outage. For the analytics-key routes specifically, `analytics-web-srv`
   never runs this migration itself either — the target telemetry DB must
   already have had ingestion or a lakehouse-role monolith run against it at
   least once (see [HTTP routes (analytics keys)](#http-routes-analytics-keys)).
2. **Populate the tables with `micromegas-import-keys`** — installed
   alongside `micromegas-query`/`-screens`/`-logout` (`pip install micromegas`,
   or via poetry in `python/micromegas`):

   ```bash
   micromegas-import-keys --table ingestion --source env --var MICROMEGAS_API_KEYS \
     --url http://ingestion:8081
   micromegas-import-keys --table analytics --source env --var MICROMEGAS_ANALYTICS_API_KEYS \
     --url https://analytics.example.com
   ```

   `--table` selects the import route; `--url` points directly at the target
   service (ingestion's own base URL for `--table ingestion` — **not**
   through `analytics-web-srv`'s proxy, which exists only for the browser).
   `--source env --var NAME` (or `--source file --path ...`) reads the legacy
   keyring's real shape — a JSON array of `{"name", "key"}` objects. Auth
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
  API-key context, and the route gate rejects any non-OIDC `auth_type` outright
  — two independent mechanisms, not one.
- **No cleartext at rest.** Only a SHA-256 hash is stored; the cleartext key
  exists only in the one-time `POST` response and in the client's own storage.
- **No timing side channel** on the DB path — the token is hashed and the hash
  used as an index key, so lookup time doesn't depend on how many leading
  bytes of a guess are correct.
- **Revocation latency is bounded, not zero** — see
  [Cache and audit env vars](#cache-and-audit-env-vars).
- **A key-store outage is never client-visible as a rejected credential** — see
  [Monitoring a key-store outage](#monitoring-a-key-store-outage).
