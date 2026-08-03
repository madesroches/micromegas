# API Keys

Micromegas keys have moved out of `MICROMEGAS_API_KEYS` (a plaintext JSON env
var, parsed once at startup) and into two Postgres tables — `ingestion_api_keys`
and `analytics_api_keys` — holding only a SHA-256 hash of each key plus a
`created_at`/`created_by`/`last_used_at`/`revoked_at`/`revoked_by` audit trail.
Three OIDC-authenticated, admin-gated HTTP routes on the ingestion service let
an operator mint, list, and revoke keys without a redeploy.

!!! warning "TLS is a prerequisite for minting"
    `POST /auth/api_keys` returns the cleartext key exactly once, over whatever
    transport the request arrives on. The ingestion service itself serves plain
    HTTP — there is no rustls/TLS acceptor anywhere in the service. **Put a
    TLS-terminating ingress in front of the ingestion service before calling
    this route in anything but a fully trusted local network**, or the
    one-time cleartext key is exposed in flight.

## Why two tables

The security model is asymmetric: a stolen write (ingestion) key is an
integrity problem; a stolen read (analytics) credential is a confidentiality
one. A shared table with a `scopes` column would make every key a potential
read credential. With "never both" as a rule, the two tables are kept
separate — one behavior change this implies: **a key valid on both ingestion
and flight-sql today must become two distinct keys** once you migrate off the
env keyring (see [Migration from the env keyring](#migration-from-the-env-keyring)).
The code (not just the schema) enforces the split — see [Security](#security).

## Schema

```sql
CREATE TABLE ingestion_api_keys (
  key_id       UUID PRIMARY KEY,
  key_hash     BYTEA NOT NULL,          -- sha256 of the full key string, 32 bytes
  name         VARCHAR(255) NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL,
  created_by   VARCHAR(255) NOT NULL,   -- OIDC email/subject of the minter, or 'import'
  last_used_at TIMESTAMPTZ,
  revoked_at   TIMESTAMPTZ,
  revoked_by   VARCHAR(255)
);
CREATE UNIQUE INDEX ingestion_api_keys_key_hash ON ingestion_api_keys(key_hash);

-- analytics_api_keys: identical shape; never gains an audience column
CREATE TABLE analytics_api_keys (LIKE ingestion_api_keys INCLUDING ALL);
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

## HTTP routes

All three routes live on the **ingestion** service, gated by `require_key_admin`:
the caller must be an OIDC identity (never an API key — `is_admin` is hardcoded
`false` on every API-key context) and must be in the configured admin list
(`MICROMEGAS_ADMINS`, or `MICROMEGAS_INGESTION_ADMINS` on the monolith).

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

**Analytics keys are not mintable through this route or any other HTTP path.**
They are few, manually issued, and stay out of every ingestion-service write
path — see [Minting an analytics key by hand](#minting-an-analytics-key-by-hand).

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
`cache_ttl_secs` window per table to avoid flooding `log_entries` with the
outage's own noise during a sustained outage.

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

-- analytics role: read + touch only
GRANT SELECT ON analytics_api_keys TO micromegas_analytics;
GRANT UPDATE (last_used_at) ON analytics_api_keys TO micromegas_analytics;
-- and no grant of any kind on ingestion_api_keys
```

## Migration from the env keyring

Importing *existing* key strings is a manual, hand-written-SQL operation today
— a proper HTTP-API-backed import tool (and a web admin UI for key management)
is tracked separately; until it lands, follow the recipe below. This plan's
tables and the ingestion mint/list/revoke API stand on their own without an
import tool — it only affects carrying *existing* key strings forward.

1. **Deploy the new binaries.** The migration creates the tables (schema v5).
   Nothing changes yet: the env keyring still authenticates every existing key,
   and the DB tables start empty. In a split deployment, start ingestion (or
   the monolith) before flight-sql — flight-sql never runs the migration itself,
   and its own startup check fails loudly, naming the table, if this ordering
   is violated.
2. **Populate the tables** — one hand-written `INSERT ... ON CONFLICT (key_hash)
   DO NOTHING` per key (see [Minting an analytics key by hand](#minting-an-analytics-key-by-hand)
   for the exact recipe; the ingestion-table shape is identical). Route each
   key explicitly:
   - An `object-cache-srv` client key stays env-only forever — never imported.
   - A key that is also a service's own ingestion self-telemetry credential
     (`MICROMEGAS_INGESTION_API_KEY`) goes into `ingestion_api_keys`.
   - **A key valid on both ingestion and flight-sql today must become two
     distinct key strings, one per table.** This is the one client-visible
     change this migration doesn't avoid.
3. **Remove `MICROMEGAS_API_KEYS`** (and prefixed variants) from ingestion and
   flight-sql once the tables are populated. A non-empty key store counts as
   "auth configured" on its own, so both services keep serving without OIDC —
   including a key-only flight-sql deployment (see
   [Grafana Authentication](../grafana/authentication.md)). `object-cache-srv`
   keeps `MICROMEGAS_API_KEYS` **permanently** — see
   [Object Cache](object-cache.md).

## Minting an analytics key by hand

`analytics_api_keys` has no HTTP lifecycle of its own — issuing read
credentials from the ingestion service would be the wrong direction for the
write/read asymmetry this design is built on. Manage it directly:

**List** (the only way to discover a `key_id` to revoke — there is no HTTP `GET`):

```sql
SELECT key_id, name, created_at, last_used_at, revoked_at
FROM analytics_api_keys
ORDER BY created_at DESC;
```

**Mint:**

```bash
KEY="mmk_$(openssl rand -base64 48 | tr -d '=+/\n' | head -c 43)"
HASH=$(printf '%s' "$KEY" | sha256sum | cut -d' ' -f1)
ID=$(uuidgen)
```

Use `printf`, not `echo` — `hash_key` covers the full key string, and `echo`
would append a trailing newline the validator never sees, silently minting a
key that can never authenticate. Use `uuidgen`, not `gen_random_uuid()` — the
table has no `DEFAULT` on `key_id`, so this recipe carries no minimum-Postgres-
version requirement.

```sql
INSERT INTO analytics_api_keys (key_id, key_hash, name, created_at, created_by)
VALUES ('$ID', decode('$HASH', 'hex'), '<name>', now(), '<operator>')
ON CONFLICT (key_hash) DO NOTHING;
```

**Revoke** — the same statement the `DELETE` route runs, keyed **only** on the
`key_id` from the list step above, never on `name` (which carries no
uniqueness constraint — a by-name revoke during rotation could revoke the
freshly minted replacement along with the row being retired):

```sql
UPDATE analytics_api_keys
SET revoked_at = COALESCE(revoked_at, now()),
    revoked_by = COALESCE(revoked_by, '<operator>')
WHERE key_id = '<key_id>'
RETURNING revoked_at;
```

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
