# Groups

Local group membership and admin-ness live in two Postgres tables (schema
v10, alongside `audience_grants`) and are managed with the Groups admin page
or the `micromegas-groups` CLI — no IdP `groups` claim, no
`MICROMEGAS_ADMINS`-family env var.

## Model

```sql
CREATE TABLE groups (
  name        VARCHAR(255) PRIMARY KEY,
  description TEXT,
  created_at  TIMESTAMPTZ NOT NULL,
  created_by  VARCHAR(255) NOT NULL,
  CONSTRAINT groups_name CHECK (name ~ '^[A-Za-z0-9_-]+$')
);

CREATE TABLE group_members (
  group_name  VARCHAR(255) NOT NULL REFERENCES groups(name) ON DELETE CASCADE,
  member      VARCHAR(255) NOT NULL,
  created_at  TIMESTAMPTZ NOT NULL,
  created_by  VARCHAR(255) NOT NULL,
  PRIMARY KEY (group_name, member),
  CONSTRAINT group_members_selector_shape CHECK (member = '*' OR member ~ '^(user|group):.+$')
);
```

One `group_members` row reads one way: *every principal matching `member` is
a member of `group_name`*. `member` is a selector in exactly the vocabulary
`audience_grants.selector` uses:

| Selector | Matches |
|---|---|
| `*` | any authenticated principal |
| `user:<email>` | the caller's OIDC `email` claim |
| `group:<name>` | every (transitive) member of the named group |

A `group:X` member is a selector string, not a foreign key: a group name
that doesn't exist matches nobody and is inert, the same as a dangling
`audience_grants` row today. The admin route refuses to create one against a
group that doesn't exist (404); a row inserted by other means (a direct
`psql` session) is tolerated, just inert.

`groups.name` shares `audience_grants`' `[A-Za-z0-9_-]{1,255}` charset, so a
group name is URL-safe and a distinct kind of thing from an email. Hard
`DELETE`, no `revoked_*` columns — a removed membership leaves no ongoing
artifact, same reasoning as `audience_grants`.

## Nesting and cycles

`group:X` as a member of `G` means "X nests into G" — every member of `X`
becomes a member of `G` too, transitively. A caller's resolved membership is
computed by walking **upward** from the caller: seed at every group listed
directly under `*` and `user:<their-email>`, then repeatedly follow
`group:<newly-reached-group>` to find what that group itself nests into.
The walk keeps a visited set, so a cycle (`group:a` nested in `b`, `group:b`
nested in `a`) terminates rather than looping — read-time tolerance for a
row that shouldn't exist.

Cycles are refused at write time instead: `POST .../members` rejects
`group:X` as a member of `G` (409) when `X == G` or when `G` is already
reachable upward from `X` (adding the row would close a loop). The check
runs against a freshly queried graph, not the TTL snapshot — a stale
snapshot could accept a cycle another replica just refused.

## The `admins` group

`admins` is reserved: it is seeded by the schema v10 migration and can never
be deleted (`DELETE /api/groups/admins` is refused, 409). A caller is admin
iff their resolved membership closure contains `admins` — this is exactly
what replaced `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS`/
`MICROMEGAS_INGESTION_ADMINS` and the IdP `groups` claim.

