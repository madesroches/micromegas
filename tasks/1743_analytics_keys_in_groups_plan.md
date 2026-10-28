# Analytics API Keys in Groups and Grants Plan

Issue: [#1743](https://github.com/madesroches/micromegas/issues/1743). Builds on #1549 (local
groups, `tasks/completed/1549_local_groups_plan.md`).

## Overview

An analytics API key can only read audiences granted to `*`. Grant and member selectors match
`*`, `user:<email>`, and `group:<name>`. A key has no email, and nothing seeds its group closure
from the key itself. So a service account that has to read non-public audiences can only do it
through a human identity, and that person then holds read access usable from any tool.

This plan adds a `key:<key_id>` selector, usable in two places:
- **as a group member:** an admin puts a key into a local group, and that group's `group:` grants
  apply to it;
- **as a read-grant selector:** an audience can be shared with a key directly. Admins can do this,
  and so can, through the existing self-service share flow, people who hold read on that audience
  themselves.

Either way the key stays dedicated, CIDR-restricted, and revocable. Two guards hold on both
paths: a key never becomes admin, and a key never mints.

## Current State

- **Selector vocabulary.** `valid_selector` (`rust/auth/src/policy.rs:105`) accepts `*`,
  `user:<id>`, and `group:<id>`. It is shared by `audience_grants` (the grant-map parse,
  `AudienceGrants::from_rows`, and the grant route's `validate_selector`,
  `rust/analytics-web-srv/src/audience_grants.rs:256`) and by `group_members`
  (`GroupGraph::from_rows`, `rust/auth/src/groups.rs:72`; `add_member_inner`,
  `rust/analytics-web-srv/src/groups.rs:469`). The DB mirrors it with two CHECKs:
  `audience_grants_selector_shape` (schema v7, `rust/ingestion/src/sql_migration.rs:237`) and
  `group_members_selector_shape` (v10, `:367`).
- **Matching.** `selector_matches` (`policy.rs:130`) drives `AudienceReadPolicy::resolve` and
  `AudienceMintPolicy`. `caller_selectors` (`policy.rs:121`) feeds flight-sql's
  `grant_selectors` (which `list_audience_grants()` visibility uses,
  `rust/analytics/src/lakehouse/query.rs:157`) and the share route's hold check.
- **Closure.** `GroupGraph::closure(email)` (`groups.rs:120`) seeds a BFS upward walk from the
  groups listed under `*` and under `user:<email>`.
- **Who resolves membership.** `MembershipProvider` (`rust/auth/src/membership.rs:47`) wraps the
  whole provider chain in flight-sql (`rust/public/src/servers/flight_sql_server.rs:373-377`) and
  in the monolith (`rust/monolith/src/main.rs:244`), and overwrites `ctx.memberships`. The
  `memberships: Arc::from([])` in `DbApiKeyAuthProvider` (`rust/auth/src/db_api_key.rs:499`) is a
  placeholder that gets replaced. **So an analytics key already gets the groups listed under `*`
  today**, because `closure(None)` still seeds at `*`. That answers the issue's second open
  question, and this plan leaves it unchanged.
- **Key identity on `AuthContext`.** There is no key id. `subject` is the key's *name*, which is
  neither unique nor stable.
- **Self-service sharing.** `create_grant` (`audience_grants.rs:506`) lets a non-admin add a
  `user:`/`group:` row when the self-service setting is on (`GrantGate`, `:336`). The caller must
  hold the same `(audience, axis)` through an identity selector, with `*` stripped
  (`caller_holds_pair`, `:442`). They are capped at `max_grants_per_caller` rows, and they may
  delete rows they created (`created_by`). `*` is admin-only.
- **Admin.** `is_admin()` is `memberships.contains("admins")` (`rust/auth/src/types.rs:84`).
  Under the bootstrap wildcard-admin state, every caller is an admin, keys included. That is
  unchanged here. `is_admin()` travels as the `x-auth-is-admin` header (`rust/auth/src/tower.rs:146`)
  and at flight-sql gates `bulk_ingest` (`flight_sql_service_impl.rs:1524`), view DDL
  (`view_ddl.rs:50`), unfiltered grant visibility (`GrantVisibility::All`), and the
  `lakehouse_admin` audience-guard bypass (`lakehouse/query.rs:127,162`). That is why the `admins`
  barrier matters.
