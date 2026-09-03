# Authorization

[Authentication](authentication.md) establishes *who* is calling. Authorization
decides *what* they may read and write.

Every telemetry row carries an opaque label — its **audience** — stamped
server-side from the credential that wrote it. A separate, editable **grant
map** says which principals may read from, and mint keys into, each audience.
Queries are filtered to the caller's own audiences.

Filtering activates whenever authentication does: an authenticated session gets
`ReadScope::Audiences`, a `--disable-auth` one keeps `ReadScope::All` and reads
everything. There is no separate switch — what an identity-less caller sees is
shaped by `MICROMEGAS_DEFAULT_AUDIENCE` and the grant map.

## Configuration

| Variable | Default | Description |
|---|---|---|
| `MICROMEGAS_AUDIENCE_GRANTS` | unset | JSON grant map, read once at startup by FlightSQL. Read axis only — mint grants must be in the DB. The monolith prefers `MICROMEGAS_ANALYTICS_AUDIENCE_GRANTS` when set, falling back to this. |
| `MICROMEGAS_DEFAULT_AUDIENCE` | `public` | Label stamped on rows whose credential carries no bound audience. Set it identically on **every** role that builds a lakehouse — FlightSQL, maintenance, monolith, **and ingestion**. |
| `MICROMEGAS_PUBLIC_VIEW_SETS` | unset | Comma-separated view sets exempt from filtering entirely; an operator-responsibility allowlist. |
| `MICROMEGAS_SELF_SERVICE_MINT` | `false` | Lets a non-admin mint their own ingestion key and manage grants. See [Self-service mint](#self-service-ingestion-key-mint). |
| `MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER` | `25` | Distinct audiences one non-admin may claim. Best-effort under concurrency. |
| `MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER` | `100` | Live keys one non-admin may hold. `list_keys`/`revoke_key` stay admin-only, so freeing a slot needs an admin. |
| `MICROMEGAS_SELF_SERVICE_MAX_GRANTS_PER_CALLER` | `50` | Rows one non-admin may have created in `audience_grants`, excluding their own `user:<email>` rows. Best-effort. |
| `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` | `60` | Snapshot lifetime for the grant, API-key, and group stores. Flat and unprefixed — no role-scoped form, including on the monolith. |

## Audiences and Grants

An audience is an opaque label on data — `public`, `team-alpha`,
`payments-svc` — not an encoding of any principal's identity. Who may use it is
separate configuration:

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

Keys are audience names: `[A-Za-z0-9_-]{1,255}`, case-sensitive, no
normalization. A value is either a bare array (read-only shorthand) or an
object with separate `"read"`/`"mint"` lists. An omitted `"mint"` list is
empty, never derived from `"read"`.

| Selector | Matches |
|---|---|
| `*` | any authenticated principal |
| `user:<email>` | the caller's `email` claim |
| `group:<g>` | members of local group `g`, transitively (see [Groups](groups.md)) |

- **No self-audience rule.** A caller is never granted an audience for being
  named like one — an API key named `team-alpha` does not read `team-alpha`. A
  personal audience is an ordinary audience with an ordinary grant.
- **Re-sharing is a grants edit, never a restamp.** A stamped audience value
  never changes; widening `team-alpha`'s `"read"` list applies to
  already-ingested data immediately, bounded by the cache TTL.
- **A malformed map fails startup**: unknown-shaped key, unrecognized selector
  prefix, or a duplicate JSON key for one audience.
- Users see their own grants, and share/mint, from the Audience Access page
  (`/audiences`, open to every authenticated user — see
  [`web-app.md`](web-app.md#audience-access)) or via
  [`list_audience_grants()`](#list_audience_grants).

### `public`

`public` has no built-in read grant. A fresh deployment's DB grant store ships
with `('public', 'read', '*')` inserted, which is the whole of why it is
universally readable; delete the row and it stops being. Writing `{"public":
["*"]}` in the env map adds nothing on top.

A caller with no identity of its own — an API key, or an OIDC token with no
`email` claim — matches that `*` selector like any other principal.
`MICROMEGAS_DEFAULT_AUDIENCE`'s default of `public` is the write-side half: it
is what puts such a caller's data under that same label.

### Worked profiles

```bash
# Open: everyone reads everything. No grant map needed; the default is public.

# Privacy: point the default at a label nobody is granted, so anything that
# omits an audience is invisible rather than published, and name the audience
# explicitly on every key you mint.
export MICROMEGAS_AUDIENCE_GRANTS='{"team-alpha": ["group:eng"]}'
export MICROMEGAS_DEFAULT_AUDIENCE=unassigned

# Personal audience with mint authority (read is a separate grant):
micromegas-grants --url https://analytics.example.com create alice-laptop mint user:alice@example.com
micromegas-grants --url https://analytics.example.com create alice-laptop read user:alice@example.com
```

!!! warning "Env-map audiences are invisible to the lazy claim"
    The self-service claim's existence check reads only the DB
    (`audience_grants`, `ingestion_api_keys`), never the env map. Before
    enabling `MICROMEGAS_SELF_SERVICE_MINT`, create a placeholder DB row for
    every audience that exists only in `MICROMEGAS_AUDIENCE_GRANTS`, plus a
    custom `MICROMEGAS_DEFAULT_AUDIENCE` — nothing seeds these, and a
    non-admin could otherwise claim exclusive rights over the name. Any axis,
    any selector; prefer an identity selector over `'*'`.

    ```bash
    micromegas-grants --url https://analytics.example.com create team-alpha read 'group:eng'
    ```

## Audience stamping {#audience-stamping}

`processes`, `streams`, and `blocks` each carry their own `audience` column,
written from the authenticated ingestion credential — never trusted from the
client payload.

- Ingestion strips any client-supplied `micromegas.*` property and stamps at
  insert time. Each row's stamp is the credential that wrote *that* row, never
  derived from the `process_id`/`stream_id` it points at.
- A DB-backed `ingestion_api_keys` row's bound audience is stamped as-is. A
  credential with none — env-keyring key, OIDC token, or no auth provider — is
  stamped with `MICROMEGAS_DEFAULT_AUDIENCE`.
- **Client self-stamping has no effect.** To get its own label a producer needs
  a DB ingestion key bound to that audience.
- `log_entries`, `measures`, and `log_stats` inherit the owning block's stamp.
- A stamped audience is immutable: there is no `UPDATE processes` path, and
  changing `MICROMEGAS_DEFAULT_AUDIENCE` affects only rows written afterwards.

**OTLP `process_id` is audience-scoped**, so two audiences posting identical
resource attributes never collapse onto one process. Each audience gets its own
id namespace and the deployment default owns the un-salted one, which has two
consequences:

- A key explicitly bound to a label *equal to* the default moves into the
  un-salted namespace, appearing as a new process row; its earlier data keeps
  the old id. Rotating a key to a different audience splits a producer's
  history across two ids the same way.
- Flipping the default can leave unaudienced traffic presenting an un-salted
  `process_id` against rows stamped under the old label, which registration
  rejects (below) until those rows age out.

### Registration conflicts

`insert_process`/`register_otel_process` reject a same-`process_id`,
different-audience re-registration with a 403, and
`insert_stream`/`register_otel_stream` do the same per `stream_id`. Clearing a
wrongly-registered id means deleting the row by hand (`DELETE FROM processes
WHERE process_id = ...`); `delete_empty_processes` only reclaims it once it has
no streams and retention has elapsed.

## Query-time audience filtering {#query-time-audience-filtering}

Two layers narrow reads under a `ReadScope::Audiences` session.

**Row-level** (`OwnershipRewrite`) injects an audience predicate into every
`MaterializedView`-backed plan, so a caller sees only rows whose own `audience`
column resolves to one of their audiences — `processes`, `streams`, `blocks`,
`log_entries`, `measures`, `log_stats`.

**Call-level** (`AudienceGuard`) covers five arg-addressed functions:
`view_instance`, `process_spans`, `perfetto_trace_chunks`, `parse_block`,
`get_payload`. A call fails with a not-found-shaped error unless its id argument
names a process or stream in one of the caller's audiences. `list_partitions()`
silently omits every row that isn't theirs, `'global'` rows included.

For `net_spans`, `otel_spans`, `images`, `async_events`, and `thread_spans` —
no `audience` column, reachable only through `view_instance(...)` — call-level
is the *only* enforcement. Elsewhere row-level already covers the scan.

Two exemptions from `view_instance`'s guard:

- `'global'` instances, which have nothing to materialize and whose rows
  row-level filtering covers one at a time. This is a different rule from
  `list_partitions()`'s `'global'`-row visibility, which gates partition
  *metadata* rather than the rows of the file.
- View sets on `MICROMEGAS_PUBLIC_VIEW_SETS`, every row of which is already
  readable.

**Freshness differs between the layers.** Row-level reads the
daemon-materialized parquet snapshot, so a process the maintenance role hasn't
caught up on is invisible to everyone, its owner included. Call-level reads
Postgres, so it is fresher for a just-ingested process but denies once retention
has deleted the Postgres row, even if a compacted partition of its data
survives. Both resolve a row's stamp identically — the only skew is this timing.

## Admin-gated lakehouse functions {#admin-gated-lakehouse-functions}

Eight functions are gated on admin-ness rather than audience:
`retire_partitions`, `materialize_partitions`, `regenerate_partitions`,
`retire_partition_by_file`, `retire_partition_by_metadata`, and the [query deny
list](functions-reference.md#query-deny-list)'s `list_query_denials`,
`deny_queries`, `remove_query_denial`. A non-admin does not get them registered
at all, so a call reads as "function not found".

Admin-ness is transitive membership in the reserved `admins` local group — see
[Groups](groups.md) and [Authentication → Admin
Privileges](authentication.md#admin-privileges).

!!! warning "Deployment-wide, not per-audience"
    None of the eight filters by audience: an admin can retire any audience's
    partitions and deny every query. A fresh deployment's `admins` group holds
    a wildcard `('admins', '*')` member, which makes **every** authenticated
    caller admin until an operator takes over — `micromegas-groups add admins
    user:<you>`, then `remove admins '*'`.

## Self-service ingestion key mint {#self-service-ingestion-key-mint}

`POST {base_path}/api/ingestion-api-keys` is not purely admin-gated: a `mint`
grant is the authorization instead, resolved per request by a point query
against `audience_grants`, never a cached snapshot. Gated behind
`MICROMEGAS_SELF_SERVICE_MINT`, off by default, along with three per-caller
caps — see [Configuration](#configuration). The knob also gates `GET
.../audience-grants/my-audiences` for non-admins, non-admin grant
create/delete, and `GET .../audience-grants/visible`'s non-admin narrowing.

- **`public` ships mintable.** A fresh deployment holds `('public', 'mint',
  '*')`, so with the knob on any authenticated non-admin can mint a
  `public`-bound key with no admin step. The row confers nothing while the knob
  is off. Remove it (`micromegas-grants delete public mint '*'`, or Audience
  Access → `public` → Mint → Remove) to require a per-caller grant. A
  deployment with a custom default still gets the literal `public` row.
- **Audiences are claimed lazily.** A non-admin who names a brand-new,
  never-granted audience *explicitly* claims it inside the same transaction
  that mints the key, writing `user:<email>` rows on **both** axes. Any
  existing grant row — admin-created, self-claimed, or another caller's
  in-flight claim — makes a matching grant required instead.
- `public` and the deployment's `MICROMEGAS_DEFAULT_AUDIENCE` can never be
  *claimed*, which is distinct from being *mintable*.
- **An admin's mint claims too**: `mint_key` runs the same ownership pre-check
  and writes the admin's own `mint`+`read` rows if the audience looks
  unclaimed. The response's `claimed` field reports it. An admin with no email
  is unaffected.
- **Mint grants must live in the DB.** An env-map `"mint"` selector is inert —
  `mint_key` never consults it.

`micromegas-setup-telemetry` wraps login, mint, and printing the
`OTEL_EXPORTER_OTLP_*` env vars:

```bash
# Existing grant (resolved via GET .../my-audiences if --audience is omitted):
micromegas-setup-telemetry --url https://analytics.example.com --name my-laptop \
    --audience alice-laptop

# Fresh claim. --claim takes the name verbatim; namespacing is your convention.
micromegas-setup-telemetry --url https://analytics.example.com --name ci-runner \
    --claim "$USER-ci-runner"

eval "$(micromegas-setup-telemetry --url https://analytics.example.com --name my-laptop)"
```

See [`python-api.md`](../query-guide/python-api.md#micromegas-setup-telemetry)
for the CLI reference and [`api-keys.md`](api-keys.md) for the mint route's
error shapes (`FORBIDDEN`, `UNAVAILABLE`, `UNAUTHENTICATED`,
`CLAIM_CONTENDED`).

## DB-backed audience grants

The env map resolves once at startup, so a new per-user grant would mean an env
edit and a restart. The `audience_grants` table is the same model over HTTP,
without a redeploy:

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

**Additive, never a replacement.** Each flight-sql process — standalone, or the
monolith's role — holds a whole-table snapshot unioned with the env map. On the
**read** axis, env map, store, or both grant identical access: no precedence,
and no forced migration off the env map. The **mint** axis is DB-only.
`analytics-web-srv` is the write surface only; it caches nothing.

The table's `CHECK` constraints are re-validated in Rust on every load, so a row
that bypassed them fails the whole snapshot load loudly.

### Routes

| Route | Body / result |
|---|---|
| `POST {base_path}/api/audience-grants` | `{"audience","axis","selector"}` → 201 created or 200 already existed, returning the row |
| `DELETE {base_path}/api/audience-grants?audience=&axis=&selector=` | 204, or 404/403. Query params, not path segments, since a `group:<id>` selector may contain `/` |
| `GET {base_path}/api/audience-grants/visible` | The rows the caller may see — backs the Audience Access page's list |
| `GET {base_path}/api/audience-grants/my-audiences` | Any authenticated caller. `{"is_admin","audiences","mint_prefix","email","held_pairs","groups"}` — audiences whose `mint` selector matches this caller, a suggested namespace prefix for a fresh name (suggestion only; nothing mints under it), the `"{audience}:{axis}"` pairs held via an identity selector (empty for an admin), and the caller's transitive group closure |

There is no paginated `GET` over the whole collection; arbitrary rows come from
[`list_audience_grants()`](#list_audience_grants).

`micromegas-grants` wraps the two write routes:

```bash
micromegas-grants --url https://analytics.example.com create team-alpha read group:eng
micromegas-grants --url https://analytics.example.com delete team-alpha read group:eng
```

### Write gate

An admin acts unconditionally. A non-admin is admitted only with
`MICROMEGAS_SELF_SERVICE_MINT` on, then constrained per call:

- **Create**: `selector` must be `user:`/`group:`, never `*` — a caller who can
  read an audience must not be able to open it to every principal. The caller
  must **hold** `(audience, axis)` via an identity selector. Delegation is per
  axis: a `read` grant shares `read`, a `mint` grant shares `mint`, neither
  confers the other. A `group:X` naming a nonexistent group is refused with 404.
- **Delete**: the row must be the caller's own `user:<email>` row ("remove my
  access", never offered for `group:`/`*`), or one they created — except their
  own `mint`/`user:<email>` row, which is the claim marker
  `max_claims_per_caller` counts from and only an admin can remove. A
  nonexistent row is 404; an existing one matching no condition is 403.

### `list_audience_grants()`

A caller-scoped table function over `audience_grants`, registered for **every**
authenticated caller — a SQL auditing surface, not a REST route. No arguments;
filter with `WHERE`. Columns: `audience`, `axis`, `selector`, `created_at`,
`created_by`.

```bash
micromegas-query --all "SELECT * FROM list_audience_grants() WHERE audience = 'team-alpha'"
```

An admin sees every row. A non-admin sees every grant on each `(audience, axis)`
pair they hold a matching grant on — deliberately wider than "rows whose
selector matches me": if you may read `team-alpha`, you may see who else may. A
non-admin with an empty selector set sees nothing; maintenance callers and
`--disable-auth` requests count as admin.

Unlike `GET .../visible`, this always applies the held-pair rule, knob or no
knob — it runs in `flight-sql-srv`, which cannot see `analytics-web-srv`'s
config. See [Admin Functions
Reference](functions-reference.md#list_audience_grants) for the full schema.

### Caching and outages

A snapshot is served for `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` (default 60).
Revocation is therefore not instant: a `DELETE` removes the row at once, but
each flight-sql process keeps granting the removed access until its next
refresh.

Outage behavior differs from the DB-backed key store's. Once a process has
loaded the table successfully, later refresh failures keep serving the last good
snapshot indefinitely. A fresh process whose first query hits a down DB has no
snapshot and fails closed, retrying at most once per TTL window. A sustained
outage surfaces on `audience_grant_refresh_error_count`, not on the request
path.