**Admins-lockout guard.** `DELETE .../{name}/members?member=` refuses (409)
whenever removing that member would leave `admins` unreachable by every
principal — no `*` and no `user:<id>`, directly or through group nesting.
This is a whole-graph reachability check, not just a check on `admins`
itself: removing the last member of a group nested into `admins` (e.g.
`admins`'s only member is `group:eng-leads`) strands `admins` exactly as
surely as emptying `admins` directly, so it's refused too — the guard
applies to any group whose membership feeds into `admins`'s reachability,
not only to `DELETE .../admins/members`. Removing a member is allowed as
long as some principal would still reach `admins` afterward; only a removal
that would leave nobody reaching it is refused.

**Two-sided authorization.** Editing group membership requires admin
authority over the group itself. Granting an audience to `group:X` still
requires authority over the *audience* (the existing per-pair hold check in
[Audiences and Grants](authentication.md#audiences-and-grants)) — holding
`admins` membership doesn't bypass that.

## The wildcard-admin warning

`admins` starting with a `('admins', '*')` row — every authenticated caller
is admin — is a real, sometimes-intentional state (see the upgrade path
below), not a misconfiguration to hide. Whenever `admins` still holds `*`:

- The web app shows an unmissable warning banner on the Admin hub and on
  the Groups page itself.
- `flight-sql-srv`/`micromegas-monolith`/`analytics-web-srv` each log a
  `warn!` once at boot: *"every authenticated caller is an admin; add a
  `user:` member to `admins` and remove `*`"*.

## Routes

All routes are admin-gated (`AdminUser`) and live on `analytics-web-srv`.

| Method | Path | Behavior |
|---|---|---|
| `GET` | `{base_path}/api/groups` | `[{"name","description","member_count","created_at","created_by"}]` |
| `POST` | `{base_path}/api/groups` | `{"name","description"?}` → 201; 400 on charset; 409 if it exists |
| `DELETE` | `{base_path}/api/groups/{name}` | 204; 409 for `admins`; 409 while referenced by a `group_members.member = 'group:<name>'` or `audience_grants.selector = 'group:<name>'` row (the response names the referrers) |
| `GET` | `{base_path}/api/groups/{name}/members` | `[{"group_name","member","created_at","created_by"}]` |
| `POST` | `{base_path}/api/groups/{name}/members` | `{"member"}` → 201 (created) / 200 (already existed); 400 on a malformed selector or over the 255-byte bound; for `group:X`, 404 if `X` doesn't exist, 409 if it would create a cycle |
| `DELETE` | `{base_path}/api/groups/{name}/members?member=` | 204; 404 unknown; 409 when no principal would still reach `admins` afterward, directly or via nesting |

`member` is passed as a query parameter on `DELETE`, not a path segment — a
`group:<id>` value isn't restricted enough in charset to be a safe raw path
segment (mirrors `audience_grants`' `DELETE ...?audience=&axis=&selector=`).

Under `--disable-auth`, `/api/groups` and `/api/groups/{*rest}` answer a
fixed 503 (`{"code": "AUTH_DISABLED", ...}`) — the real routers are never
merged in that mode, the same shape the key-management/grant routes use.

`GET {base_path}/api/audience-grants/my-audiences` gains a trailing
`"groups"` field: the caller's own resolved closure, straight off
`AuthContext`, no query — lets the CLI and the Audience Access page explain
why a caller holds a `group:` grant.

There is no `list_groups()` SQL table function: group membership is
admin-only data (unlike `audience_grants`, which has no self-service SQL
audience either — the REST list route and CLI `list`/`members` are the only
surface). A caller's own closure is exposed through
`my-audiences().groups`, which is all the self-service UI needs.

## `micromegas-groups` CLI

```
micromegas-groups --url URL list
micromegas-groups --url URL create <name> [--description TEXT]
micromegas-groups --url URL delete <name>
micromegas-groups --url URL members <name>
micromegas-groups --url URL add <name> <member>
micromegas-groups --url URL remove <name> <member>
```

Same `--url`/`--profile`/auth resolution as `micromegas-grants`. There is no
`bootstrap` convenience command — taking over from a wildcard-seeded
`admins` is the two-command sequence below, documented as-is.

## Latency and outage behavior

Local-group membership is a whole-table snapshot cache, the same shape
`audience_grants` uses, and shares that store's cache-TTL knob:
`MICROMEGAS_AUTH_CACHE_TTL_SECONDS` (default `60`) bounds how quickly a
membership or admin change takes effect **per process** — the same knob
also governs the API-key and audience-grant snapshot caches (see
[API Keys → Cache and audit env vars](api-keys.md#cache-and-audit-env-vars)).
A write through the admin routes is immediate on the writing process's own
next read of the underlying table, but every other process serving traffic
keeps its prior snapshot until the TTL elapses.

A group-store outage is surfaced as **`503`** (or `Status::unavailable` on
gRPC/FlightSQL) — never `401`/`403` — so a client retries instead of
treating a session as permanently invalid, the same convention the key
stores use.

## Upgrade path (schema v10)

1. **Deploy the new binaries and run the migration once** — start
   `telemetry-ingestion-srv` or the monolith. None of
   `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS`/
   `MICROMEGAS_INGESTION_ADMINS` should be set anywhere in the deployment:
   the migration does not read them, and every process refuses to start
   (the removed-var check) if any is still set, regardless of value.
2. **The v10 migration always seeds `admins` with a single `('admins', '*')`
   row** — fresh install or upgrade, every time, no exception. This means
   *every authenticated caller* can reach several gates that were
   previously narrower, matching the state the SQL admin-function gate
   already had:
     - The web admin routes (`AdminUser`/`require_admin`, the
       audience-grant write gate, the ingestion-key mint gate).
     - The mint-any-audience arm of the mint policy.
     - The FlightSQL `bulk_ingest` gate — now satisfiable by an
       API-key caller, not just OIDC (see
       [`bulk_ingest`](../query-guide/python-api.md#bulk_ingesttable_name-table)).
     - `list_audience_grants()`'s all-rows branch, to every
       authenticated caller.
3. **Start everything else.** `flight-sql-srv` and `analytics-web-srv`
   processes started before the migration ran answer 503 until it has (the
   schema-stale startup warning says so).
4. **Take over from the wildcard**, on every upgrade and every fresh
   install alike:
   ```
   micromegas-groups --url <analytics-web-srv URL> add admins user:<you>
   micromegas-groups --url <analytics-web-srv URL> remove admins '*'
   ```
   Do this as the first post-migration step, every time — there is no
   longer a way to preserve a prior admin list across the migration.

Anyone who relied on IdP claim-derived `group:` grants must re-add
membership by hand: the v10 migration creates each such group empty (from
every distinct `group:X` selector already present in `audience_grants`) and
logs it; the Groups page lists it with zero members. A legacy selector whose
`X` fails the group-name charset (a display name with spaces, say) is left
as an inert grant row with a migration warning — not deleted, not
force-renamed.