- **Mint.** `AudienceMintPolicy` (`policy.rs:541`) is reached only from `mint_key` in
  `analytics-web-srv`, which accepts OIDC logins only (`rust/analytics-web-srv/src/auth/state.rs:76`).
  So an analytics key cannot reach mint today, but only because of how routes are wired.
- **UI and CLI.** `GroupsPage.tsx:140` (`AddMemberDialog`) and `AudienceAccessPage.tsx:117`
  (`SelectorKind = 'everyone' | 'user' | 'group'`) both have a kind toggle.
  `micromegas-groups`/`micromegas-grants` (`python/micromegas/micromegas/cli/`) are thin HTTP
  clients.
- **`read_audiences`.** This is the half-wired Stage 4b field
  (`tasks/data_isolation/audience_based_access_control_plan.md`). It is folded into
  `AudienceReadPolicy::resolve` but never populated.
- **Schema.** `LATEST_DATA_LAKE_SCHEMA_VERSION = 12`. A v13 (the `stream_tags` index) was
  removed in #1736 without ever shipping.

## Design

### 1. `key:<uuid>` in the shared selector vocabulary

`valid_selector` gains a fourth arm: `key:<uuid>` in **canonical** form only, meaning
`Uuid::parse_str(rest)` succeeds **and** `uuid.to_string() == rest` (lowercase and hyphenated,
which is also how Postgres renders `uuid::text`). Matching is an exact string compare, so a
non-canonical spelling would never match. New helpers in `policy.rs`:

```rust
pub fn key_selector(key_id: Uuid) -> String                 // "key:{key_id}"
pub fn parse_key_selector(selector: &str) -> Option<Uuid>  // the canonical check above
```

Because `valid_selector` is shared, `GroupGraph::from_rows`, `AudienceGrants::{parse, from_rows}`,
and both admin routes accept `key:` with no further changes.

**Read axis only.** A `key:` selector on the `mint` axis is rejected in `AudienceGrants::parse`
and `from_rows`, in the same way they reject other malformed rows. The route rejects it with
400, and the DB rejects it by CHECK. The mint guard (§4) would make such a row inert anyway.
Refusing it outright keeps the grant table from implying an authority the key does not have.

`selector_matches` gains
`key:` → `caller.analytics_key_id.is_some_and(|k| key_selector(k) == selector)`.
`caller_selectors` appends `key:<id>` when `analytics_key_id` is `Some`. That way flight-sql's
`list_audience_grants()` shows a key the pairs it holds directly, the same way it does for a
user.

### 2. Carry the key id on `AuthContext`

Add `pub analytics_key_id: Option<uuid::Uuid>` to `AuthContext`. It is `Some(row.key_id)` only
when `DbApiKeyAuthProvider`'s table is `ApiKeyTable::Analytics`, and `None` everywhere else
(ingestion keys, OIDC, static env keys, disabled-auth/maintenance contexts). It is a plain field
with no default, so the compiler lists every construction site (about 15, mostly tests).
Ingestion keys keep `None`, so neither path can change what they can do.

### 3. Closure seeding with an `admins` barrier

```rust
pub fn closure(&self, email: Option<&str>, key_id: Option<Uuid>) -> Vec<String>
```

```
base     = walk_upward(groups under "*" ∪ groups under "user:<email>")          // unchanged
key_part = walk_upward_with_barrier(groups under "key:<key_id>", ADMINS_GROUP)  // new
closure  = sorted(base ∪ key_part)
```

The barrier group is never inserted and never traversed. Implement this as an `Option<&str>`
barrier parameter on the existing `walk_upward` rather than a second copy of the loop. The
barrier covers three cases:
- a direct `('admins', 'key:…')` row that got past the CHECK,
- a key that reaches `admins` through nesting (`key → G → admins`),
- every group above `admins` that is reachable only through it.

The `*` path is unchanged, so the wildcard-admin state behaves as today.
`any_principal_reaches` (the `admins` lockout guard) still seeds only from `*`/`user:`, so a key
path never counts toward "someone can still be admin".

`MembershipProvider` passes `ctx.analytics_key_id` along. `has_wildcard_admin` becomes
`closure(None, None)`. No new cache: both paths read the existing `DbGroupsSource` and
`DbAudienceGrantsSource` snapshots, so removing a member or deleting a grant takes effect within
`MICROMEGAS_AUTH_CACHE_TTL_SECONDS`. Revocation takes effect within the same TTL through
`DbApiKeyAuthProvider`'s cache (the key stops authenticating at all).

