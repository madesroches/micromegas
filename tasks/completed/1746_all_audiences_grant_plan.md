# All-Audiences (`*`) Read Grant Plan

Issue: [#1746](https://github.com/madesroches/micromegas/issues/1746). Builds on #1743 (`key:`
selector, `tasks/completed/1743_analytics_keys_in_groups_plan.md`).

## Overview

Two parts. First, a UI bug: in the Add audience grant dialog, typing `*` as the audience leaves
**Add grant** disabled with no explanation. The audience field only shows a static hint, and the
computed `selectorError` is never rendered. Second, the feature the reporter was probably after:
a grant whose audience is `*`, meaning every audience, including ones created after the grant.
It is allowed on the `read` axis only, only admins can create it, and it cannot be combined with
the `*` selector. With it, an analytics key (or a user or group) can read the whole lake without
being admin and without one grant per audience.

Two supporting changes come with it. Schema v14 drops the last three shape CHECKs on the grant
and group tables, so all grant and group vocabulary is validated in code. And the grant and group
snapshot loaders skip a malformed row (with a warning and a metric) instead of failing the whole
load, so one bad row written by direct SQL no longer takes the grant store down for everyone.

## Current State

- **Audience vocabulary.** `is_valid_audience` (`rust/auth/src/policy.rs:47`) accepts
  `[A-Za-z0-9_-]{1,255}`. The grant map parse (`AudienceGrants::parse`, `:283`) and the DB row
  parse (`AudienceGrants::from_rows`, `:329`) both reject any other audience. The DB mirrors this
  with `audience_grants_audience_name CHECK (audience ~ '^[A-Za-z0-9_-]+$')` (schema v7,
  `rust/ingestion/src/sql_migration.rs:236`). `write_audience.rs` and `analytics/src/audience.rs`
  keep their own copies for *stamped* audiences, which are not affected here.
- **Snapshot failure mode.** Every row parser is all-or-nothing:
  - `AudienceGrantsLoader::fetch` (`rust/auth/src/db_audience_grants.rs:53`) returns `Err` on an
    unrecognized axis, and `AudienceGrants::from_rows` returns `Err` on a bad audience or selector.
  - `GroupsLoader::fetch` (`rust/auth/src/groups.rs:219`) goes through `GroupGraph::from_rows`
    (`:52`), which returns `Err` on a bad group name or member selector.
  - `SnapshotSource` (`rust/auth/src/db_snapshot.rs`) turns that `Err` into "unavailable" on a
    cold start, or into serving the last good snapshot forever afterwards. Either way, **one**
    bad row breaks reads, or freezes grant changes, for **every** caller.
  - Other callers of the same parsers fail the request: mint's point query
    (`authorize_mint`, `rust/analytics-web-srv/src/ingestion_keys.rs:378`, mapped to 503), and
    the group routes' in-transaction graph reads (`groups.rs:580`, `:736`, mapped to 500).
- **Read resolution.** `AudienceReadPolicy::resolve` (`policy.rs:506`) builds an explicit
  `BTreeSet<String>` of audiences from the static map and the store snapshot, and returns
  `ReadableAudiences(Arc<[String]>)` (`:396`). The flight-sql bridge (`caller_context`,
  `rust/public/src/servers/flight_sql_service_impl.rs:616-636`) maps it to
  `ReadScope::Audiences`. There is no audience registry, so "every audience" cannot be listed.
- **`ReadScope`** (`rust/analytics/src/lakehouse/read_scope.rs:26`) is `All` (internal,
  maintenance, and `--disable-auth` only; never a policy output) or `Audiences(list)`. Consumers:
  - `OwnershipRewrite` (`ownership_rewrite.rs`): `audiences()` (`:202`) feeds
    `resolved_predicate` (`:218`, `resolved_audience IN (...)` for the semi-join and `EXISTS`
    branches) and `audience_column_predicate` (`:238`, `audience IN (...)`). `analyze` skips
    everything for `All` (`:488`).
  - `AudienceGuard` (`audience_guard.rs`): `is_readable` (`:317`), `authorize`, `readable_ids`,
    `global_rows_visible` (`:463`), and `authorize_view_instance`, all short-circuiting on `All`.
  - `list_partitions` (`list_partitions_table_function.rs:143`) and `make_session_context`
    (`query.rs:334`) compare against `All` only.
- **Grant route.** `create_grant_inner` (`rust/analytics-web-srv/src/audience_grants.rs:513`)
  runs `validate_audience`/`validate_axis`/`validate_selector`, then the `key:`-on-mint refusal,
  then the non-admin checks (selector `*` → 403, own mint claim marker, `caller_holds_pair`,
  `max_grants_per_caller`). `delete_grant_inner` (`:724`) runs only `validate_axis`.
  `caller_holds_pair` and the `/visible` and `list_audience_grants()` held-pair queries all
  compare `audience` literally.
