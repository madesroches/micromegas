# Audience Access Page Plan (#1510)

## Overview

Add an **Audience Access** page to `analytics-web-app` for the `audience_grants` store (#1489),
open to **every authenticated user**, not just admins. It answers "what can I read, and why?",
lets a user share what they can see with other users and groups, remove their own access, and
mint an ingestion key into an audience they may mint into. Admins see the whole store and keep
every power they have today (any selector, including `*`; delete any row).

The issue framed this as an admin-only UI over the existing REST routes. Three design decisions,
taken on explicit direction, reshape it:

1. **Reads go through SQL.** A new caller-scoped table function, `list_audience_grants()`, served
   by the analytics service and reached from the web app through the same `useStreamQuery` →
   `POST /api/query-stream` path **Admin → Query Deny List** already uses. The paginated JSON
   `GET /api/audience-grants` route is deleted. This removes the whole class of "the page groups
   by audience but only holds one page of rows" problems without inventing a streamed REST
   transport, and makes the same data queryable from `micromegas-query` for ad-hoc auditing. The
   one exception is the new Audience Access page's own list, which reads a small unpaginated REST
   route instead (Design §3, §6) — its writes are fixed to this deployment's own store, so its
   read has to be too, which a selectable or default flight-SQL data source can't promise.
2. **Writes stay REST.** `POST` / `DELETE /api/audience-grants` keep their structured answers
   (201 vs 200 "already existed", 404, 403 with a reason) that the `(time, msg)` shape of a
   mutating UDTF handles badly. Their gate widens from `AdminUser` to `AuthenticatedUser` plus a
   policy: a non-admin may act only on `(audience, axis)` pairs they hold a grant on, and only on
   `user:`/`group:` selectors and their own row (Design §3).
3. **Non-admins are first-class.** The page moves out of `/admin`; the self-service mint the CLI
   already exposes (`micromegas-setup-telemetry`) gets a UI; and the server takes over the
   "admin minted a brand-new audience → grant the admin their own `read`/`mint` rows" step that
   `setup_telemetry.py` performs client-side today, so the CLI's admin branch and its two list
   round-trips disappear along with `WebClient.list_audience_grants`.

The grouped-by-audience layout (mockup Option B) stays; the same page renders fewer cards and
fewer actions for a non-admin.

## Current State

### Server — `rust/analytics-web-srv/src/audience_grants.rs`

`AudienceGrantsState { pool: Option<PgPool>, self_service_mint_enabled: bool }` (lines 45-54).
Error enum `AudienceGrantError` (80-97): 400 `BAD_REQUEST`, 404 `NOT_FOUND`, 500
`DATABASE_ERROR`, 503 `NOT_CONFIGURED`, 500 `INTERNAL_ERROR`, 403 `FORBIDDEN`.

| Route | Handler | Gate | Notes |
|---|---|---|---|
| `POST /api/audience-grants` | `create_grant` :272 | `AdminUser` | Body `{audience, axis, selector}`, `deny_unknown_fields`. `insert_or_get` CTE upsert: **201 created / 200 already existed**, full row either way. `created_by` = caller email ?? subject. |
| `GET /api/audience-grants?audience=&axis=&limit=&offset=` | `list_grants` :333 | `AdminUser` | JSON array, `created_at DESC`, `limit` default 100 clamped to 500 — silent truncation. **Deleted by this plan.** |
| `DELETE /api/audience-grants?audience=&axis=&selector=` | `delete_grant` :432 | `AdminUser` | Natural key in query params (a `group:<id>` may contain `/`). 204 / 404. |
| `GET /api/audience-grants/my-audiences` | `my_audiences` :532-568 | `AuthenticatedUser`; non-admin also needs the self-service knob (:536-540) | `MyAudiencesResponse { is_admin, audiences, mint_prefix, email }` (:507-513). `audiences` = `SELECT DISTINCT audience FROM audience_grants WHERE axis='mint' AND selector = ANY($1)` over `["*", "user:<email>", "group:<g>"…]` built in Rust (:547-559). `mint_prefix_for(&email)` (:477-505) is `pub`. **Unchanged.** |

Validation: `validate_audience` → `is_valid_audience` (`[A-Za-z0-9_-]{1,255}`), `validate_axis`
(`read`/`mint`), `validate_selector` → `valid_selector` (`*`, `user:<id>`, `group:<id>`) plus a
255-**byte** bound (`VARCHAR(255)`).

Under `--disable-auth`, `web_server.rs:420-430` merges `key_management_disabled_router`
(:308-334) instead of the real routers: every `/api/audience-grants*` and
`/api/ingestion-api-keys*` request is a fixed 503.

### Server — `rust/analytics-web-srv/src/ingestion_keys.rs` (mint)

`POST /api/ingestion-api-keys` → `mint_key` (:325-472), gated by `MintGate` (:290-316):
`AuthenticatedUser`, and for a non-admin `self_service_mint_enabled` must be on (403 otherwise).
`MintRequest { name, audience: Option<String> }`; `MintResponse { key_id, name, created_at,
audience, key }` (:274-283) — **no field says whether the audience was freshly claimed**.

Flow: `resolve_audience` (:248-265; explicit → `MICROMEGAS_DEFAULT_KEY_AUDIENCE` → 400) →
per-caller live-key bound for non-admins (:352-366) → mint-grant point query `SELECT selector
FROM audience_grants WHERE audience=$1 AND axis='mint'` (:372-385, no cache) →
`AudienceMintPolicy::resolve_audience` (admin: any valid name; non-admin: a `mint` selector must
match). On a non-admin denial with an explicit audience and an email, the **lazy-claim branch**
(:409-432) calls `try_claim_and_mint` (:489-639): advisory lock (409 `CLAIM_CONTENDED` if
contended), ownership predicate `EXISTS(grant row on any axis) OR EXISTS(key row)` (:544-550) →
403 if taken, reserved-name check (`public`, the default audience; :566-571), per-caller claim
bound (:576-590), then in one transaction writes `user:<email>` rows on **both** `mint` and
`read` and the key row. **An admin never takes this branch**: an admin minting into a brand-new
audience gets a key but no grant rows, which is why `setup_telemetry.py` writes them client-side.

### Server — the query path

- `rust/analytics/src/lakehouse/query.rs` `register_lakehouse_functions` (:104-225): ungated
  registrations (`list_partitions` :132, `list_view_sets` :139, …), then the admin gate
  `if caller.is_admin || !caller.admin_principal_possible {` (:181-224) holding the mutating
  functions and the deny-list trio.
- `rust/analytics/src/lakehouse/read_scope.rs` `CallerContext` (:46-78): `read_scope`,
  `is_admin`, `isolation_config`, `admin_principal_possible`, `identity: Option<String>`.
  **No email, no groups.** `identity` is `UserAttribution::user_id` — a subject, doc-flagged
  "audit-only, must never feed a `ReadPolicy`". Module doc (:1-16) records that
  `micromegas-analytics` deliberately does **not** depend on `micromegas-auth`; `rust/public`
  resolves auth and passes plain data down.
- `rust/public/src/servers/flight_sql_service_impl.rs` `caller_context` (:584-621) is the only
  production construction site. It already holds `ext.get::<AuthContext>()` (:596) — which
  carries `email` and `groups` — when it builds the literal at :609. Seven other struct-literal
  sites: `read_scope.rs:88,102` (constructors) and five test fixtures
  (`analytics/tests/common/db_fixtures.rs:111`, `lakehouse_admin_gate_test.rs:43`,
  `ownership_rewrite_db_test.rs:227`, `ownership_rewrite_public_view_set_tests.rs:206`,
  `prong_b_guard_db_test.rs:155`).
- `rust/analytics/src/lakehouse/list_query_denials_table_function.rs` — the UDTF template:
  `schema()` (:20-37), a `TableFunctionImpl` whose sync `call_with_args` returns a
  `TableProvider` (:51-60), and an async `scan` (:67-121) that runs the DB query, builds arrays,
  and returns `DataSourceExec` over a `MemorySourceConfig`. `deny_queries_table_function.rs`
  shows a caller value (`identity`) captured at registration time.
- `lakehouse_context.rs:96` builds `QueryDenyList::new(lake.db_pool.clone())` — the lake pool is
  the same Postgres that holds `audience_grants` (migration v7,
  `rust/ingestion/src/sql_migration.rs:192-202`).
- `rust/auth/src/policy.rs`: `selector_matches` (:104-117, **private**), `valid_selector`
  (:94-102, `pub`), `AudienceReadPolicy::resolve` (:475-504: `public` ∪ env map ∪ DB snapshot
  ∪ per-key `read_audiences`).
- `rust/analytics-web-srv/src/stream_query.rs` forwards the **web user's own bearer token**
  to flight-sql-srv (:271-275), so a UDTF's `CallerContext` for a web-app query is the logged-in
  user's. `BLOCKED_FUNCTIONS` (:90) is a substring deny-list of the three retire functions only.
- `rust/analytics/tests/lakehouse_admin_gate_test.rs` plans UDTF calls against an offline
  lakehouse (lazy pool, never dialed); `NON_MUTATING_CALLS` (:158) is where an ungated function
  belongs.

### Frontend — `analytics-web-app`

- `src/routes/QueryDenyListPage.tsx` + `src/lib/query-deny-list-api.ts`: the SQL-fronted page
  precedent — SQL builders with doubled single quotes, `decodeQueryDenyRules(table)` using
  `timestampToDate`, `useStreamQuery()` (no args; `execute`, `isStreaming`, `isComplete`,
  `error`, `getTable()`), `useDefaultDataSource`, results read in an effect keyed on
  `[q.isComplete, q.error]`, local dialog component, `Suspense` wrapper, `AuthGuard` inside both
  the content and the fallback.
- `src/components/ApiKeysAdminPage.tsx`: mint dialog (:214-286, Name + Audience inputs) and the
  one-time secret banner with copy-to-clipboard (:188-212). `src/lib/api-keys-shared.ts`
  `MintApiKeyResponse { key_id, name, created_at, key, audience? }` (:23-31), `mint(name,
  audience?)` (:101-111). `src/lib/ingestion-api-keys-api.ts` exports `mintIngestionApiKey`.
- `src/components/AuthGuard.tsx` `{ children, requireAdmin? }`; `src/lib/auth.tsx` `User { sub,
  email?, name?, is_admin? }` (:6-11).
- Navigation: `src/components/layout/Sidebar.tsx:308` shows the Admin item only to admins;
  `src/components/layout/Header.tsx:137-179` user menu has a name/email header and **Sign out**
  only — no user-facing pages exist today. `src/routes/AdminPage.tsx` has seven cards;
  `src/router.tsx` routes at :50-57.
- Nothing in the frontend calls `/api/audience-grants` today.

### Python

- `python/micromegas/micromegas/web_client.py`: `mint_ingestion_api_key` :99 (retries once on
  409 `CLAIM_CONTENDED`), `list_ingestion_api_keys` :150, `my_audiences` :180,
  `create_audience_grant` :235, `list_audience_grants` :256, `delete_audience_grant` :281.
- `cli/grants.py`: `create` / `list` (:73-97) / `delete`, `--url` required.
- `cli/setup_telemetry.py`: `run` (:240-304) calls `my_audiences` → `resolve_audience`
  (:78-149; the admin branch :134-139 calls `list_audience_grants(audience=)` and
  `_audience_has_existing_keys` :58-75, which pages the admin-only key list) → `mint` → writes
  the env file → **if `is_brand_new_admin_claim`, creates `user:<email>` `mint`+`read` grants**
  (:299-301).
- `cli/query.py` (`micromegas-query`) connects over FlightSQL via `connection.connect`; table
  functions run as `micromegas-query --all "SELECT * FROM list_query_denials()"`.
- Tests: `tests/cli/test_grants.py` (list: 3 tests), `tests/cli/test_setup_telemetry.py`
  (`resolve_audience` matrix incl. `test_admin_audience_check_pages_through_ingestion_keys`
  :334; `run()` grant-writing tests :376-570), `tests/test_web_client.py::TestAudienceGrants`
  (list: 2 tests), `TestMyAudiences`.

### Documentation

- `mkdocs/docs/admin/web-app.md:55-57` says grants are "not yet a web UI page"; `## Query Deny
  List` (:127-155) is the structural precedent for a page section.
- `mkdocs/docs/admin/authentication.md`: `## Audiences and Grants` (:295-402), `### Self-service
  ingestion key mint` (:404-472, knob table :414-418), `### DB-backed audience grants`
  (:474-544, HTTP route table :526-531, admin-gate rationale :533-540).
- `mkdocs/docs/admin/api-keys.md`: routes (:96-232, table :115-124, error shapes :131-140),
  lazy claim + prefix convention (:277-291), web admin pages (:340-411).
- `mkdocs/docs/query-guide/python-api.md`: `### micromegas-grants` (:859-883),
  `### micromegas-setup-telemetry` (:884-934).

## Design

### 1. Query path: caller selectors on `CallerContext`

Add one field to `CallerContext` (`read_scope.rs`):

```rust
/// The grant selectors this caller matches — `"*"`, `"user:<email>"` when an email is present,
/// and one `"group:<g>"` per claimed group — precomputed by `rust/public` from the
/// `AuthContext`, so `micromegas-analytics` never needs the auth crate. Empty for internal and
/// maintenance callers and for a request with no `AuthContext` at all (`--disable-auth`).
/// Consumed by `list_audience_grants()` (Design §2); admins do not need it — they see every row.
pub grant_selectors: Arc<[String]>,
```

`internal()` / `maintenance()` set it empty. At `flight_sql_service_impl.rs:609` the `auth_ctx`
bound in the `Some(auth_ctx)` arm at :596 has already gone out of scope by the time the
`CallerContext` literal is built, so the site re-reads `ext.get::<AuthContext>()` (or hoists the
lookup above the `read_scope` match) and fills `grant_selectors` via `.map(caller_selectors)
.unwrap_or_default()`, defaulting to empty when the extension is absent. The five test fixtures
gain the field (the compiler
enumerates them — the `CallerContext` doc's stated reason for having no `Default`).

The selector-list construction is shared by three consumers — this site, `my_audiences`
(`audience_grants.rs:547-559`), and the new write policy (§3) — so it becomes
`pub fn caller_selectors(caller: &AuthContext) -> Vec<String>` in `rust/auth/src/policy.rs`
next to `selector_matches`, and `my_audiences` switches to it. Semantics are identical to what
`my_audiences` builds today, including the leading `"*"` (`audience_grants.rs:547`);
`selector_matches` stays private and unchanged. The write policy (§3) is the one consumer that
must strip that leading `"*"` before binding the result — a `*` grant is not something a caller
individually holds, so it must not stand in for a hold check.

### 2. Query path: `list_audience_grants()`

New `rust/analytics/src/lakehouse/list_audience_grants_table_function.rs`, modeled on
`list_query_denials_table_function.rs`. Registered in `register_lakehouse_functions` **outside**
the admin gate, next to `list_view_sets` (:139), for every caller:

```rust
ctx.register_udtf(
    "list_audience_grants",
    Arc::new(ListAudienceGrantsTableFunction::new(
        lakehouse.lake().db_pool.clone(),
        if caller.is_admin { GrantVisibility::All }
        else { GrantVisibility::Held(caller.grant_selectors.clone()) },
    )),
);
```

No arguments; filter with `WHERE`. **Schema** (column order is the stable SQL surface):

| Column | Arrow type | Null |
|---|---|---|
| `audience` | `Utf8` | no |
| `axis` | `Utf8` | no |
| `selector` | `Utf8` | no |
| `created_at` | `Timestamp(Nanosecond, Some("+00:00"))` | no |
| `created_by` | `Utf8` | no |

Rows ordered `audience, axis, selector`. The query runs in `scan` (as the template does), so the
function plans offline and `lakehouse_admin_gate_test.rs` can list it under `NON_MUTATING_CALLS`.

**Visibility rule.** Admin: every row. Non-admin: *every grant on each `(audience, axis)` pair
the caller holds a matching grant on* —

```sql
SELECT g.audience, g.axis, g.selector, g.created_at, g.created_by
FROM audience_grants g
WHERE EXISTS (
  SELECT 1 FROM audience_grants h
  WHERE h.audience = g.audience AND h.axis = g.axis AND h.selector = ANY($1)
)
ORDER BY g.audience, g.axis, g.selector
```

with `$1` = `grant_selectors`. This is deliberately wider than "rows whose selector matches me":
if you may read `team-alpha`, you may see who else may — which is exactly the "who can see this
audience" question the page exists to answer, and it is the same set you are allowed to modify
(§3), so the page never shows a share/revoke control over a row you cannot see the siblings of.
An empty selector list (internal, maintenance callers) yields zero rows. An API-key caller
has no email or groups, so it sees only pairs that carry a `*` grant.

This function is unconditional: it cannot read `analytics-web-srv`'s `self_service_mint_enabled`
knob (a different crate/service, Current State's stated boundary), so it always applies the
held-pair rule above to a non-admin regardless of that knob. The REST `GET
.../audience-grants/visible` (§3), which backs the page's own list, narrows for a non-admin when
the knob is off; this table function does not follow it there. The two paths are deliberately
asymmetric — see §3 and Trade-offs.

The function reads the table directly on every call — **not** `DbAudienceGrantsSource`'s
TTL snapshot — so a reload after a REST write always shows the write. `stream_query.rs`'s
`BLOCKED_FUNCTIONS` needs no change (read-only function).

**What it does not show.** Effective read access is the DB table ∪ the `MICROMEGAS_AUDIENCE_GRANTS`
env map ∪ `public` ∪ per-key `read_audiences` (`policy.rs:475-504`). This function is the DB table
only; the page states the other three as standing notes (§6). Exposing the resolved `read_scope`
as a second function is a cheap follow-up, not part of this change (Trade-offs).

### 3. REST writes: from admin-only to a self-service policy

`create_grant` and `delete_grant` switch their extractor from `AdminUser(ValidatedUser)` to a new
`GrantGate(AuthContext)`, a `FromRequestParts` extractor modeled directly on `ingestion_keys.rs`'s
`MintGate` (:290-316): it calls `AuthenticatedUser::from_request_parts` first, then — for a
non-admin — checks `self_service_mint_enabled` on the layered `AudienceGrantsState` extension and
rejects with 403 `FORBIDDEN` "self-service grant management is disabled" before the handler body
ever runs. This is the same knob that already gates `my_audiences` and the mint route (sharing an
audience is the second half of the self-service feature it introduced — claim an audience, then
let your team in — so it rides the same switch rather than adding a
`MICROMEGAS_SELF_SERVICE_GRANTS` sibling), and putting the check in a `FromRequestParts` gate
rather than in the handler body is not a style choice: `auth/handlers.rs:558-563` documents why
`AdminUser` itself is an extractor and not an in-body check — doing the admin check after the
body parses would let a non-admin caller force the server to buffer up to `DefaultBodyLimit`
bytes before being rejected — and `MintGate` was created for the exact same
`self_service_mint_enabled` knob to preserve that ordering. Replacing `AdminUser` with a plain
`AuthenticatedUser` extractor and moving the knob check into the handler body, where
`Json<CreateGrantRequest>` has already been parsed, would silently drop that guarantee.
`list_grants`, `ListQuery`, `DEFAULT_LIMIT` and `MAX_LIMIT` are deleted; `GrantListEntry` is kept
(renamed `VisibleGrantRow` if that reads better) as the row type for the new `GET .../visible`
route (§3, below), which selects the same five columns.

`GrantGate` calling `AuthenticatedUser::from_request_parts` needs somewhere to convert that
call's `Unauthenticated` rejection, exactly as `MintGate` does. `IngestionKeyError` already
carries an `Unauthenticated(String)` variant (`ingestion_keys.rs:136`, rendered at :185) plus
`impl From<Unauthenticated> for IngestionKeyError` (:208-211); `AudienceGrantError` has neither —
today's `my_audiences` never needed one because it takes `AuthenticatedUser` as a handler
parameter and lets axum render the rejection directly, but `GrantGate`'s `?`-based extractor body
does not have that luxury. So `AudienceGrantError` gains its own `Unauthenticated` variant (401,
`{code:"UNAUTHENTICATED"}`) and matching `From<Unauthenticated>` impl. This is a new variant on
an already-`pub`, not-`#[non_exhaustive]` enum re-exported through `pub mod audience_grants`, so
it is recorded as a published-enum addition in the CHANGELOG (Documentation), next to the
`GrantGate`/`AdminUser` gate-widening note (which is not itself a breaking-change entry — see
Documentation).

`GrantGate` yields the caller's `AuthContext` and handles only the knob half of the pre-check:

- **Admin** → exactly today's behavior, no further checks.
- **Non-admin, self-service knob off** → 403 `FORBIDDEN` "self-service grant management is
  disabled", raised by `GrantGate` itself before the body is read.

Everything below — the per-pair hold/ownership checks — genuinely needs the parsed body (the
`audience`/`axis`/`selector` in `CreateGrantRequest`, or the query-string triple for delete) and
so stays in the handler, run after `GrantGate` has already admitted the request.

**Create (non-admin).** After the existing shape validation:

1. `selector` must be `user:…` or `group:…`. `*` is refused with 403 — a user who can read an
   audience must not be able to open it to every authenticated principal.
2. The caller must **hold** `(audience, axis)` **via an identity selector**: `SELECT
   EXISTS(SELECT 1 FROM audience_grants WHERE audience=$1 AND axis=$2 AND selector = ANY($3))`
   with `$3` = `caller_selectors(&caller)` **with its leading `"*"` filtered out**. A `*` row on
   the pair makes it publicly readable/mintable, but must not let every authenticated user plant
   durable `user:`/`group:` rows on that pair: those rows would outlive the `*` grant once an
   admin deletes it, silently re-widening access the admin thought they'd just closed. A caller
   who can act only through a `*` row gets the same 403 "you have no `<axis>` grant on
   `<audience>` to share" as one with no matching row at all. Delegation is per axis: read lets
   you share read, mint lets you share mint; neither confers the other. Holding via a `group:` row
   counts; holding via the env map does not (the web server does not load the env map, and the
   page already says env-map grants are invisible here).
3. Then the same `insert_or_get` as today, `created_by` = email ?? subject. 201 / 200 semantics
   unchanged.

The hold check and the insert are not one transaction; the race (an admin revokes the caller's
grant between the two statements) produces a row the admin can immediately see and delete, and
is not worth a lock.

**Delete (non-admin).** The row must be one of:

- the caller's **own** row: `selector = 'user:<email>'`, when the caller has an email. This is
  "remove my access". It is not offered for `group:` or `*` rows — those would affect other
  principals. **There are no negative grants**: a user whose access comes from a `group:` row
  (or `*`) cannot opt out of it — the store only records who is let in, and a member cannot edit
  a row that admits the whole group. Removing your own `user:` row only removes that one path; if
  a group or `*` row still covers you, you still hold the pair and still see it on the page.
- a row the caller **created**: `created_by = <email ?? subject>`. This is the counterpart of
  sharing: a mistaken share must be revocable by the person who made it, not only by an admin.

Implemented as `DELETE … WHERE audience=$1 AND axis=$2 AND selector=$3 AND (selector = $4 OR
created_by = $5)` when the caller has an email, with `$4 = 'user:<email>'`; when the caller has
no email, the own-row arm is dropped from the predicate entirely rather than bound to a sentinel
— `DELETE … WHERE audience=$1 AND axis=$2 AND selector=$3 AND created_by = $4` — leaving only the
`created_by = subject` arm. If zero rows, a follow-up `SELECT EXISTS` picks **404** (no such row)
or **403** (exists, not yours). Admins keep unconditional delete, including of their own rows —
the "except for admins" in the direction is read as *the self-removal rule is the non-admin's
delete permission; admins are not restricted by it*, and admin access does not depend on grant
rows anyway. A user who removes their own last `read` row on an audience loses the ability to
see or re-share it; the confirm dialog says so (§8).

**List, for the page itself (REST, not the UDTF).** New `GET
{base_path}/api/audience-grants/visible`, `AuthenticatedUser`, no admin gate — built the same way
`my_audiences` already is: no pagination, reading `AudienceGrantsState.pool` directly. Admin:
`SELECT audience, axis, selector, created_at, created_by FROM audience_grants ORDER BY audience,
axis, selector`. Non-admin, **self-service knob on**: the same `EXISTS`-joined held-pair
visibility query §2 gives `list_audience_grants()`, bound to `caller_selectors(&caller)` (the
unfiltered list — `*` is fine here, unlike the write hold-check) — every grant row on a pair the
caller holds, including other principals' `selector` and `created_by`. Non-admin, **self-service
knob off**: the query narrows to `SELECT audience, axis, selector, created_at, created_by FROM
audience_grants WHERE selector = ANY($1) ORDER BY audience, axis, selector` with `$1 =
caller_selectors(&caller)` — the caller's own rows only, never a sibling's.

This narrowing mirrors why `my_audiences` already gates on the same knob (:536-540, "must not
widen on upgrade regardless of the knob"): unlike `my_audiences`, which only ever reveals whether
the caller's own identity matches a selector, the held-pair query discloses *other* principals'
`selector` and `created_by`, so a default knob-off deployment must not hand that to every
authenticated caller. This exists only so `AudienceAccessPage` (§6) can read from the exact pool
its own writes hit.

`list_audience_grants()` (§2) cannot enforce this same narrowing: it runs in
`micromegas-analytics` / flight-sql-srv, which has no access to `analytics-web-srv`'s
`self_service_mint_enabled` config (Current State's stated crate boundary — `micromegas-analytics`
deliberately does not depend on `micromegas-auth` or any web-server config). It therefore always
runs the wider held-pair query for a non-admin, knob or no knob, and is still how
`micromegas-query` and other SQL clients read the store (§11, Decision 1). The two paths
deliberately differ: `/visible` narrows because a browsing non-admin should not get this
disclosure for free on a knob-off deployment; the UDTF does not, because it structurally cannot
check that knob, and is treated as an accepted, intentional exception (Trade-offs).

### 4. Mint route: server-side claim for admins, `claimed` in the response

Two changes in `ingestion_keys.rs`:

1. **Admins take a claim attempt for a brand-new audience, decided separately from
   `resolve_audience`.** For an admin, `AudienceMintPolicy::resolve_audience` always returns `Ok`
   (:404-408), so `mint_key` never reaches the `Err` arm the non-admin lazy-claim branch
   (:409-432) hangs off — the admin path cannot reuse that hook. Instead, once `resolve_audience`
   succeeds for an admin with an explicit, non-reserved audience, `mint_key` runs the same
   ownership check `try_claim_and_mint` uses internally (:544-550: no grant row and no key row on
   any axis) as its own, separate query — it has to be a second query, since that predicate is
   only evaluated after the advisory lock inside `try_claim_and_mint` (:509-522) and isn't
   available to `mint_key` beforehand. If the audience looks unclaimed, call `try_claim_and_mint`;
   otherwise mint proceeds on the plain insert exactly as today.
2. **`try_claim_and_mint` gets an admin mode that never turns a claim attempt into a mint
   failure.** Called for an admin, its in-lock recheck can still disagree with the pre-check —
   the row now exists (403, :552-559), the advisory lock is contended (409 `CLAIM_CONTENDED`,
   :514-522), or `"user:" + email` exceeds the 255-byte selector bound (403, :533-538) — and
   today each of those is a hard failure. For an admin, all three fall through to the ordinary
   insert instead of erroring: the claim is best-effort, `claimed` comes back `false`, and the
   admin still gets their key. A genuine DB error still propagates as 500. Per-caller claim and
   key bounds stay skipped for admins, as they are today on the plain path.
3. **`MintResponse` gains `claimed: bool`** (appended, additive JSON): `true` only when this call
   actually created the audience's first grant rows — the admin gets their key **and** their own
   `user:<email>` `mint` + `read` rows in one transaction, which is what
   `setup_telemetry.py:291-301` does client-side today (an admin who mints into a fresh audience
   cannot otherwise read what their own key uploads). The web dialog uses `claimed` to say "you
   claimed `<audience>` and now hold `read` and `mint` on it" rather than silently adding two rows
   to the page; the CLI no longer needs to infer it.

An admin minting into an *existing* audience is unchanged (the pre-check finds it already claimed
and the plain insert runs directly). An admin with no email cannot be granted a `user:` row, so
that case also stays on the plain path — the pre-existing gap, now server-side and logged.

`MintGate`, bounds, the lazy-claim branch for non-admins, `CLAIM_CONTENDED` and the reserved-name
rule for non-admins are unchanged.

### 5. Frontend: `src/lib/audience-grants-api.ts` (new)

```ts
export type GrantAxis = 'read' | 'mint'

export interface AudienceGrant {
  audience: string
  axis: GrantAxis
  selector: string
  createdAt: Date | null      // defensive, matching QueryDenyRule.createdAt
  createdBy: string
}

/** Server's raw JSON for one grant, as POST returns it. */
export interface AudienceGrantResponse { audience; axis; selector; created_at: string; created_by }

/** GET /api/audience-grants/visible (§3) — this deployment's own store, unpaginated, decoded
 *  straight from JSON (no Arrow). What `AudienceAccessPage` actually calls to list grants. */
export function fetchVisibleGrants(): Promise<AudienceGrant[]>

export class AudienceGrantError extends Error { constructor(public code: string, message: string, public status: number) }

/** `created` is false when the row already existed (200, not 201). */
export function createAudienceGrant(audience, axis, selector): Promise<{ grant: AudienceGrantResponse; created: boolean }>
/** Resolves on 204; 403/404 surface as AudienceGrantError with the server message. */
export function deleteAudienceGrant(audience, axis, selector): Promise<void>

export interface MyAudiences { is_admin: boolean; audiences: string[]; mint_prefix: string | null; email: string | null }
/** GET /api/audience-grants/my-audiences. A 403 (knob off for a non-admin) is returned as
 *  AudienceGrantError so the page can hide the self-service controls instead of failing. */
export function fetchMyAudiences(): Promise<MyAudiences>

export const AUDIENCE_PATTERN = /^[A-Za-z0-9_-]{1,255}$/
export const MAX_SELECTOR_BYTES = 255          // a BYTE bound — use TextEncoder
export function validateSelector(selector: string): string | null
```

Create/delete/my-audiences/visible are plain `authenticatedFetch` in `data-sources-api.ts`'s
shape: `createAudienceGrant` reads `response.status === 201` before awaiting the body;
`deleteAudienceGrant` never calls `.json()` on a 204; every key component goes through
`encodeURIComponent`. `fetchVisibleGrants` is what the page calls to list grants (§6) — unlike
`QueryDenyListPage`, this page's list is REST, not a SQL string handed to `useStreamQuery`, since
its writes are fixed to one store and its read must be too (§6, "Reading through this
deployment"). Minting reuses `mintIngestionApiKey` from `ingestion-api-keys-api.ts`;
`MintApiKeyResponse` in `api-keys-shared.ts` gains `claimed?: boolean`.

### 6. Frontend: `src/routes/AudienceAccessPage.tsx` (new)

Route **`/audiences`**, `AuthGuard` **without** `requireAdmin`. Reachable from a new **Audience
access** item in the Header user menu (`Header.tsx`, between the name/email header and Sign out)
for everyone, and from an eighth Admin card for admins (§9). One route file, local dialogs,
`Suspense` wrapper, `usePageTitle('Audience Access')`.

State:

```
listQuery = useVisibleGrants()        // wraps `fetchVisibleGrants()` in useStreamQuery's minimal
                                       // { isComplete, error } shape (so item 4 below needs no
                                       // special-casing) but calls REST, not SQL, and is an
                                       // all-or-nothing read with no progressive rowCount — see
                                       // "Reading through this deployment" below. No page-local
                                       // DataSourceField: unlike Query Deny List, this page's
                                       // writes can reach only one store, so its read must never
                                       // be able to point anywhere else
grants: AudienceGrant[]               // listQuery.grants once isComplete; the complete visible set
me: MyAudiences | null                // from fetchMyAudiences; null while loading or on 403
selfServiceOff: boolean               // fetchMyAudiences returned 403 (non-admin, knob off)
isAdmin = user?.is_admin              // from useAuth(); server is the authority, this is UX
myEmail = me?.email ?? user?.email
axisFilter: GrantAxis | null; findText: string
share: { audience, axis } | null; shareError; isSharing; alreadyExistedNote
deleteTarget: AudienceGrant | null; isDeleting; deleteError
mint: { open, prefillAudience? }; mintError; isMinting; mintedKey: MintApiKeyResponse | null
```

`loadGrants` = `listQuery.execute()`, invoked on mount, on `PageLayout onRefresh`, and after every
successful write (download state, don't patch locally) — no SQL string, no data source to pass;
`useVisibleGrants` always calls `fetchVisibleGrants()`. Results land in `listQuery.grants` on
`isComplete`, already typed — no Arrow decode. `fetchMyAudiences` runs once on mount and again
after a mint that returned `claimed: true` (the mintable set changed).

**Grouping** (`useMemo` over `grants` + `findText`): bucket by `audience`, then `axis`. Audiences
by `localeCompare`; within an axis `*` first, then alphabetically. `findText` decides only which
*cards* appear (matching the audience name or any selector, case-insensitively); it never hides
chips or changes counts within a surviving card — the per-card count, the summary line and the
"No mint grants" sentence are always computed from that audience's complete rows. The **Axis**
filter is the one control allowed to hide whole rows.

**Layout** (Option B mockup; a non-admin sees the same page with fewer cards):

1. Breadcrumb: `Admin / Audience Access` for admins, plain `Audience Access` otherwise. Header
   `<h1>` + subtitle — admin: *"Who can read from, and mint into, each audience."*; non-admin:
   *"The audiences you can read from and mint into, and who shares them with you."* Primary
   actions: **Mint ingestion key** (everyone, hidden when `selfServiceOff` and not admin) and
   **Add grant** (admin only; a non-admin adds grants from a card, where the pair is fixed).
2. Filter bar: **Find** and **Axis** (Both / read / mint) — both client-side; no data-source
   control (there is only one store to show, see the standing note below). Summary line *"N
   grants across M audiences"* — totals for the loaded set, never narrowed by filters.
3. Standing notes:
   - **Propagation**: read grants take effect within `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS`
     (default 60 s) because reads are served from a whole-table snapshot; mint grants and the
     rows on this page are live.
   - **Scope**: `public` is always readable by every authenticated principal — no row needed.
   - **Env-map grants**: read access may also come from the `MICROMEGAS_AUDIENCE_GRANTS` startup
     map or a per-key `read_audiences` list; neither is shown here and neither can be shared from
     here.
   - **Reading through this deployment.** Query Deny List deliberately lets an admin point at any
     registered data source, because its rule table lives wherever the query runs — reads and
     writes always land on the same service there. This page can't offer that: Share, Remove,
     Revoke, and Mint always call this deployment's own REST routes, which read and write only
     `AudienceGrantsState`'s pool — a specific Postgres connection, not something an arbitrary
     flight-SQL data source is guaranteed to also be. Rather than read through
     `list_audience_grants()` and a data source that could silently point at a different
     deployment's lake, `useVisibleGrants` calls the REST `GET .../audience-grants/visible` (§3),
     which runs the same visibility rule directly against `AudienceGrantsState.pool` — the exact
     pool `create_grant`/`delete_grant` use. There is no `DataSourceField` and no data source to
     pick: the page's read and every one of its writes are, by construction, the same connection,
     so a chip's `×` can never act on a store other than the one rendering the chip.
     `list_audience_grants()` (§2) is unaffected and stays how `micromegas-query` and other SQL
     clients audit the store (§11); this page simply doesn't route its own display through it.
   - **Non-admin, knob off** (replaces the mint, share, and removal controls): *"Self-service is
     disabled on this deployment. You can see your grants here; ask an admin to change them."*
4. `ErrorBanner` (`onRetry={loadGrants}`) for the list query's error (`listQuery.error.message`)
   and for write failures that are not dialog-scoped.
5. One card per audience: header = audience (monospace) + grant count; then a `read` row and a
   `mint` row, each an axis badge + selector chips + a **Share** button (`+ Share read access` /
   `+ Share mint access`). Share is shown when `isAdmin`, or when `!selfServiceOff` and some chip
   on the row is `user:<myEmail>` — **not** when the only chip is `*`: by §3 the create hold-check
   strips the leading `"*"` before binding, so a caller who can see a pair only through a `*`
   grant gets the same 403 as one with no matching row at all, and offering Share there would be
   a button that always fails server-side. A row held only through a `group:` chip the page
   cannot attribute to the caller also shows Share (the page cannot see the caller's groups, so
   it cannot rule this in or out from chips alone); the server remains the authority and a 403
   renders inline in the dialog. Visible-but-not-shareable is a real, intentional state for a
   `*`-only pair: the caller can see who else the pair is granted to, but cannot add to it
   themselves.
   - Chip: selector (monospace) over `created_by · created_at`. A `*` chip has a red-tinted
     border and the words *any authenticated principal*. A chip whose selector is
     `user:<myEmail>` is marked **you**.
   - Chip delete `×` is shown when `isAdmin`, or when `!selfServiceOff` and
     (`selector === 'user:' + myEmail` (**Remove my access**) or `createdBy === myEmail`
     (**Revoke**)). `aria-label` names the action and the triple. Other chips have no control
     for a non-admin.
   - An axis with no grants shows *"No mint grants — nobody can issue ingestion keys stamped with
     this audience."* (admin) — for a non-admin an empty axis simply doesn't render, since by §2
     they wouldn't see the audience through that axis anyway.
   - React key = `${audience} ${axis} ${selector}`.
6. Loading: a plain spinner + *"Loading grants…"* — `fetchVisibleGrants()` is one REST call with
   no progressive results, so there is no row count to show mid-flight; cards render only on
   `isComplete`, so a grouping is never transiently under-counted.
7. Empty states — admin: *"No audience grants yet. Every authenticated principal can already read
   `public`; add a grant to open up a named audience."* + Add grant. Non-admin: *"You hold no
   audience grants. You can read `public`. Mint an ingestion key into a new audience to claim
   one, or ask an admin for a grant."* + Mint ingestion key (when self-service is on). Filtered
   to nothing: *"No grants match this filter."*

### 7. Dialogs: Add grant / Share

One `GrantDialog` component, `ApiKeysAdminPage`'s modal chrome, two modes:

- **Add grant** (admin, from the header): Audience text input (hint `[A-Za-z0-9_-]`, ≤255),
  Axis segmented control (Read / Mint, hint *"Read: may query data stamped with this audience.
  Mint: may issue ingestion keys stamped with it. A read grant never confers mint."*), Selector
  = three-way segmented control (Everyone / User / Group) + id input, composed into `*`,
  `user:<id>`, `group:<id>` with a monospace preview. Hint: *"Matched against the caller's OIDC
  `email` / `groups` claim. There is no user directory here — enter the claim value verbatim."*
- **Share** (anyone, from a card row): audience and axis are fixed and displayed, not editable;
  Selector control offers **User / Group only** (no Everyone) — mirroring the server's rule so
  the UI never offers what the server will refuse. Title *"Share read access to `team-alpha`"*.

Submit disabled while submitting, while the audience fails `AUDIENCE_PATTERN`, or while
`validateSelector` is non-null. Server errors (400 `BAD_REQUEST`, 403 `FORBIDDEN` with its
reason) render inline at the top of the dialog body; the dialog stays open. On success the
dialog closes and `loadGrants` runs; a 200 shows a neutral note *"That grant already existed
(created 2026-08-14 by ops@example.com)."*

### 8. Delete flow

`ConfirmDialog` (`variant="danger"`, `error={deleteError}`, `isLoading={isDeleting}`), copy by
case:

- Admin / revoke-own-share: *Delete the **read** grant on `team-alpha` for `group:eng`?
  Principals matching this selector lose access once the grant cache expires (up to 60 s).*
- Remove my access: *Remove your direct **read** grant on `team-alpha`? Unless a group or
  everyone grant also covers you, you lose access to this audience and cannot restore it
  yourself — an admin or someone who holds it would have to share it again.* The hedge is
  deliberate: the page cannot see the caller's groups, so it cannot know whether a `group:` chip
  on the same row applies to them.

A 404 (someone else got there first) and a 403 (the server disagrees that it's yours) are shown
in the dialog and the list reloads.

### 9. Mint dialog

`MintKeyDialog`, reusing `ApiKeysAdminPage`'s dialog shape and its one-time-secret banner + copy
button (lifted into a small shared component rather than duplicated). Fields:

- **Name** (required, ≤255 bytes).
- **Audience**: a select of `me.audiences` (the mintable set from `my-audiences`) plus a **New
  audience** option revealing a text input. For a non-admin the new name is prefixed with
  `me.mint_prefix` — the same convention `setup_telemetry.py` applies — and the composed value is
  previewed in monospace: *"Will claim `alice-myproj` and grant you read + mint on it."* Admins
  are never prefixed. Pre-filled when opened from a card's mint row.
- Submit calls `mintIngestionApiKey(name, audience)`. Success: the banner with the key, and if
  `claimed` a line *"You claimed `<audience>`; you now hold read and mint on it."*, then
  `loadGrants` + `fetchMyAudiences`. Errors inline: 403 `FORBIDDEN` (not in your mintable set /
  bound reached / knob off), 409 `CLAIM_CONTENDED` (*"being claimed by another request — try
  again"*), 400.

The `/admin/ingestion-keys` page is untouched; this dialog is the non-admin path the CLI already
has, now in the browser.

### 10. Navigation and admin card

- `router.tsx`: `const AudienceAccessPage = lazy(() => import('@/routes/AudienceAccessPage'))`,
  `<Route path="/audiences" element={<AudienceAccessPage />} />` outside the `/admin/*` group.
- `Header.tsx`: a menu item **Audience access** → `/audiences` (icon `Users`), for every
  authenticated user.
- `AdminPage.tsx`: eighth card → `/audiences`, `Users` icon, `bg-blue-500/15 text-blue-500`,
  copy *"See who can read from and mint into each audience, and grant access."*

### 11. Python client + CLI

- `WebClient.list_audience_grants` is **removed**. `create_audience_grant`,
  `delete_audience_grant`, `my_audiences`, `mint_ingestion_api_key` unchanged (the latter's
  return dict now also carries `claimed`).
- `micromegas-grants`: the `list` subcommand is **removed**; `create` and `delete` stay. Listing
  is `micromegas-query --all "SELECT * FROM list_audience_grants()"`, which as a bonus gives a
  non-admin their own view and admins `WHERE`/`ORDER BY`. The `--help` epilog and the docs say
  so.
- `micromegas-setup-telemetry`: `resolve_audience`'s admin branch (:134-139) returns
  `(args.audience, False)` — no list calls; `_audience_has_existing_keys` and `_KEY_PAGE_SIZE`
  are deleted; `run()`'s post-mint grant-writing step (:291-301) is deleted, since the server
  now claims for admins (§4). The tuple return collapses to a single value. The stderr progress
  line gains *"claimed audience <a>"* when `result.get("claimed")`. Deleting that grant-writing
  step leaves `email = my_audiences.get("email")` unused at `setup_telemetry.py:248`; it is
  removed too. Removing `cli/grants.py`'s `list` subcommand leaves its `json` import unused;
  removed as well.

### 12. Server module docs and `--disable-auth`

`audience_grants.rs`'s module doc (currently "every handler `AdminUser`-gated except one")
is rewritten for the policy in §3. `key_management_disabled_router` (`web_server.rs:309-334`)
registers `{base_path}/api/audience-grants` **and** a `{base_path}/api/audience-grants/{{*rest}}`
wildcard under `--disable-auth`, both answered by the fixed 503 `key_management_disabled` handler
— so the page's own list read, `GET .../audience-grants/visible` (§3), 503s exactly like its
writes do, and the page renders the same disabled banner for the list as for a failed write; it
never reaches its populated state under `--disable-auth`. `list_audience_grants()` itself is
unaffected by this: `is_admin(metadata)` (`user_attribution.rs:81-90`) returns `true` when the
`x-auth-is-admin` header is absent, which is the documented `--disable-auth` convention, and §2's
registration takes the `GrantVisibility::All` branch before `grant_selectors` ever matters — so
`micromegas-query --all "SELECT * FROM list_audience_grants()"` still returns every row in the
store under `--disable-auth`, even though the web page cannot show them.

## Mockups

`tasks/1510_audience_grants_admin_ui_mockups/option-b-grouped-by-audience.html` — the chosen
layout, drawn for the admin view: one card per audience, `read`/`mint` chip rows, two-line
chips with `created_by · created_at`, per-axis add buttons, standing notes, the axis-filtered
card, loading, empty state, and the grant dialog in clean and server-error states. The
non-admin view is the same page with only the caller's pairs, **Share** in place of **Add**,
delete controls only on their own and their created chips, and the mint dialog — not separately
mocked. `option-a-flat-table.html` is the rejected flat-table alternative; it remains the better
shape for date-ordered auditing, which `list_audience_grants()` now covers from SQL directly.

## Implementation Steps

**Phase 1 — shared auth helper + caller context**

1. `rust/auth/src/policy.rs`: `pub fn caller_selectors(&AuthContext) -> Vec<String>`; switch
   `analytics-web-srv/src/audience_grants.rs::my_audiences` to it.
2. `rust/analytics/src/lakehouse/read_scope.rs`: `grant_selectors` on `CallerContext` +
   constructors; `rust/public/src/servers/flight_sql_service_impl.rs:609` populates it; update
   the five test fixtures.

**Phase 2 — `list_audience_grants()`**

3. `rust/analytics/src/lakehouse/list_audience_grants_table_function.rs` (new) +
   `GrantVisibility`; register in `query.rs` outside the gate; `mod` in `lakehouse/mod.rs`.
4. Tests: `lakehouse_admin_gate_test.rs` `NON_MUTATING_CALLS` gains the call; new `#[ignore]`d
   `rust/analytics/tests/list_audience_grants_db_test.rs` (pattern: `query_deny_list_db_test.rs`)
   seeding rows with a per-run `created_by` tag and asserting admin-all, held-pair visibility
   (incl. a `group:` hold and sibling rows), empty-selectors-zero-rows, column order/types.

**Phase 3 — REST writes + mint**

5. `audience_grants.rs`: delete `list_grants` and its types/constants; add `AudienceGrantError::
   Unauthenticated` and its `From<Unauthenticated>` impl (§3); new `GrantGate` extractor on
   create/delete with the §3 policy; add `GET .../visible` (§3), including its knob-off narrowing
   for non-admins; module doc. Router loses the old paginated `GET` on the collection path, gains
   `GET .../visible`.
6. `ingestion_keys.rs`: admin claim path + `claimed` on `MintResponse` (§4).
7. `tests/audience_grants_tests.rs`: drop list tests; add non-admin cases, split per the file's
   documented offline/live convention (:9-16). Offline, against the lazy pool: knob off → 403,
   `*` → 403 (both rejected before any query runs). `#[ignore]`d, live DB: not held → 403, held
   via `group:` → 201, own-row delete → 204, other's row → 403, absent → 404, plus `GET
   .../visible` admin/non-admin(knob on)/non-admin(knob off) cases (§3). Update
   `live_create_list_delete_round_trip` to verify via a direct `sqlx` read. `tests/
   ingestion_keys_tests.rs`: admin fresh-audience mint writes both rows and returns `claimed:
   true`; existing audience → `claimed: false`; reserved names never claimed. `tests/
   routing_tests.rs` (disable-auth 503 router) is unaffected.

**Phase 4 — frontend**

8. `src/lib/audience-grants-api.ts` (new) + `src/lib/__tests__/audience-grants-api.test.ts`;
   `claimed?` on `MintApiKeyResponse`.
9. `src/routes/AudienceAccessPage.tsx` (new) with `GrantDialog`, `MintKeyDialog`, and the
   shared one-time-secret banner extracted from `ApiKeysAdminPage.tsx`.
10. `router.tsx`, `Header.tsx` menu item, `AdminPage.tsx` card.
11. `src/routes/__tests__/AudienceAccessPage.test.tsx`.

**Phase 5 — Python + docs**

12. `web_client.py` (remove `list_audience_grants`), `cli/grants.py` (remove `list`),
    `cli/setup_telemetry.py` (§11); update `tests/cli/test_grants.py`,
    `tests/cli/test_setup_telemetry.py`, `tests/test_web_client.py`.
13. Docs and `CHANGELOG.md` per Documentation.

**Phase 6 — verification**

14. `cargo build && cargo test && cargo clippy` in `rust/`; `yarn lint`, `yarn type-check`,
    `yarn test` in `analytics-web-app/`; `poetry run pytest` and `poetry run black` in
    `python/micromegas/`.

## Files to Modify

Created:

- `rust/analytics/src/lakehouse/list_audience_grants_table_function.rs`
- `rust/analytics/tests/list_audience_grants_db_test.rs`
- `analytics-web-app/src/lib/audience-grants-api.ts`
- `analytics-web-app/src/lib/__tests__/audience-grants-api.test.ts`
- `analytics-web-app/src/routes/AudienceAccessPage.tsx`
- `analytics-web-app/src/routes/__tests__/AudienceAccessPage.test.tsx`
- `analytics-web-app/src/components/MintedKeyBanner.tsx` (lifted from `ApiKeysAdminPage.tsx`)

Modified:

- `rust/auth/src/policy.rs`, `tests/policy_tests.rs`
- `rust/analytics/src/lakehouse/read_scope.rs`, `query.rs`, `mod.rs`
- `rust/public/src/servers/flight_sql_service_impl.rs`
- `rust/analytics/tests/common/db_fixtures.rs`, `lakehouse_admin_gate_test.rs`,
  `ownership_rewrite_db_test.rs`, `ownership_rewrite_public_view_set_tests.rs`,
  `prong_b_guard_db_test.rs`
- `rust/analytics-web-srv/src/audience_grants.rs`, `ingestion_keys.rs`
- `rust/analytics-web-srv/tests/audience_grants_tests.rs`, `ingestion_keys_tests.rs`
- `analytics-web-app/src/lib/api-keys-shared.ts`, `src/components/ApiKeysAdminPage.tsx`,
  `src/components/layout/Header.tsx`, `src/routes/AdminPage.tsx`, `src/router.tsx`
- `python/micromegas/micromegas/web_client.py`, `cli/grants.py`, `cli/setup_telemetry.py`
- `python/micromegas/tests/cli/test_grants.py`, `tests/cli/test_setup_telemetry.py`,
  `tests/test_web_client.py`
- `mkdocs/docs/admin/web-app.md`, `authentication.md`, `api-keys.md`,
  `mkdocs/docs/admin/functions-reference.md`, `mkdocs/docs/query-guide/python-api.md`,
  `mkdocs/docs/query-guide/functions-reference.md`
- `CHANGELOG.md`

## Trade-offs

- **SQL for reads, REST for writes — except this page's own list, which is REST too.** Reads
  generally want a streamed, filterable result set and the query path already has one
  (`list_audience_grants()`, kept for `micromegas-query` and other SQL clients, §11); writes want
  structured status codes the UDTF log-stream shape can't give. But this page's writes are fixed
  to one store (`AudienceGrantsState`'s pool) while a flight-SQL data source is not, so its own
  list goes through a small new REST GET (`/audience-grants/visible`, §3) against that same pool
  instead of through the UDTF and a data source — the one exception to "reads go through SQL,"
  made because for this specific page the two transports would otherwise disagree about which
  store is on screen (§6, "Reading through this deployment"). The cost is a second, small
  implementation of the visibility rule (a REST handler alongside the UDTF's `scan`) instead of
  one; both apply the same `WHERE` shape from §2/§3, so a future change to the rule has two call
  sites to update, not one.
- **Deleting the JSON list route instead of keeping it beside the UDTF.** Two read paths would
  drift; the only clients were the CLI `list` subcommand and `setup_telemetry`'s emptiness check,
  both handled here. Anyone scripting against it moves to `micromegas-query`.
- **Visibility = every row on the pairs you hold, not just your own rows.** Wider, but it is the
  set you can also modify, and "who else can see this" is the question. The cost is that on an
  audience with a `*` read grant every reader sees every other `user:` row on it — acceptable
  for principals who already share the same data.
- **`/visible`'s non-admin query narrows when the self-service knob is off; `list_audience_grants()`
  never does.** The REST list route exists only to back this page, so it applies the same
  narrowing `my_audiences` already relies on: with the knob off, a non-admin's `/visible` call
  falls back to `selector = ANY(caller_selectors)` (own rows only) instead of the held-pair
  `EXISTS` query, so a default knob-off deployment never discloses one principal's grants to
  another through this route. The table function can't make the same call — `micromegas-analytics`
  / flight-sql-srv has no visibility into `analytics-web-srv`'s config — so `list_audience_grants()`
  always runs the wider held-pair query for a non-admin, exactly as an admin sees every row. This
  is a deliberate, accepted asymmetry: the function is a SQL auditing surface (like
  `list_query_denials()`), already ungated for every authenticated caller, and this plan does not
  invent a second gating channel for it.
- **`grant_selectors` as plain strings on `CallerContext`** rather than an `analytics` →
  `auth` crate dependency, honoring the existing architectural line in `read_scope.rs`. The
  cost is that selector grammar is now interpreted in two places (built in auth, matched in SQL
  by equality) — but the match is a string equality, not a re-parse.
- **Non-admin writes ride `MICROMEGAS_SELF_SERVICE_MINT`.** Fewer knobs; a deployment that
  turned on self-service claiming also gets self-service sharing. A deployment wanting claiming
  without sharing has no switch — if that ever matters, a sibling knob is a one-line addition.
- **Per-axis delegation, including mint.** A user who may mint into an audience may let a
  teammate mint too. Restricting delegation to `read` is a one-line change in §3 if preferred.
- **Server-side admin claim changes the mint route's behavior** for admins minting into a
  brand-new audience: two grant rows now appear. This is precisely what the CLI did for them
  already; the change makes the web dialog and any `curl` user consistent with it, and removes a
  client-side mirror of a server predicate. Recorded in the CHANGELOG.
- **Revoke-own-share was inferred**, not asked for: without it a mistaken share is permanent until
  an admin acts. It is scoped by `created_by`, which is an email (or subject) string, so a user
  whose email changes loses the ability to revoke their older shares — an admin still can.
- **No `read_audiences()` function yet.** The effective scope (with env map and `public`) stays a
  standing note. Exposing `CallerContext::read_scope` as a one-column function is cheap and
  independent; deferred to keep this change to one new SQL surface.
- **Cards render only on `isComplete`.** `fetchVisibleGrants()` is a single REST call, all-or-
  nothing by construction — there is no progressive paint to trade away, unlike the streamed
  `useStreamQuery` path Query Deny List uses. Rendering only on completion still buys a grouping
  that is never transiently wrong; it just costs nothing extra to do so here.
- **The `/admin/ingestion-keys` page keeps its own mint dialog.** Two mint entry points, one
  admin-only with the key table, one self-service on this page. Merging them is a separate
  cleanup.

## Documentation

- `mkdocs/docs/admin/web-app.md` — replace the "not yet a web UI page" comment (:55-57); add a
  `## Audience Access` section next to `## Query Deny List` covering the route (`/audiences`,
  every authenticated user), the SQL function it reads through and the REST writes, what a
  non-admin sees and may do (share, remove own access, revoke own shares, mint/claim), the
  self-service knob, and that `AuthGuard` is UX only.
- `mkdocs/docs/admin/authentication.md` — `### DB-backed audience grants`: route table
  (:526-531) drops the `GET` collection row and re-states `POST`/`DELETE` gates as
  `AuthenticatedUser` + policy (§3); the admin-gate rationale (:533-540) is replaced by the
  policy description; add `list_audience_grants()` and its visibility rule. `### Self-service
  ingestion key mint`: the knob now also governs sharing/removal; the lazy-claim paragraph notes
  admins now claim too; the "non-admin cannot free a slot" note stays true. `## Audiences and
  Grants`: point to the page as the way a user sees their own grants.
- `mkdocs/docs/admin/api-keys.md` — mint route row (:117) gains `claimed`; :277-291 notes the
  admin claim and that the web page applies the same prefix convention as the script;
  `## Web app admin pages` gains a pointer to the non-admin page.
- `mkdocs/docs/query-guide/python-api.md` — `### micromegas-grants`: two subcommands, list via
  `micromegas-query`; `### micromegas-setup-telemetry`: admin bullet updated (server claims).
- `mkdocs/docs/admin/functions-reference.md` — `list_audience_grants()` next to
  `list_query_denials()`, following the existing shape (`list_partitions()`,
  `list_view_sets()`): the full entry — schema, no arguments, and the visibility rule (admin sees
  every row; a non-admin sees every grant on the pairs they hold).
- `mkdocs/docs/query-guide/functions-reference.md` — `list_audience_grants()` marked 🔧
  (non-admin-gated) alongside `list_partitions()` and `list_view_sets()`: a short stub — one line
  on what it lists — ending "See [Admin Functions Reference](../admin/functions-reference.md) for
  details," matching how those two entries already point at the admin doc. This is the reference
  a non-admin actually has reason to open — §11 tells them to list their grants with
  `micromegas-query --all "SELECT * FROM list_audience_grants()"`, and §2 registers the function
  outside the admin gate, so leaving it out of the query-guide page entirely would undocument the
  non-admin path for the audience that needs it.
- `CHANGELOG.md` `## Unreleased` — amended in place, not layered with new "removed" bullets: the
  `GET`/`POST`/`DELETE {base_path}/api/audience-grants` (#1489), `micromegas-grants`
  `create`/`list`/`delete` + `WebClient.list_audience_grants` (#1489), and
  `micromegas-setup-telemetry`'s client-side admin grant write (#1374) bullets all describe
  surface that has never shipped in a release, so there is nothing to migrate from and no
  "Minor breaking change" removal notice belongs on any of them — each is simply rewritten to
  describe the surface this plan actually ships:
  - The **Web App** bullet for #1489 drops the `GET` collection route entirely (it never ships)
    and describes `POST`/`DELETE` as gated by the new self-service policy (§3: admin unrestricted;
    non-admin needs `MICROMEGAS_SELF_SERVICE_MINT` on, an identity selector, and a held pair) plus
    the new `GET .../audience-grants/visible` route backing the page's own list. The gate widening
    from `AdminUser` to the new `GrantGate` extractor is described as an HTTP authorization-surface
    change, not a **Minor breaking change** — `create_grant`/`delete_grant` are private handlers
    and `GrantGate`, like `MintGate`, is a private extractor, so nothing published changes shape.
    **Minor breaking change** does stay on `AudienceGrantError` gaining an `Unauthenticated`
    variant and its `From<Unauthenticated>` impl (§3) — a published-enum addition re-exported
    through `pub mod audience_grants`.
  - A new **Analytics** bullet documents `list_audience_grants()` (§2) — this one really is new,
    not an amendment.
  - The **Python** bullet for #1489 drops `list` from the CLI subcommands and
    `list_audience_grants` from the `WebClient` methods it documents, leaving `create`/`delete`
    and their matching methods — a plain edit to what ships, not a breaking-change note.
  - The **Python** bullet for #1374 (`micromegas-setup-telemetry`) drops the sentence about an
    admin's grant rows being written client-side via the admin grants API, replacing it with a
    note that the mint route now claims a brand-new audience for an admin caller server-side (§4)
    and that `MintResponse` gains `claimed`.
  - `CallerContext` gaining `grant_selectors` (§1) is recorded as **Minor breaking change** (Rust
    API, struct-literal construction sites) wherever `CallerContext` is next documented in this
    section.

## Testing Strategy

**Rust** — `rust/analytics`: `lakehouse_admin_gate_test.rs` proves `list_audience_grants()`
plans for admin and non-admin alike (`NON_MUTATING_CALLS`); `list_audience_grants_db_test.rs`
(`#[ignore]`, live DB) covers the visibility rule. `rust/auth`: `caller_selectors` unit cases
(no email, groups, both). `rust/analytics-web-srv`, split per `audience_grants_tests.rs:9-16`'s
documented offline/live convention: two branches stay offline against the lazy pool (`sqlx::
PgPool::connect_lazy`, never reaching the database) because `GrantGate`/the pre-query check
reject them before any SQL runs — knob off → 403, `*` refused → 403 — via
`build_handler_router_with_user_and_groups`. Every other §3 branch reaches Postgres and is
`#[ignore]`d, live DB only: not held → 403, held via `user:` → 201, held via `group:` → 201,
own-row delete → 204, created-by delete → 204, absent → 404, plus admin behavior unchanged; `GET
.../visible` for admin-sees-all, non-admin-sees-held-pairs (knob on), and
non-admin-sees-own-rows-only (knob off, mirroring `list_audience_grants_db_test.rs`'s cases and
§3's narrowing); `ingestion_keys_tests.rs` for §4.

**Frontend** (vitest + Testing Library, `QueryDenyListPage.test.tsx`'s harness minus its
data-source and `streamQuery` mocks (this page has no `DataSourceField` and reads via REST, not
flight-SQL, §6): mock `@/lib/auth`, `@/hooks/usePageTitle`, `@/components/layout`; `global.fetch`
for both the list read and the write calls):

`audience-grants-api.test.ts` — `fetchVisibleGrants` decodes a JSON array to `AudienceGrant[]`
and surfaces a non-2xx as `AudienceGrantError`; create 201/200 → `created`; delete encodes every
component and resolves on 204; 403/404 → `AudienceGrantError` with code/status/message;
`fetchMyAudiences` 403 → `AudienceGrantError`; `validateSelector` byte bound.

`AudienceAccessPage.test.tsx` —
- Admin: grouping/sorting/counts; Add grant from header with `*` allowed; delete on any chip;
  Axis filter hides rows; Find keeps whole cards; empty state with Add.
- Non-admin (`is_admin: false`, `my-audiences` 200): only the rows the mock returns are shown;
  no Add-grant header button; Share on each row offers User/Group only and posts the fixed
  pair; chip `×` only on `user:<me>` (copy says "Remove your … access") and on `createdBy ===
  me`; a 403 on create renders inline; Mint dialog lists `audiences`, prefixes a new name with
  `mint_prefix`, shows the key banner and the "claimed" line when `claimed: true`, and reloads.
- Non-admin, `my-audiences` 403: notes say self-service is disabled; no Share/Mint/× controls;
  list still renders.
- List error → banner with retry; mutations call `execute` again (no local patching).

**Python** — `test_grants.py`: `list` tests removed, `main` rejects `list`; `test_web_client.py`:
list tests removed; `test_setup_telemetry.py`: admin branch no longer calls list or key paging
(delete `test_admin_audience_check_pages_through_ingestion_keys`), `run()` never calls
`create_audience_grant`, the progress line reports `claimed`.

**Manual** — monolith with `--disable-auth` (the page's list read 503s exactly like its writes,
so it shows the disabled banner, not a populated list — `list_audience_grants()` is still
queryable directly with `micromegas-query --all "SELECT * FROM list_audience_grants()"` and
returns every row since `--disable-auth` is treated as admin); real OIDC: as admin,
mint into a fresh audience from the page and see the two new rows; as a non-admin with the knob
on, claim, share with a group, have the group member see and remove their own access, revoke the
share; cross-check with `micromegas-query --all "SELECT * FROM list_audience_grants()"` as both
principals; knob off as non-admin.

## Decisions

1. **SQL functions vs REST — resolved as a split.** Reads via `list_audience_grants()`, writes
   via REST. The earlier objection that the CLI would keep the REST list route alive no longer
   holds: the CLIs are new, `list` moves to `micromegas-query`, and `setup_telemetry`'s only
   list use is replaced by a server-side claim. The `admin_principal_possible` fallback that
   made an admin-gated SQL function unattractive is moot — the function is registered for
   everyone and scoped inside. One exception: `AudienceAccessPage` itself reads via the small
   REST `GET .../audience-grants/visible` (§3) instead of `list_audience_grants()`, because its
   writes are fixed to this deployment's own store and a flight-SQL data source is not — see §6,
   "Reading through this deployment."
2. **"Except for admins" on self-removal** is read as: the own-row rule is the *non-admin's*
   delete permission; admins are not bound by it and keep unconditional delete. Admin access
   does not derive from grant rows, so an admin deleting their own row changes nothing about
   what they can do.
3. **Delegation is per axis** (read → read, mint → mint) and never `*`. Restricting it to read
   only is a one-line change if wanted.
4. **Non-admins may also revoke rows they created** — inferred as the necessary counterpart of
   sharing; flagged in Trade-offs.
5. **No negative grants.** Self-removal deletes only the caller's own direct `user:` row. Access
   held through a `group:` or `*` row cannot be declined by the member; the page offers no
   control for it and the confirm copy hedges accordingly (§8).
6. **Web-server and analytics admin lists may differ in split mode** (`MICROMEGAS_ADMINS` vs
   `MICROMEGAS_ANALYTICS_ADMINS`). Accepted: admin role is assigned through OIDC and the two
   variables are expected to agree; the monolith shares one.
7. **`/visible` narrows for a non-admin when the self-service knob is off; `list_audience_grants()`
   does not.** The REST list route can read `analytics-web-srv`'s `self_service_mint_enabled`
   config and falls back to own-rows-only when it's off, mirroring why `my_audiences` gates on the
   same knob. The table function runs in a different crate/service with no access to that config,
   so it always applies the full held-pair rule for a non-admin — accepted as an intentional
   exception because it is a SQL auditing surface, already ungated for every authenticated caller,
   like `list_query_denials()`.