### 4. Mint stays separate

`AudienceMintPolicy::resolve_audience` returns `Err("analytics API keys cannot mint")` when
`caller.analytics_key_id.is_some()`, before any selector matching. This turns today's
wiring-dependent guarantee into one that holds by construction for both paths, and it gives the
acceptance criterion's "no mint effect" a unit test. `ReadPolicy` needs no change: memberships
already flow into it through `group:` selectors, and direct grants flow in through
`selector_matches`.

### 5. Database (schema v13)

Replace both CHECKs:

```sql
-- KEY = '^key:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
audience_grants_selector_shape:
  CHECK (selector = '*' OR selector ~ '^(user|group):.+$'
         OR (selector ~ KEY AND axis = 'read'))
group_members_selector_shape:
  CHECK (member = '*' OR member ~ '^(user|group):.+$'
         OR (member ~ KEY AND group_name <> 'admins'))
```

There is no FK to `analytics_api_keys`. That matches `group:X`, which is a string too, not an
FK.

### 6. Routes

Both routes share a small module (`rust/analytics-web-srv/src/key_selector.rs`) with two steps:
- `canonicalize_key_selector(selector)` is pure. It accepts any `Uuid::parse_str` spelling after
  `key:` and rewrites it to `key_selector(uuid)`. It runs in the outer handler, before the shape
  check (`valid_selector` / `validate_selector`) and before `MutationAudit` is built, so the audit
  record carries the canonical id.
- `ensure_live_key(tx, selector)` runs inside the existing transaction after authorization. It
  returns **404** (`"analytics API key … not found or revoked"`) unless `analytics_api_keys` has a
  row for it with `revoked_at IS NULL`. That catches typos, and it catches ingestion key ids,
  which live in the other table.

A concurrent revoke after the check does no harm: a revoked key does not authenticate. The
lookup runs only after authorization, so the existence answer is not exposed to callers who
could not create the row anyway (and key ids are random UUIDv4s).

- **`POST /api/groups/{name}/members`** (admin-only, unchanged gate): a `key:` member into
  `admins` returns **400** (`"API keys cannot be members of admins"`), then `ensure_live_key` runs inside
  the existing transaction. Nesting `group:G` into `admins` when `G` contains a key stays
  allowed: the barrier makes it inert for the key, and humans in `G` still become admin.
- **`POST /api/audience-grants`** (admin, or a non-admin through self-service): `key:` with
  `axis = mint` returns **400**, then `ensure_live_key` runs. Everything else is the existing flow. A
  non-admin is still subject to the self-service setting, `caller_holds_pair` (the caller must
  hold `(audience, read)` through their own `user:`/`group:` selector, never `*`), and
  `max_grants_per_caller`. They may still delete only rows they created. So a person can share
  with a key exactly what they could already share with another person.
- **`DELETE`** on both routes takes the member/selector as a query param (`?member=`;
  `?audience=&axis=&selector=`) and canonicalizes it with the pure step. It does not check
  existence, so a member or grant row left behind by a revoked key can still be removed.
- **Listings** (`GET /api/groups/{name}/members`, and `GET /api/audience-grants/visible`, which
  the Audience Access page reads; row type `VisibleGrantRow`): append `key_name: Option<String>` and `key_revoked: bool`, filled by
  `LEFT JOIN analytics_api_keys k ON <col> = 'key:' || k.key_id::text`. These are HTTP JSON
  APIs, not the SQL layer, and the change is additive anyway.

Revoking a key does **not** cascade-delete its member or grant rows. They are the record of what
the key could read, they are inert once the key is revoked, and the listings flag them as
revoked.

### 7. CLI and web app

- `micromegas-groups` and `micromegas-grants`: the help strings list `key:<key_id>`.
  `micromegas-groups members` prints `key_name` and a `revoked` tag when they are present.
  `micromegas-grants` has no listing command (listing is `list_audience_grants()` through
  `micromegas-query`), so only its create/delete selector help strings change. No new commands.
- `groups-api.ts` / `audience-grants-api.ts`: row types gain `key_name: string | null` and
  `key_revoked: boolean`.