- **Web app.** `GrantDialog` (`analytics-web-app/src/routes/AudienceAccessPage.tsx:134`):
  `audienceValid = AUDIENCE_PATTERN.test(audienceInput)` (`:180`), static hint at `:227`,
  `selectorError` computed at `:181` and never shown. `groupGrants` (`:86`) builds one card per
  audience, so a `*` row would already get its own card, labeled with a bare `*`.
  `AUDIENCE_PATTERN` and `validateSelector` live in `src/lib/audience-grants-api.ts`.
- **CLI.** `micromegas-grants` (`python/micromegas/micromegas/cli/grants.py:88`) is a thin HTTP
  client. Its audience help string says `[A-Za-z0-9_-]{1,255}`.
- **Schema.** `LATEST_DATA_LAKE_SCHEMA_VERSION = 13`, deployed. v13 dropped the two selector-shape
  CHECKs (#1743). Three shape CHECKs remain on the grant and group tables:
  `audience_grants_audience_name` (v7), the unnamed inline `axis IN ('read', 'mint')` on
  `audience_grants` (v7, auto-named `audience_grants_axis_check` by Postgres), and `groups_name`
  (v10). The four audience-name CHECKs on stamped data (`ingestion_api_keys`, `processes`,
  `streams`, `blocks`) are unrelated to grants and stay.

## Design

### 1. Audience selector vocabulary (`rust/auth/src/policy.rs`)

```rust
/// The grant audience meaning every audience, current and future. Read axis only.
pub const ALL_AUDIENCES: &str = "*";

pub fn is_valid_grant_audience(aud: &str) -> bool {
    aud == ALL_AUDIENCES || is_valid_audience(aud)
}
```

`is_valid_audience` is unchanged: it still defines what a *stamped* audience is, and `*` can
never be one. Only grant parsing uses the new predicate.

`AudienceGrants` gains a separate field rather than an `entries["*"]` key, so `readers()` never
yields `*` as if it were a real audience:

```rust
pub struct AudienceGrants {
    entries: BTreeMap<String, GrantEntry>,
    all_audiences_read: Vec<String>,   // selectors on the ('*', read) pair
}
```

`parse` and `from_rows` accept `is_valid_grant_audience`, send `*` read selectors to
`all_audiences_read`, and treat `*` on the mint axis as invalid, in the same way they treat `key:`
on mint. (`parse` rejects the whole document; `from_rows` skips the row, see §7.) `merge`
concatenates the new field too. `(*, read, *)` is **not** invalid here: it is coherent, just too
broad to be self-service (see §4). A DB row like that can only come from direct SQL.

### 2. A third read scope: every audience

`ReadableAudiences` becomes an enum:

```rust
pub enum ReadableAudiences {
    Listed(Arc<[String]>),
    Every,
}
```

`AudienceReadPolicy::resolve` returns `Every` as soon as any selector in `all_audiences_read`
(static map or store snapshot) matches the caller, checked before the per-audience loop.
Otherwise it returns `Listed` as today. `into_inner` is replaced by a `match` in the bridge.

`ReadScope` gains a matching variant:

```rust
pub enum ReadScope {
    All,
    Audiences(Arc<[String]>),
    /// Any row carrying an audience. A request scope like `Audiences`, never `All`.
    EveryAudience,
}
```

`EveryAudience` is **not** `All`. It goes through the same enforcement as `Audiences`, and only
the membership test changes from "audience in list" to "has an audience". Per consumer:

| Consumer | `EveryAudience` behavior |
|---|---|
| `OwnershipRewrite::analyze` | Not short-circuited (`!= All`), so the rule is built and every branch runs as for `Audiences`. |
| `resolved_predicate` | `resolved_audience IS NOT NULL`. A process with no `processes` row still has no match, as today. |
| `audience_column_predicate` | `audience IS NOT NULL` on the (cast) column. Simplified away by DataFusion on views whose `audience` is non-nullable. |
| `is_readable` | `Audience(_)` → true, `Unknown` → false, `Ambiguous` → non-empty and every owner readable. |
| `authorize` / `readable_ids` / `authorize_view_instance` | Unchanged code: they resolve ownership and call `is_readable`. |
| `global_rows_visible` | Same arm as `Audiences` (public view set or lakehouse admin). |
| `list_partitions` / `make_session_context` | Unchanged (`!= All` already treats it as restricted). |

`ownership_rewrite.rs`'s `audiences()` helper is replaced by one helper that builds the
membership predicate for a given column expression (`IN (...)`, `lit(false)` for an empty list,
or `IS NOT NULL`), shared by both predicate functions. That keeps the empty-list fail-closed rule
in one place.

Bridge (`flight_sql_service_impl.rs:626`): `Listed(a)` → `ReadScope::Audiences(a)`,
`Every` → `ReadScope::EveryAudience`.

Revocation: the grant lives in the existing `DbAudienceGrantsSource` snapshot, so deleting it
takes effect within `MICROMEGAS_AUTH_CACHE_TTL_SECONDS`, with no new cache.

### 3. Database (schema v14): drop the remaining grant/group shape CHECKs

```sql
ALTER TABLE audience_grants DROP CONSTRAINT IF EXISTS audience_grants_audience_name;
ALTER TABLE audience_grants DROP CONSTRAINT IF EXISTS audience_grants_axis_check;
ALTER TABLE groups DROP CONSTRAINT IF EXISTS groups_name;
```

`upgrade_data_lake_schema_v14`, a dispatch arm, and `LATEST_DATA_LAKE_SCHEMA_VERSION = 14`. The
axis constraint was declared inline without a name, and Postgres names that deterministically
`audience_grants_axis_check`. The v14 doc comment says so. After v14, the only constraints left on `audience_grants`, `groups`,
and `group_members` are structural ones: primary keys, `NOT NULL`, column lengths (`VARCHAR(255)`, and `VARCHAR(4)` for `axis`), and
the `group_members.group_name → groups(name)` foreign key.

**Code validation coverage**, checked write path by write path. Direct SQL is the only writer
these checks don't cover, and §7 handles that:

| Column | Writers and their check | Readers and their check |
|---|---|---|
| `audience_grants.audience` | `create_grant`: `validate_audience` (`audience_grants.rs:243`; delete does not call it), becoming `is_valid_grant_audience` plus the `*` rules in §4. Lazy claim in `try_claim_and_mint` (`ingestion_keys.rs:734`): the audience comes from `resolve_audience` (`:267`, `is_valid_audience`), called first in `mint_key` (`:428`); the same transaction also inserts into `ingestion_api_keys`, whose stamped-audience CHECK stays. v9 seed: literal `'public'`. | `AudienceGrants::from_rows` (snapshot loader and `authorize_mint`). |
| `audience_grants.axis` | `create_grant` / `delete_grant`: `validate_axis` (`:252`). Lazy claim: literal `"mint"` / `"read"`. v9 seed: literals. | Snapshot loader's axis `match` (`db_audience_grants.rs:63`). `authorize_mint` filters `axis = 'mint'` in SQL, so it never sees another axis. |
| `groups.name` | `create_group`: `validate_group_name` (`groups.rs:297`). v10: literal `'admins'`, and the legacy backfill checks `is_valid_group_name` (`sql_migration.rs:398`). There is no rename route. | `GroupGraph::from_rows` (snapshot loader, and the route graph reads). |
| `group_members.group_name` | Bounded by the FK to `groups(name)`, which stays. | `GroupGraph::from_rows`. |

Lengths stay enforced twice: the column types remain, and the Rust predicates cap
at 255 bytes (`is_valid_audience`, `is_valid_group_name`, `MAX_SELECTOR_BYTES`).

Display paths read rows raw and need no validation: `/visible`, `list_audience_grants()`,
`/my-audiences` (`held_pairs` can now contain `*:read`; only `canShareRow` reads it, see §5), and the group
listings.

### 4. Route (`rust/analytics-web-srv/src/audience_grants.rs`)

- `validate_audience` uses `is_valid_grant_audience`, and its message names both forms. It
  gates create only; `delete_grant_inner` needs no change, so a `*` row can be removed like any
  other.
- `create_grant_inner`, after the existing `key:`-on-mint refusal, when
  `body.audience == ALL_AUDIENCES`:
  - `axis = mint` → **400** `"the '*' audience (all audiences) is allowed on the read axis only"`;
  - `selector = '*'` → **400** `"a '*' audience with a '*' selector would let every principal
    read every audience; grant the audiences individually or use public view sets"`;
  - non-admin → **403** `"only admins may create grants on the '*' audience"`. This is the first
    check in the `!caller.is_admin()` branch, before `caller_holds_pair`, so the 403 does not
    depend on what the caller holds.
- The audit log records the triple as-is (`MutationAudit::grant`). No change.
- `caller_holds_pair`, `/visible`, `/my-audiences`, and `list_audience_grants()` compare
  `audience` literally. So a `*` grant shows up as its own `(*, read)` pair, and holding it does
  **not** let a non-admin share a literal audience (see Decisions).

### 5. Web app

`src/lib/audience-grants-api.ts`:

```ts
export const ALL_AUDIENCES = '*'
/** null when (audience, axis, selector) is a creatable grant shape; otherwise a reason. */
export function validateGrantAudience(audience: string, axis: GrantAxis, selector: string): string | null
```

It returns, in order: `null` for an empty audience (no error shown before typing); the charset
message when the value is neither `*` nor `AUDIENCE_PATTERN`; `"'*' (all audiences) is
read-axis only"` on mint; `"'*' audience with the everyone selector is not allowed"` when the
selector is `*`.

`GrantDialog` (Add mode only; Share mode has a fixed audience):
- `audienceValid` becomes `audienceInput !== '' && validateGrantAudience(...) === null`.
- The audience field renders the error inline (red text, replacing the static hint) when it is
  non-null. When the input is `*` and valid, the hint reads "All audiences, current and future —
  read only." When the input is `*` and the axis is mint, the mint error is shown. The
  "point at the everyone selector" case is covered by hiding the `everyone` kind while the audience is
  `*`; the selector-`*` message stays as a fallback in `validateGrantAudience` (covered by
  `audience-grants-api.test.ts`).
- `selectorError` is rendered below the selector preview when `idInput !== ''`, so the dialog
  does not open with an error showing.
- When the audience is `*`, the `everyone` kind is removed from `kinds`, and switching to `*`
  while `everyone` is selected resets the kind to `user`. The reset runs in the audience
  input's `onChange` (add mode defaults the kind to `everyone`); the existing `key`-on-mint reset
  lives in the axis `onClick`.

Page:
- The `*` card's header reads **All audiences** with the literal `*` in a `title` tooltip and a
  short note: "applies to every audience, including ones created later". The card uses the red
  border style already used for `*` selector chips. `groupGrants` sorts it first.
- `canShareRow` returns false for the `*` audience when `!isAdmin`; the server would 403 anyway.
- The empty mint-row sentence is not rendered for the `*` card (mint is impossible there).

No mockup: this reuses the existing card and chip styles, with a header label and inline error
text.

### 6. CLI

`grants.py`: the `create` audience help becomes `"Audience name ([A-Za-z0-9_-]{1,255}), or '*'
for all audiences (read axis, admin only)"`, and the module docstring gains a one-line example.
`delete` help: `"Audience name, or '*'"`. No code change: the server does the validation, and the
shell quoting (`'*'`) is the user's job, shown in the docs. `web_client.py`'s
`create_audience_grant` / `delete_audience_grant` docstrings get the same mention.

### 7. Resilient row parsing: skip a bad row, don't fail the load

Grants and group memberships only ever *grant*. Dropping a row can only narrow what someone can
do, so skipping a malformed row is fail-closed for that row and keeps everyone else working.

- `AudienceGrants::from_rows` and `GroupGraph::from_rows` stop returning `Result`. Each returns
  the parsed value plus the rows it skipped, in a small shared type in `db_snapshot.rs`:

  ```rust
  pub struct Parsed<T> {
      pub value: T,
      /// One human-readable reason per skipped row, e.g. "audience_grants row ('team alpha',
      /// read, user:a@x): invalid audience name".
      pub skipped: Vec<String>,
  }
  ```

  The checks themselves don't change; a failed check now means `continue` instead of `return
  Err`. In `GroupGraph::from_rows`, a skipped group name also makes its member rows fail the same
  name check, so no edge points at a group that isn't in the set.
- `AudienceGrantsLoader::fetch`: an unrecognized axis becomes a skipped row too. The inline axis
  `match` (`db_audience_grants.rs:63-72`) is extracted into a pure helper that returns the skipped
  reason, so it is testable without a DB. `try_get` and
  query failures stay `Err`: they mean the DB or schema is broken, not that one row is bad.
- Logging: each loader's `fetch` calls one shared helper,
  `db_snapshot::report_skipped(name, &skipped)`. It emits `warn!` per skipped row, naming the
  store and the reason. It only logs. Each `fetch` also emits its own literal-named metric next to
  the call (`imetric!` needs a static name): `audience_grant_skipped_row_count` /
  `group_skipped_row_count`. The metric carries `skipped.len()` on every refresh, 0 included, and
  `report_skipped` is called only when `skipped` is non-empty (the maintenance.rs data-integrity pattern).
  This repeats once per refresh (one TTL) for as long as the row exists. That is the intended
  signal: an operator has to fix or delete the row.
- `authorize_mint` (`ingestion_keys.rs:378`): uses `.value`, and `warn!`s any skipped rows. Its
  503 arm goes away, because a malformed mint selector no longer blocks valid ones on the same
  audience.
- `groups.rs:580` (cycle check on add) and `:736` (`admins` lockout guard on remove) use `.value`,
  `warn!` on non-empty `skipped` as `authorize_mint` does, and their 500 arms go away. Both stay sound on a graph with skipped edges. A skipped edge is
  inert in resolution, so a cycle through it can't loop. The lockout guard only gets more
  conservative: it can refuse a removal that only a skipped row would have justified, but it can
  never allow one it shouldn't.
- `AudienceGrants::parse` (the static JSON map, a test-only entry point) stays strict. A config
  document should fail fast on a typo.
- The comments that call these arms "unreachable short of a direct SQL write" and the
  "fails the snapshot load" doc comments (`db_audience_grants.rs:50`, `groups.rs:46-51`,
  `policy.rs` `from_rows`, `ingestion_keys.rs:382-385`, `policy.rs:327-328` "the one place both
  paths fail closed on a malformed row", the `GroupsError::Internal` doc at
  `analytics-web-srv/src/groups.rs:75-76`, and `sql_migration.rs:222` / `sql_migration_test.rs:220`
  on the dropped CHECKs) are rewritten to describe skipping.

## Implementation Steps

1. **Auth** (`rust/auth/src/policy.rs`): `ALL_AUDIENCES`, `is_valid_grant_audience`, the
   `all_audiences_read` field and its handling in `parse`, `from_rows`, and `merge`, the
   `ReadableAudiences` enum (`new` is replaced by constructing `Listed(..)`, including
   `read_policy_threading_tests.rs:332` and `policy.rs:533`; `into_inner` call
   sites, ~20 in `policy_tests.rs`, 3 in `db_audience_grants_tests.rs`, and
   `read_policy_threading_tests.rs:179` and `:234`, move to matching), the `Every` branch in `AudienceReadPolicy::resolve`, and doc
   comments (module doc, `ReadPolicy` "can never return all" wording: `Every` is still not
   `All`; the resolve formula block, the `ReadPolicy` "maximally permissive still has to enumerate the
   audiences" passage at ~`:418-422`, and the `ReadableAudiences::new` citation at `:427`). Update `db_audience_grants.rs`'s fetch doc if it names the
   audience rule.
2. **Analytics** (`rust/analytics/src/lakehouse/`): `ReadScope::EveryAudience` in
   `read_scope.rs`; `is_readable` and `global_rows_visible` in `audience_guard.rs`; the shared
   membership-predicate helper in `ownership_rewrite.rs`. Fix the stale docs that describe
   `ReadScope` as two-valued or `IN`-only: `read_scope.rs:5-6` and the enum doc (`:22-27`),
   `audience_guard.rs:29-31`, the `ownership_rewrite.rs` branch table (`:56`) and the
   `resolved_predicate`/`audience_column_predicate` docs (`:209-237`), and
   `list_partitions_table_function.rs:182`, `:211`.
3. **Bridge** (`rust/public/src/servers/flight_sql_service_impl.rs`): the `match` in
   `caller_context`. Fix test doubles in `rust/public/tests/read_policy_threading_tests.rs`
   reported by the compiler, and add the `Every` bridge test (Testing Strategy).
4. **Migration** (`rust/ingestion/src/sql_migration.rs`): `upgrade_data_lake_schema_v14` (§3),
   the dispatch arm, and `LATEST_DATA_LAKE_SCHEMA_VERSION = 14`. In
   `rust/ingestion/tests/sql_migration_test.rs`, `v7_creates_audience_grants_table_with_constraints`
   migrates to latest and then expects the audience CHECK to reject `'group:everyone'` (~line 307)
   and the axis CHECK to reject `'write'` (~line 294). The audience insert now succeeds. `axis` is
   `VARCHAR(4)`, so `'write'` would still fail on length: switch that insert to a 4-character
   unknown axis (`'xyz'`), which now succeeds. Flip both assertions to "accepted at v14" and
   update the doc comment. The v7 test's "v13" wording (`sql_migration_test.rs:315`, `:322`)
   becomes v14.
4b. **Resilient parsing** (§7): `Parsed<T>` and `report_skipped` in
   `rust/auth/src/db_snapshot.rs`; the `from_rows` changes in `policy.rs` and `groups.rs`; the
   loaders in `db_audience_grants.rs` (including the axis-`match` helper) and `groups.rs`; the per-loader skipped-row `imetric!`; and the three route call sites in
   `rust/analytics-web-srv/src/ingestion_keys.rs` and `groups.rs`.
5. **Route** (`rust/analytics-web-srv/src/audience_grants.rs`): `validate_audience`, the three
   `*`-audience checks, and the `create_grant` doc comment's list of non-admin checks.
6. **Web app**: `audience-grants-api.ts` (`ALL_AUDIENCES`, `validateGrantAudience`) and
   `AudienceAccessPage.tsx` (dialog errors, kind filtering, `*` card label, `canShareRow`).
7. **CLI**: `grants.py`, `web_client.py` docstrings.
8. **Docs and CHANGELOG** (below).

## Files to Modify

- `rust/auth/src/policy.rs`, `db_audience_grants.rs`, `groups.rs`, `db_snapshot.rs`
- `rust/analytics/src/lakehouse/read_scope.rs`, `audience_guard.rs`, `ownership_rewrite.rs`
- `rust/public/src/servers/flight_sql_service_impl.rs`
- `rust/ingestion/src/sql_migration.rs`, `rust/ingestion/tests/sql_migration_test.rs`
- `rust/analytics-web-srv/src/audience_grants.rs`, `ingestion_keys.rs`, `groups.rs`
- Tests: `rust/auth/tests/policy_tests.rs`, `groups_tests.rs`, `db_audience_grants_tests.rs`,
  `membership_tests.rs` (`.expect` on `from_rows` at `:73`, `:132`), `rust/analytics/tests/audience_guard_tests.rs`,
  `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs`,
  `rust/analytics-web-srv/tests/audience_grants_tests.rs`,
  `rust/public/tests/read_policy_threading_tests.rs` (compile fixes, plus the `Every` bridge test)
- `analytics-web-app/src/lib/audience-grants-api.ts`, `src/routes/AudienceAccessPage.tsx`,
  `src/lib/__tests__/audience-grants-api.test.ts`, `src/routes/__tests__/AudienceAccessPage.test.tsx`
- `python/micromegas/micromegas/cli/grants.py`, `python/micromegas/micromegas/web_client.py`
- `mkdocs/docs/admin/authorization.md`, `groups.md`, `api-keys.md`, `web-app.md`, `functions-reference.md`,
  `mkdocs/docs/query-guide/python-api.md`
- `CHANGELOG.md`

## Trade-offs

- **New `ReadScope` variant vs. listing audiences at resolve time.** There is no audience
  registry. A `SELECT DISTINCT audience` over `processes` on each request would be costly, and
  would race with audiences created mid-session. The variant states "every audience" directly.
- **`EveryAudience` vs. reusing `All`.** `All` turns off the row-level filter, the call-level
  guard, and the global-partition rule, and it is the internal-caller marker the codebase keeps
  greppable. `EveryAudience` keeps all of that enforcement and changes only the membership test,
  so an unowned process or a global file stays exactly as hidden as for a listed scope.
- **`IS NOT NULL` predicate vs. skipping the rewrite.** Skipping would be marginally cheaper on
  audience-column views. A predicate keeps every branch (semi-join, `EXISTS`, guarded-instance
  skip) identical to the `Audiences` path, so there is no new branch to get wrong.
- **Separate `all_audiences_read` field vs. a `"*"` entry in `entries`.** As an entry, `readers()`
  would yield `"*"` as if it were an audience name, and every iteration site would need to know.
- **Sentinel `"*"` inside `ReadScope::Audiences` vs. an enum variant.** A sentinel fails closed
  (no row has audience `*`), but each consumer would have to special-case it by string. The
  variant makes the compiler list every match site.
- **Drop the CHECKs vs. relax them.** Relaxing `audience_grants_audience_name` to allow `*` on
  read only would mean one more migration now, and another for each future vocabulary change
  (prefix patterns, a new axis of 4 characters or fewer; a longer axis name would need
  `axis` widened). After v14 the grant and group vocabulary lives in code only.
  With §7 in place, a bad direct-SQL row costs one inert row, not an outage.
- **Skip bad rows vs. fail the load.** Failing the load was meant as fail-closed, but in effect it
  turns one bad row into a total outage on cold start, or into a silently frozen last-good
  snapshot (grant revocations stop taking effect, which is fail-*open* for revocation). Skipping is
  fail-closed per row, because rows only grant. The cost is that a typo'd row is inert, not loud.
  The per-refresh `warn!` and the metric make it visible.

## Decisions

- `(*, read, *)` is refused (400) at the route, not offered behind a confirmation. Unrestricted
  read for everyone is reachable deliberately through per-audience `*` grants or
  `public_view_sets`.
- `*`-audience grants are admin-only on create; self-service sharing never produces one.
- Holding the `*` read grant does not let a non-admin share any literal audience:
  `caller_holds_pair` stays a literal match.
- Global (`'global'`) partition rows in `list_partitions` stay admin/public-view-set only under
  `EveryAudience`.
- Prefix patterns (`team-*`) are out of scope.
- Schema v14 drops all three remaining grant/group shape CHECKs (user call). v13 was already
  deployed, so it couldn't be amended. After v14, grant and group vocabulary is validated in code
  only.
- Snapshot loaders and the route-side parses skip malformed rows instead of failing (user call).
  The static JSON grant map (`parse`) stays strict.
- The settings table (`DbOverrides::from_rows`) keeps failing the whole parse on a bad value.
  It is a different trade: a skipped setting falls back to env, which can *widen* exposure (e.g.
  `public_view_sets`), as its own doc comment says.

## Documentation

- `mkdocs/docs/admin/authorization.md`: in Audiences and Grants (~line 66), add that a grant's
  audience may also be `*`, meaning every audience including future ones, read axis only,
  admin only, never with the `*` selector. Add an example
  `micromegas-grants … create '*' read key:<key_id>`. In the grant store schema block (~322),
  delete the `audience_grants_audience_name` line and the inline axis `CHECK`, and say that the
  audience, axis, and selector are validated in code only. In "The grant store" (~331-334), rewrite
  the passage saying a malformed row written directly "fails the whole snapshot load
  loudly", and the following sentence that the audience-name `CHECK` "is re-validated in Rust the
  same way" (false after v14), to say that a malformed row is skipped with a warning and the
  `audience_grant_skipped_row_count` metric, and no longer fails the snapshot. Reword `:11-12` and `:171`, which name
  `ReadScope::Audiences` as the only authenticated/narrowing scope, to include `EveryAudience`.
  In the query-time filtering section, add
  one sentence that a caller holding a `*` read grant is filtered to "any stamped audience"
  (global partition rows still need admin).
- `mkdocs/docs/admin/authorization.md`, "Write gate" **Create** bullet (~362-370): add the `*`
  audience rules: `*` on the mint axis → 400, `(*, read, *)` → 400, and a non-admin creating on
  the `*` audience → 403 (checked before `caller_holds_pair`).
- `mkdocs/docs/admin/groups.md`: delete the `groups_name` CHECK from the schema block, and rewrite
  the passage (~39-42) saying a bad row "fails the snapshot load loudly" to describe
  skip-and-warn (`group_skipped_row_count`). Reword the charset sentence (~:50) that says
  `groups.name` shares `audience_grants`' charset: grant audiences additionally accept `*`.
- `mkdocs/docs/admin/api-keys.md`, "Granting a key read access" (~244): add the `*` option for a
  key that must read everything, with the caveat that it covers audiences created later.
- `mkdocs/docs/admin/web-app.md`, Audience Access (~212): the **All audiences** card, and the
  Add dialog accepting `*` on read.
- `mkdocs/docs/admin/functions-reference.md` (`list_audience_grants()`): the `audience` column
  may be `*`. Also extend the `list_partitions` scope note (~71-78), which only covers a
  `ReadScope::Audiences` session, to callers holding a `*` read grant: `'global'` rows stay hidden
  unless the view set is public or the caller is admin.
- `mkdocs/docs/query-guide/python-api.md`: the `micromegas-grants` CLI section (~955-980), where
  the create/delete examples take an audience argument; add the `*` form.
- `CHANGELOG.md` (Unreleased): the dialog fix, the `*` audience feature, schema
  v14 (drops `audience_grants_audience_name`, the `audience_grants` axis CHECK, and
  `groups_name`), malformed grant and group rows being skipped with a warning and a metric
  instead of failing the snapshot, and a
  **Minor breaking change**: `ReadableAudiences` becomes an enum (`into_inner` removed),
  `ReadScope` gains `EveryAudience`, `AudienceGrants::from_rows` and `GroupGraph::from_rows`
  return `Parsed<_>` instead of `Result`.

## Testing Strategy

No-DB unit tests:

- `rust/auth/tests/policy_tests.rs`
  - `is_valid_grant_audience`: `*` accepted; `**`, `team-*`, and empty rejected; ordinary names
    accepted. `is_valid_audience("*")` stays false.
  - `parse`: `*` on read accepted; `*` on mint rejects the document.
  - `from_rows` skipping: a batch with one valid row and one each of bad audience, `*` on mint,
    `key:` on mint, and bad selector returns the valid grant in `value` and four entries in
    `skipped`. The valid grant still resolves for its caller. The existing `from_rows` rejection
    tests (`policy_tests.rs` ~:582, :593, and `key_selector_is_read_axis_only_in_the_grant_map`
    ~:896-902, which asserts `from_rows(...).is_ok()/.is_err()`) are rewritten to assert on `skipped` instead of
    `expect_err`/`unwrap_err`.
  - `AudienceReadPolicy::resolve` returns `Every` for a caller matching a `*`-audience read
    selector (`user:`, `group:` via memberships, `key:`), from the static map and from
    `from_rows`. A caller matching nothing there gets `Listed` with exactly its literal grants.
    A `*` on the `*` audience matches everyone (the parse layer allows it; the route refuses it).
  - `AudienceMintPolicy` is unaffected: a caller holding `('*', read, user:U)` cannot mint any
    audience.
- `rust/auth/tests/groups_tests.rs`: `GroupGraph::from_rows` with a bad group name, a member row
  under that name, and a bad member selector, next to valid rows. All three are skipped, and the
  closure from the valid rows is unchanged. A skipped `('admins', <bad>)` member never puts
  `admins` in a closure. The existing rejection tests (~:147, :154) are rewritten to assert on
  `skipped`.
- `rust/auth/tests/db_audience_grants_tests.rs`: the `#[ignore]` test
  `live_malformed_row_fails_the_whole_snapshot_load` is inverted: with `'not a valid audience'`
  seeded, `source.current()` succeeds, the malformed row is absent, and a valid row still
  resolves.
- `rust/public/tests/read_policy_threading_tests.rs` (offline): a stub `ReadPolicy` returning
  `ReadableAudiences::Every`, asserting the `processes` EXPLAIN (as in `explain_processes`)
  contains the `IS NOT NULL` audience predicate, so the bridge mapping stays filtered and never
  becomes `All`.
- `report_skipped` is thin glue over the above, so it is covered through `from_rows`. The
  unrecognized-axis skip is tested directly in `db_audience_grants_tests.rs` through the pure
  axis helper.
- `rust/analytics/tests/audience_guard_tests.rs`: the `is_readable` truth table for
  `EveryAudience` (`Audience` → true, `Unknown` → false, `Ambiguous` empty → false, all-known →
  true), and `global_rows_visible` under `EveryAudience` false for a non-admin and a non-public
  view set.
- `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs` (offline plan shape): under
  `EveryAudience`, `streams` (nullable `audience`) gets an `IS NOT NULL` filter (no `IN` list;
  on views with a non-nullable `audience` such as `blocks`, `log_entries`, `measures`, DataFusion's
  simplifier correctly removes the filter, so don't assert it there), a
  semi-join view keeps its `process_id IN (subquery)` with `resolved_audience IS NOT NULL`, and a
  public view set still gets no predicate. These matter because a wrong branch here silently
  leaks or drops rows.
- `rust/analytics-web-srv/tests/audience_grants_tests.rs`, in the existing no-DB harness (checks
  that run before any query): `*` on mint → 400, `(*, read, *)` → 400, non-admin `*` audience
  → 403 (knob on through `test_settings`, following `create_grant_403_for_non_admin_star_selector`),
  and an invalid audience still → 400 with the new message.
- Web app: `audience-grants-api.test.ts` covers `validateGrantAudience`'s cases.
  `AudienceAccessPage.test.tsx` covers: invalid audience text shows the inline error and keeps
  Add grant disabled; `*` on read with a key selector enables Add grant; `*` hides the `everyone` kind;
  `*` on mint shows the mint error; a malformed `key:`/`user:` id shows `selectorError`; the `*`
  card renders as "All audiences" and has no Share button for a non-admin.
- Migration: no new live-DB test (no bug in the wild). The existing `#[ignore]` migration tests
  cover v14 once their assertions are flipped (step 4).

## Manual Verification

These need the real Postgres schema, the grant snapshot, and the flight-sql stack. A failure
would be obvious on the first run.

1. Start the monolith with OIDC configured and ingestion auth on: export
   `MICROMEGAS_OIDC_CONFIG` (as in `local_test_env/ai_scripts/start_services_with_oidc.py`, which
   requires it already set), `MICROMEGAS_STATE_SECRET`, `MICROMEGAS_AUTH_REDIRECT_URI`,
   `MICROMEGAS_WEB_CORS_ORIGIN=http://localhost:3000`, and `MICROMEGAS_APP_SQL_CONNECTION_STRING`
   (`postgres://$MICROMEGAS_DB_USERNAME:$MICROMEGAS_DB_PASSWD@127.0.0.1:$MICROMEGAS_DB_PORT/micromegas_app`,
   as `start_services.py` builds it; the web server exits at startup without the last two), then
   run `cargo run --bin micromegas-monolith -- --roles all --ingestion-listen-addr 127.0.0.1:9000
   --frontend-dir ../analytics-web-app/dist > /tmp/monolith.log 2>&1` from `rust/` with no `--disable-auth` and no `--disable-ingestion-auth`, against a v13 database.
   (`start_services.py --monolith` passes `--disable-auth`, under which every query runs as
   `ReadScope::All` and no grant behavior is visible; with OIDC it passes
   `--disable-ingestion-auth`, so ingestion keys would not stamp an audience.)
   `/tmp/monolith.log` reports data lake schema 14. `\d audience_grants` and `\d groups` show no
   CHECK constraints.
2. As an admin, mint analytics key K and note its `key_id`. Save K to a file and point a
   `~/.micromegas/config.json` profile's `api_key_file` at it. With no grants for K,
   `micromegas-query --profile <that profile>` returns only `public` rows.
3. `micromegas-grants --url http://127.0.0.1:3000 create '*' read key:<K>` → created.
   `create '*' mint key:<K>` → 400. `create '*' read '*'` → 400. As a non-admin with
   self-service on, `create '*' read user:<me>` → 403.
4. Run `micromegas-setup-telemetry --url http://127.0.0.1:3000 --otlp-endpoint
   http://127.0.0.1:9000/ingestion/otlp --name <name> --audience fresh-aud` to mint an ingestion key into a new
   audience, and ingest a process with it. Within the auth cache TTL (default 60 s), K reads
   `fresh-aud` rows (`SELECT DISTINCT audience FROM processes`), with no further grant.
   `list_partitions()` with K still hides `'global'` rows.
5. Open `/audiences`: the **All audiences** card is listed first, showing K's chip. In the Add
   dialog, typing `team alpha` shows the inline charset error, `*` with mint shows the read-only
   error, and `*` with read and a key enables **Add grant**.
6. `micromegas-grants … delete '*' read key:<K>`. Within the auth cache TTL (default 60 s), K no longer reads `fresh-aud`.
7. Resilience: with `psql`, insert `('team alpha', 'read', 'user:x', now(), 'psql')` and
   `('public', 'xyz', '*', now(), 'psql')` into `audience_grants`. Within one TTL, the log
   shows one `warn!` per row naming it. `public` is still readable, and creating and deleting a
   grant still takes effect. Delete the two rows; the warnings stop. This is manual because only
   the real refresh loop shows the end-to-end log-and-keep-serving behavior. The parse logic
   itself is unit-tested above.

## Open Questions

None blocking. The defaults above (refuse `(*, read, *)`, keep `'global'` partition rows
admin-only) are recorded under Decisions and can be revisited before implementation.