- `GroupsPage.tsx` `AddMemberDialog`: a fourth kind, `key`, hidden when the group is `admins`.
  It shows a `<select>` of live analytics keys (name plus short id) from the existing
  `analytics-api-keys-api.ts` list call. That page is admin-only, so the list call is allowed.
- `AudienceAccessPage.tsx` grant dialog: a fourth `SelectorKind`, `key`, offered only for the
  `read` axis. Admins get the same key `<select>`. Non-admins can't list keys, so they get a text
  input for the key id, which the key's operator gives them.
- Both pages render a `key:` chip as `key_name` (with the id in a `title` tooltip), plus a
  "revoked" tag when `key_revoked`.

These are small additions that copy the `group` kind's existing controls, so no mockup was
produced.

## Implementation Steps

1. **Auth core** (`rust/auth`)
   - `types.rs`: add `analytics_key_id` and fix every construction site the compiler reports
     (`db_api_key.rs`, `api_key.rs`, `oidc.rs`, `analytics-web-srv/src/web_server.rs`, and the
     tests).
   - `policy.rs`: the `key:` arm in `valid_selector`, `key_selector`/`parse_key_selector`, the
     arms in `selector_matches` and `caller_selectors`, the mint-axis rejection in
     `AudienceGrants::{parse, from_rows}`, and the analytics-key early return in
     `AudienceMintPolicy`. Update the docs on `valid_selector`, `AudienceReadPolicy`, and
     `read_audiences`.
   - `groups.rs`: the barrier parameter on `walk_upward`, the new `closure` signature, and
     module/field docs covering `key:`.
   - `membership.rs`: pass `ctx.analytics_key_id`.
   - `db_api_key.rs:479-500`: set `analytics_key_id`, and replace the stale `memberships` comment.
2. **Migration** (`rust/ingestion/src/sql_migration.rs`): `upgrade_data_lake_schema_v13` replaces
   both CHECKs. Bump `LATEST_DATA_LAKE_SCHEMA_VERSION` to 13 and add the dispatch arm.
3. **Routes** (`rust/analytics-web-srv/src/`): the new `key_selector.rs` helper. In `groups.rs`:
   the `admins` refusal, the canonicalize and `ensure_live_key` calls, delete canonicalization, and
   the `members` listing join. In `audience_grants.rs`: the mint-axis refusal, the same two calls,
   delete canonicalization, and the join on `visible_grants` (`VisibleGrantRow`).
4. **CLI** (`python/micromegas/micromegas/cli/groups.py`, `grants.py`): help strings, and
   `members` output in `groups.py`.
5. **Web app**: the API types, plus the new kind in the `GroupsPage.tsx` and
   `AudienceAccessPage.tsx` dialogs, and chip rendering.
6. **Docs and CHANGELOG** (see below).

## Files to Modify

- `rust/auth/src/types.rs`, `policy.rs`, `groups.rs`, `membership.rs`, `db_api_key.rs`,
  `api_key.rs`, `oidc.rs`
- `rust/ingestion/src/sql_migration.rs`
- `rust/analytics-web-srv/src/groups.rs`, `audience_grants.rs`, `web_server.rs`, `lib.rs` (or
  `main.rs` module list), new `key_selector.rs`
- Test constructors of `AuthContext` under `rust/auth/tests/`, `rust/public/tests/`,
  `rust/analytics-web-srv/tests/`
- `rust/auth/tests/groups_tests.rs`, `membership_tests.rs`, `policy_tests.rs`
- `python/micromegas/micromegas/cli/groups.py`, `grants.py`
- `analytics-web-app/src/lib/groups-api.ts`, `audience-grants-api.ts`,
  `src/routes/GroupsPage.tsx`, `AudienceAccessPage.tsx`, and their `__tests__`
- `mkdocs/docs/admin/groups.md`, `authorization.md`, `api-keys.md`, `web-app.md` (Audience
  Access and Groups), `functions-reference.md`, `control-plane-audit-log.md`
- `CHANGELOG.md`

## Trade-offs

- **Both paths vs. one.** Group membership alone keeps all of a key's access admin-curated in
  one place, but it needs a group even for a single key, and only admins can grant. Direct grants
  alone serve sharing, but give admins no way to manage many keys × many audiences as one unit.
  Both paths share one selector, so supporting both costs little more than supporting one.
- **`key:` inside `valid_selector` vs. a member-only validator.** Once grants accept `key:`, a
  separate member-only validator would just duplicate it. The per-table restrictions (no
  `admins`, no mint axis) live where the table-specific context is.
- **Barrier walk vs. write-time refusal only.** Refusing `key:` in `admins` does not cover
  nesting (`key → G → admins`). Refusing to nest any key-holding group into `admins` would block
  a legitimate human use of `G`. The barrier is one rule enforced at the one resolution site.
- **`analytics_key_id: Option<Uuid>` vs. a principal enum.** An enum would rewrite the `email`,
  `bound_audience`, and `allow_delegation` call sites for no behavioral gain.
- **Mint guard in the policy vs. relying on route wiring.** One early return makes "keys never
  mint" independent of which services accept keys in the future.
- **No revoke cascade.** Deleting rows on revoke would erase the record of what a key could read
  and couple `analytics_keys.rs` to two other tables.

## Decisions

- `key:` is accepted in `audience_grants.selector` as well as in group members, including from
  non-admins through self-service sharing (user call: people should be able to share their data
  with a service account as they wish, within the existing share rules).
- A user's share to a key outlives that user's own access, the same as their shares to `user:`
  or `group:` do today. The user who created it or an admin removes it. Accepted risk, unchanged
  in kind.
- Key names are shown to anyone who can see the grant row naming the key (accepted, since key
  names are labels, not secrets).
- Analytics keys keep getting the `*` path's groups, as they do today (resolves the issue's second
  open question).
- Under the bootstrap `('admins', '*')` state, analytics keys stay admin like every caller. It is
  a warned state that reopens `bulk_ingest` to keys; `is_admin()` also gates view DDL, unfiltered
  grant visibility and the audience-guard bypass.

## Migration

v13 replaces two CHECK constraints on small tables (a validating scan each). **Version-number
reuse:** a dev database that ran the removed `stream_tags` v13 and was *not* rolled back would
now pass the startup `assert_eq!` and silently skip this migration. The CHANGELOG entry repeats
a rollback note: at version 13 with a `stream_tags` index present, run
`DROP INDEX IF EXISTS stream_tags; UPDATE migration SET version = 12;` before deploying the new
build (safe because that v13 changed nothing else).

## Documentation

- `mkdocs/docs/admin/groups.md`: add the selector table row `key:<key_id>` (never in `admins`),
  the updated CHECK, the `admins` barrier rule, the 400/404 responses, the `key_name`/`key_revoked`
  listing fields, and the issue's three-command example.
- `mkdocs/docs/admin/authorization.md`: add `key:<key_id>` to the selector vocabulary (read axis
  only). In the paragraph on callers with no identity (around line 94), say that an analytics key
  matches `*`, its own `key:` grants, and its groups' `group:` grants. Keys never mint and never
  become admin through membership. In the self-service section, say that sharing with a key
  follows the same hold rule.
- `mkdocs/docs/admin/api-keys.md`: replace the "per-key `read_audiences` grant" text (lines ~67
  and ~183) with the two paths. Add a short "Granting a key read access" section. Note revocation
  latency and that rows left behind by revoked keys stay until removed.
- `mkdocs/docs/admin/web-app.md` (Audience Access, and the Groups Add-member kind toggle at
  ~lines 127-129, "everyone / user / group"): the new `key` kind.
- `mkdocs/docs/admin/functions-reference.md`: add `key:<key_id>` to the selector values (~line
  444), fix the "API-key caller with no selectors sees zero rows" statement (~457-458), and update
  the per-key `read_audiences` text (~477-479).
- `mkdocs/docs/admin/control-plane-audit-log.md` (~line 139): add `key:<key_id>` to the selector
  vocabulary.
- `CHANGELOG.md` (Unreleased): the feature, schema v13, and the **Minor breaking change**:
  `AuthContext` gains `analytics_key_id` and `GroupGraph::closure` gains a `key_id` parameter.

## Testing Strategy

No-DB unit tests (crate `tests/` folders):

- `rust/auth/tests/policy_tests.rs`
  - `valid_selector`/`parse_key_selector`: accepts canonical `key:<uuid>`; rejects uppercase,
    braced, simple (no hyphens), empty, and non-UUID suffixes.
  - `AudienceGrants::parse`/`from_rows` reject `key:` on the mint axis and accept it on read.
  - Read: a caller with `analytics_key_id = Some(k)` reads an audience whose read list contains
    `key:k` (static map and store snapshot). `Some(other)` and `None` (an OIDC user or an
    ingestion key) do not.
  - `caller_selectors` includes `key:k` only when `analytics_key_id` is `Some`.
  - Mint: an analytics-key caller whose memberships include a group that holds a mint grant on
    `A` gets `Err` from `AudienceMintPolicy::resolve_audience(Some("A"))`, while
    `AudienceReadPolicy` still yields `A` from the same group's read grant.
- `rust/auth/tests/groups_tests.rs`
  - `closure(None, Some(k))` includes `key:k`'s direct groups, their ancestors, and `*` groups.
    `closure(None, None)` and `Some(other)` exclude the key's groups.
  - Barrier: a direct `('admins', key:k)` row, nested `key → G → admins`, and a group above
    `admins` all stay out of the key's closure while `G` stays in. With `('admins', '*')`,
    `admins` is still in.
  - `any_principal_reaches("admins")` is false when the only path is through a key.
- `rust/auth/tests/membership_tests.rs`: the wrapper passes `analytics_key_id` through and yields
  `is_admin() == false` for a key nested under `admins`. With `None`, it yields only `*` groups.
- `rust/auth/src/db_api_key.rs` in-file tests (the pre-populated-cache pattern): an
  analytics-table row yields `analytics_key_id == Some(key_id)`; an ingestion row yields `None`.
- `rust/analytics-web-srv`: no-DB tests for the pre-query validation, where the existing route
  harness reaches it before the pool: a malformed `key:` (400), `key:` into `admins` (400), and `key:`
  on the mint axis (400). Canonicalization of an uppercase UUID is a pure-function unit test of
  `canonicalize_key_selector`, not a route test (the route reaches the existence query). The key-existence 404, the
  listing joins, and the self-service hold check against `key:` rows need a real DB. Per
  `CLAUDE.md` those are manual verification, not new live-DB tests.
- Migration: no new live-DB test. The existing `#[ignore]` migration tests exercise v13 when they
  are run.
- Web app: `GroupsPage.test.tsx` covers the `key` kind (absent for `admins`, posts `key:<id>`)
  and the chip with name and revoked tag. `AudienceAccessPage.test.tsx` covers the `key` kind
  (absent on the mint axis; a select for admins, a text input for non-admins) and the chip.
- Python: one test in `python/micromegas/tests/cli/test_groups.py`, following the existing
  FakeClient + capsys pattern, asserting that `members` prints `key_name` and the revoked tag.

## Manual Verification

These need the real Postgres schema, the self-service setting, and the real flight-sql auth
stack, and a breakage would be obvious on the first run.

1. `python3 local_test_env/ai_scripts/start_services.py --monolith`. Confirm `/tmp/monolith.log`
   reports data lake schema 13.
2. As an admin, mint analytics key K1 and note its `key_id`. Create audience `secret` with data,
   and no `*` read grant on it.
3. **Group path.** `micromegas-groups … create usage-reports`, then
   `micromegas-groups … add usage-reports key:<K1 IN UPPERCASE>`. The output shows the canonical
   id, and `members` shows the key name. `add admins key:<K1>` → 400. `add usage-reports
   key:<random uuid>` → 404. Then `micromegas-grants … create secret read group:usage-reports`:
   a query with K1 returns `secret` rows. After `remove usage-reports key:<K1>`, within 60 s it no
   longer does.
4. **Direct path, admin.** `micromegas-grants … create secret read key:<K1>`: K1 reads `secret`.
   `create secret mint key:<K1>` → 400.
5. **Direct path, self-service.** Enable self-service sharing. Give user U `read` on audience
   `team-a` through `user:U`. As U, share `team-a` read with `key:<K1>` from the Audience Access
   page (text input). K1 now reads `team-a`. As U, sharing `secret` (not held) → 403. Sharing
   `public` (held only through `*`) → 403. U can delete the row they created.
6. Revoke K1. Within 60 s K1 is rejected, and both pages show its chips tagged "revoked"; the
   rows can still be removed.

## Open Questions

1. **Drop `read_audiences`?** Recommendation: yes, as a separate follow-up that deletes the field,
   its `AudienceReadPolicy` fold, and the Stage 4b section. Direct `key:` read grants now cover
   exactly what it was planned for, and they can be edited.
