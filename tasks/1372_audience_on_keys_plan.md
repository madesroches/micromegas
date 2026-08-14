# Audience on Ingestion API Keys Plan (#1372)

## Overview

Give every `ingestion_api_keys` row a single, immutable **write audience** and carry it into
`AuthContext.bound_audience`, so Stage 5 (#1373) has an authenticated value to stamp
`micromegas.audience` from. This is **Stage 4** of the AbAC rollout
(`tasks/data_isolation/audience_based_access_control_plan.md` § "Stage 4"), and the migration
vehicle for existing team deployments.

**It also settles what an audience *is*, because this is the last cheap moment to do so.** The
shipped Stage 1 model encodes the principal into the audience value — `user:alice@example.com`,
`group:eng` — and derives a caller's readable set from their own identity by re-deriving those same
strings. This plan replaces that with the model the umbrella plan already records as its target
state:

> **An audience is an opaque label on data. Who may read it is separate configuration that can be
> changed after the fact.**

The prefixed encoding is a shortcut around building a grant relation, and it is paid for in the one
currency this system cannot refund: **immutable history**. Once a process is stamped
`user:alice@example.com`, that data can never be shared with her team, because sharing would mean
restamping already-ingested processes. Under a grant model the same data stays stamped `alice` and
an operator edits one line of config. The umbrella plan's own long-term schema is already
`group_read_grants(group_id, audience TEXT)` **and** `group_mint_grants(group_id, audience TEXT)` —
opaque audience, read and mint grants kept in two separate relations, deliberately never collapsed
into one — so this converges on the recorded end state rather than diverging from it, carrying the
same read/mint split as one env map rather than two tables (§2); it just gets there before any
audience value becomes durable.

Scope beyond that is narrow: one schema migration, one column read on the auth hot path, an audience
on the two insert routes (mint + import), and the CLI/UI surface that makes the column visible and
settable. Nothing *stamps* data yet (#1373) and nothing resolves an audience through `MintPolicy`
yet (#1374, Stage 6).

## Scope note: this amends shipped Stage 1

Steps 1–2 below rewrite `rust/auth/src/policy.rs`'s model (shipped in #1369) and relax
`rust/analytics/src/lakehouse/read_scope.rs`'s validator (shipped in #1370). That is outside a
literal reading of #1372, and it is deliberate:

- **#1372 is the first stage that makes an audience value durable.** It lands in a `NOT NULL` column
  that is immutable by design, and #1373 stamps it onto processes days later. Persisting values in a
  vocabulary we intend to abandon means a data migration instead of a config edit.
- **Enforcement itself is unaffected, but the shipped escape hatch breaks.** `OwnershipRewrite`
  filters on whatever `ReadPolicy::resolve` returns and never inspects an audience's shape, so that
  part is genuinely inert; no knob that activates AbAC is set in any deployment (activation is Stage
  7). But Stage 2 ships an escape-hatch pair — `MICROMEGAS_IMPLICIT_GROUPS=everyone` plus
  `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` — that every auth-enabled deployment is documented
  to set to avoid zero visible rows (`CHANGELOG.md`, `tasks/completed/1370_ownership_rewrite_plan.md`).
  This plan deletes `MICROMEGAS_IMPLICIT_GROUPS` outright, and the relaxed `[A-Za-z0-9_-]` validator
  makes the surviving `group:everyone` value hard-fail startup (`OwnershipRewriteConfig::from_env` is
  invoked with `?` in both `flight_sql_server.rs` and `monolith/src/main.rs`). So this is an
  operator-facing break, not a no-op: every deployment relying on that pair must re-set
  `MICROMEGAS_UNSTAMPED_AUDIENCE` to an opaque name (e.g. `public`) in the same deploy that picks up
  this change, which is exactly what the upgrade note and CHANGELOG entry below call out.
- The cost is roughly 150 lines of policy code plus its tests, and a `group:everyone`/prefix sweep
  through docs and comments.

If it should be split, the natural cut is a companion issue amending #1369's model that lands
immediately before this one; the rest of this plan is unchanged either way. Flagged in
[Open Questions](#open-questions).

## Current State

### The key store (Stage 0, #1383 / #1411 / #1458)

- `rust/ingestion/src/sql_migration.rs:100-141` — migration **v5** creates `ingestion_api_keys` and
  `analytics_api_keys`: `key_id UUID PK`, `key_hash BYTEA` (unique index),
  `name`/`created_at`/`created_by`/`last_used_at`/`revoked_at`/`revoked_by`. No `audience` column.
  `LATEST_DATA_LAKE_SCHEMA_VERSION` is 5; `execute_migration` (`:201-258`) is the version-stepping
  chain and ends in `assert_eq!(current_version, LATEST_DATA_LAKE_SCHEMA_VERSION)`.
- `rust/auth/src/db_api_key.rs` — `DbApiKeyAuthProvider`, parameterized by `ApiKeyTable`
  (`Ingestion` | `Analytics`, `:28-46`). `validate_request` (`:247-349`) hashes the bearer token,
  consults a negative cache, then runs, inside a `moka` `try_get_with` loader:
  `UPDATE <table> SET last_used_at = now() WHERE key_hash = $1 AND revoked_at IS NULL RETURNING
  key_id, name` (`:272-275`). The resulting `AuthContext` (`:317-332`) sets `is_admin: false`,
  `allow_delegation: true`, `bound_audience: None`, `read_audiences: vec![]`, `groups: vec![]`.
- `rust/analytics-web-srv/src/ingestion_keys.rs` — the **only** write surface for
  `ingestion_api_keys`: `mint_key` (`:170-212`), `list_keys` (`:235-285`), `revoke_key` (`:302-330`),
  `import_key` (`:375-441`). Both insert sites list their columns explicitly (`:186-189`,
  `:393-398`). State is `IngestionKeysState { pool: Option<PgPool> }` (`:51-54`), built in
  `web_server.rs:643-645`.
- `python/micromegas/micromegas/cli/import_keys.py` — `micromegas-import-keys`, reads the legacy
  `[{"name","key"}]` keyring and POSTs each entry to `.../import` via
  `WebClient.import_ingestion_api_key(name, key)` (`web_client.py:99`).
- `analytics-web-app/` — `ApiKeysAdminPage.tsx` (shared table + mint dialog, config-driven,
  `ApiKeysAdminPageConfig` at `:32-48`), `lib/api-keys-shared.ts`, `lib/ingestion-api-keys-api.ts`,
  `routes/IngestionApiKeysPage.tsx`.

### The audience model as shipped (Stage 1, #1369 / Stage 2, #1370)

- `policy.rs:89-104` — `identity_and_group_audiences` builds
  `{user:<email>} ∪ {group:<g> : g ∈ claim} ∪ {group:<g> : g ∈ MICROMEGAS_IMPLICIT_GROUPS}`, shared
  by `AudienceReadPolicy::resolve` (`:208-215`) and `AudienceMintPolicy`'s non-admin arm
  (`:263-275`). **The readable set is derived from the caller's identity, not looked up.**
- `policy.rs:109-113` — `is_well_formed_audience`: `user:`/`group:` prefix, non-empty after it.
  `read_scope.rs:106-110` holds a deliberate second copy gating `MICROMEGAS_UNSTAMPED_AUDIENCE`
  (`:167-181`).
- `policy.rs:134-159` — `parse_implicit_groups`, the `MICROMEGAS_IMPLICIT_GROUPS` comma-separated
  parser.
- `ownership_rewrite.rs` — consumes the resolved `ReadScope` as an opaque list of strings; it
  injects `audience IN (…)` and **never inspects an audience's shape**. This is why the model change
  below is invisible to enforcement.
- `types.rs:59-77` — `bound_audience: Option<String>` (`None` for every principal today),
  `read_audiences: Vec<String>` (Stage 4b's per-key direct grant), `groups: Vec<String>` (raw IdP
  claim values, not yet namespaced).
- The umbrella plan's [long-term model] already specifies
  `group_read_grants(group_id UUID, audience TEXT)` and `group_mint_grants` — opaque audiences,
  grants in their own relation, read and mint separated.

## Design

### 1. Audiences become opaque labels

An audience is a **name for a bucket of data**: `public`, `team-alpha`, `payments-svc`,
`alice-laptop`. It carries no principal semantics, and no code derives one from an identity.

`policy.rs`:

```rust
/// The reserved audience every authenticated principal may read.
pub const PUBLIC_AUDIENCE: &str = "public";

/// An audience is an opaque label. Two constraints, both about the name and neither about
/// meaning: `[A-Za-z0-9_-]{1,255}` (255 = the `ingestion_api_keys.audience` column width), and
/// uniqueness. The character set exists so that an audience never needs escaping in any
/// encoding it passes through -- config, SQL, JSON, a URL path segment, a log line -- rather
/// than because any one of those needs it today.
///
/// Deliberately **not** normalizing: no case folding, no trimming. The value is stored and
/// compared verbatim, which is what makes uniqueness meaningful -- `team-alpha` and `Team-Alpha`
/// are two audiences, and neither is a typo the system silently repairs into the other.
pub fn is_valid_audience(aud: &str) -> bool;
```

- **Escaping is a non-issue by construction**, and it already was on the enforcement path:
  `OwnershipRewrite` builds its predicate from `lit(ScalarValue::Utf8(..))`
  (`ownership_rewrite.rs:197,204`) — logical-plan literals, never interpolated SQL text. The charset
  makes that belt-and-braces instead of load-bearing, and keeps every *future* consumer (a URL, a
  CLI flag, a comma-separated knob) free of the question.
- **Uniqueness is byte-exact identity, enforced where audiences are enumerated** — today the grant
  map's JSON keys (unique by construction). If an audience registry table lands later (the natural
  home for a description or an owner), it carries a `UNIQUE` index on the name; nothing else changes,
  because every consumer already compares verbatim.
- **An email is not a valid audience name** (`@` and `.` are out), which is what removes the
  self-audience rule below. That is the one place this charset changes the design rather than just
  the validator.

`read_scope.rs`'s copy (crate-boundary duplication, already deliberate and documented at `:112-118`)
gets the same relaxation, so `MICROMEGAS_UNSTAMPED_AUDIENCE=public` — or `=team-alpha` — validates.

What this buys, concretely: data ingested by alice's laptop is stamped `alice-laptop`. Six
months later her team needs it. Under the shipped model that is impossible without restamping
history. Under this one it is a grant edit, and every already-ingested process becomes visible
immediately, because the *data* never encoded who could see it.

### 2. Access is a grant map, with a read/mint axis — `{prefix}_AUDIENCE_GRANTS`

A JSON object, **keyed by audience**, reading as "who can access this audience" — it is a map, and
JSON is this codebase's encoding for structured config (`MICROMEGAS_ADMINS`,
`MICROMEGAS_OIDC_CONFIG`). Its keys are unique by construction, which is where audience uniqueness
is enforced today.

Each value carries an explicit **intent axis**, not one list consulted by both policies: a bare
array is shorthand for **read-only** selectors (the common case, and the only thing most audiences
need), and an object form, `{"read": [...], "mint": [...]}`, adds an explicit mint list when one is
needed. This makes the map a 1:1 stand-in for the umbrella plan's two grant relations,
`group_read_grants(group_id, audience)` and `group_mint_grants(group_id, audience)`
(`audience_based_access_control_plan.md`, "Read and write finally separate") — one relation per
axis, kept in a single env map for now only because there is no store yet to split them across two
tables. That section calls re-collapsing the two relations into one "a security regression relative
to the read-only phrasing"; the shorthand form here can't become that regression, which is why an
omitted `"mint"` list is always empty, never defaulted from `"read"`.

Resolved as `{prefix}_AUDIENCE_GRANTS`, falling back to unprefixed `MICROMEGAS_AUDIENCE_GRANTS`
when the prefixed name is unset — the same convention as every peer knob `AudienceReadPolicy::
from_env(prefix)` already resolves (`implicit_groups_var`, `resolved_var` for
`UNSTAMPED_AUDIENCE`/`PUBLIC_VIEW_SETS`, `DbApiKeyConfig::from_env_with_prefix`, `admin_var`), via
a new `audience_grants_var(prefix: &str)` helper of the same shape:

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

`public` and `team-alpha` above use the bare-array shorthand: read-only grants, no mint authority.
`alice-laptop` needs alice herself to be able to mint into it, so it spells out both lists.

Principal selectors **keep their prefixes**, and that is where prefixes belong — they say which
identity axis to match, on either list:

| Selector | Matches |
|---|---|
| `*` | any authenticated principal |
| `user:<email>` | `AuthContext.email` |
| `group:<g>` | any raw value in `AuthContext.groups` (the IdP claim) |

**`public` is the only built-in**: readable by every authenticated principal. It is expressible as
`{"public": ["*"]}`, but built in so an operator who writes a grant map without it doesn't silently
hide legacy data.

There is **no self-audience rule** — no "you may read the audience named after you". Two reasons,
and the first is the charset:

- An email cannot be an audience name under `[A-Za-z0-9_-]`, so the rule would need a derivation
  (`alice@example.com` → `alice_example_com`), and a lossy one: `a.b@x.com` and `a_b@x.com` collide
  into the same audience. A collision here is two people reading each other's data.
- Keying it on `AuthContext.subject` instead avoids the charset problem and introduces a worse one:
  `subject` is the **key name** for API-key principals (`db_api_key.rs:318`), so an admin who names a
  key `team-alpha` would mint themselves read access to the `team-alpha` audience. `email` was
  immune to that only because keys never carry one.

So a personal audience is an ordinary audience with an ordinary grant — `"alice-laptop":
["user:alice@example.com"]`. The cost is one grant entry per user instead of zero; the mitigation is
that Stage 6 (#1374) already mints a personal key per user through a route, and creating the grant in
that same flow is a natural extension of it rather than new machinery. See
[Open Questions](#open-questions).

`AudienceReadPolicy::resolve(caller)` is therefore a pure lookup over each audience's **read** list
(the whole array for the bare-array shorthand, the `"read"` field for the object form), with no
derivation anywhere:

```text
{ PUBLIC_AUDIENCE }
∪ { a : "*"            ∈ grants[a].read }
∪ { a : "user:<email>" ∈ grants[a].read }               if email present
∪ { a : "group:<g>"    ∈ grants[a].read for some g ∈ caller.groups }
∪ caller.read_audiences                                 (Stage 4b per-key direct grant)
```

`AudienceMintPolicy` resolves over the **separate** `grants[a].mint` list instead — never derived
from `.read`, which is what keeps the two axes independent rather than re-collapsing them: a
bare-array audience has an empty mint list by construction, so a read grant confers no mint
authority (the existing asymmetry, unchanged), and being able to *read* `public`, which every
authenticated principal is, does not imply being able to *mint into* it unless some grant names
`public` in a `"mint"` list — the built-in-readability rule is a read-side convenience, not a
blanket publish grant. `read_audiences` never enters the mintable set; `is_admin` callers may mint
any valid audience name, `public` included. `read_audiences` needs no rework: it is already a
principal-level direct grant, which the target model keeps as a first-class case.

**`MintPolicy::resolve_audience(caller, requested: None)` now returns `Err`** ("no audience
requested and none can be defaulted"), for every caller, admin or not. The shipped trait doc
comment reads `None` as "mint a key scoped to myself" and both policy arms implement that via
`default_self_audience`, an email derivation (`policy.rs:115-121`) that §2 removes along with every
other identity-derived audience; there is no replacement derivation, because under an opaque-label
model there is no "myself" audience to default to. The trait doc comment is updated to say so. This
is consistent with, not new work on top of, §5's route-level `resolve_audience` helper, which
already treats an absent audience for `mint` as `state.default_audience` then a `400` — Stage 6
(#1374) wires the route helper to call the trait method instead of duplicating its logic, and both
now agree that an unresolvable `None` is an error, not a silent default.

**`MICROMEGAS_IMPLICIT_GROUPS` is removed**, subsumed by `{"<name>": ["*"]}`. It shipped one release
ago, is inert without activation, and keeping two knobs that both mean "everyone reads this" is the
confusion this rewrite exists to end. Env-var churn pre-GA is acceptable — the stability contract
covers the SQL surface, not deployment config — but it belongs in the CHANGELOG as an operator-facing
break.

Grant resolution stays `async` and fallible exactly as the trait already requires, so replacing the
env map with the `group_read_grants` store later changes the body of one function and nothing else.

### 3. Schema — migration v6

`rust/ingestion/src/sql_migration.rs`: new `upgrade_data_lake_schema_v6`,
`LATEST_DATA_LAKE_SCHEMA_VERSION` bumped to 6, new `if 5 == current_version { … }` arm in
`execute_migration`:

```sql
ALTER TABLE ingestion_api_keys ADD COLUMN audience VARCHAR(255);
UPDATE ingestion_api_keys SET audience = 'public' WHERE audience IS NULL;
ALTER TABLE ingestion_api_keys ALTER COLUMN audience SET NOT NULL;
ALTER TABLE ingestion_api_keys ADD CONSTRAINT ingestion_api_keys_audience_name
  CHECK (audience ~ '^[A-Za-z0-9_-]+$');
```

- **A column, not a mapping table** (umbrella plan §Stage 4 step 9): the key→audience binding is 1:1
  and immutable, so `NOT NULL` is fail-closed by construction and the auth hot path stays join-free.
  Note this is *not* in tension with §2 — the mutable, reconfigurable part is audience→principals,
  which is exactly what moved out into grants.
- **Backfill to `public`, then `SET NOT NULL`**, in that order, so the migration works on a table
  that already has rows. Every existing row is a pre-AbAC key: nothing is stamped, nothing is
  enforced, and everything it has ingested is visible to every caller. `public` is the *accurate*
  description of that state, not a new grant — which is why it is safe to apply unattended.
- A **literal**, not an env read: a migration's result must not depend on which process (ingestion /
  monolith / maintenance) ran it, and every other migration literal in this file is likewise frozen
  history.
- The `CHECK` mirrors `is_valid_audience` for hand-written `INSERT`s and the `psql` runbook —
  `VARCHAR(255)` already caps the length, so the regex carries the character set. It stays in step
  with the Rust predicate by being the same two rules stated in the same place in the docs; there is
  no third definition of "valid audience" anywhere.
- `analytics_api_keys` is untouched: its read-side mirror is `read_audiences` (Stage 4b), a
  set-valued grant in the opposite direction.
- **`NOT NULL` with no default breaks the opposite deployment order too.** The migration ships in
  `telemetry-ingestion-srv`/monolith, but `analytics-web-srv`'s mint/import `INSERT`s list columns
  explicitly (`ingestion_keys.rs:186-189`, `:393-398`) and — pre-#1372 — omit `audience`. In a split
  deployment where the two are separate processes, running the migration before rolling
  `analytics-web-srv` means every mint/import against the now-v6 schema hits the `NOT NULL`
  constraint until the web service is upgraded. The [Documentation](#documentation) upgrade note
  below states the required order: roll `analytics-web-srv` to this change in the same deploy that
  runs the v6 migration, not before and not after. Ingestion (writing telemetry payloads) and key
  *validation* (`db_api_key.rs`, §4) never insert this column, so both are unaffected regardless of
  ordering.

### 4. Auth provider — carrying the audience

`rust/auth/src/db_api_key.rs`:

```rust
impl ApiKeyTable {
    /// Whether this table carries Stage 4's write-side `audience` column.
    pub fn has_audience(self) -> bool { matches!(self, ApiKeyTable::Ingestion) }
}

struct KeyRow {
    key_id: uuid::Uuid,
    name: String,
    /// `Some` for `Ingestion` (the column is `NOT NULL`), `None` for `Analytics`.
    audience: Option<String>,
}
```

The loader's `RETURNING` list is built from `&'static str` literals alongside `table_name()`, so
nothing caller-supplied reaches the SQL:

```rust
let returning = if table.has_audience() { "key_id, name, audience" } else { "key_id, name" };
```

The produced `AuthContext` changes twice:

- `bound_audience: row.audience.clone()` — `Some(..)` for every ingestion key, `None` for analytics
  keys (unchanged there).
- `allow_delegation: !self.table.has_audience()` — **false for ingestion keys**, unchanged (`true`)
  for analytics keys. `allow_delegation` only governs `x-user-*` attribution on the gRPC path
  (`user_attribution.rs:163`, reached solely from `flight_sql_service_impl.rs:636`), which an
  ingestion key can never reach — it lives in the other table. An ingestion write credential is not a
  delegating service account.

**`email` stays `None`** (it already is: `db_api_key.rs:319`) — a deliberate deviation from the
umbrella plan's step-9 sketch (`AuthContext { …, email: Some(…), … }`), settled rather than left
open. Under §2, `email` is what `user:` grant selectors match on; populating it from `created_by`
would hand an ingestion key every audience granted to the minting admin. The only other consumer of
a principal's email is `OidcAuthProvider::is_admin` (`oidc.rs:397-400`), which is OIDC-only and
never sees an API-key `AuthContext`. If a key's *owner* is ever needed (e.g. for attribution or UI
display), that is Stage 6 (#1374) territory and wants its own `ingestion_api_keys` column rather
than reusing `created_by` as an email.

The audience is cached with the row; since it is immutable that is free. A hand edit takes effect
within `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS` — the same stated property as revocation.

### 5. The default-audience knob, and why `mint` has no built-in default

New in `policy.rs`, resolved **once at startup** so a typo fails fast:

```rust
/// Resolves `{prefix}_DEFAULT_KEY_AUDIENCE`, falling back to unprefixed
/// `MICROMEGAS_DEFAULT_KEY_AUDIENCE` when the prefixed name is unset -- the same
/// `implicit_groups_var`-style convention as every other knob `AudienceReadPolicy::from_env`
/// resolves. `None` when neither is set. Invalid ⇒ `Err`.
pub fn default_key_audience_from_env(prefix: &str) -> Result<Option<String>>;
```

```rust
// web_server.rs
let ingestion_keys_state = ingestion_keys::IngestionKeysState {
    pool: analytics_keys_pool,
    default_audience: micromegas::auth::policy::default_key_audience_from_env("")?,
};
```

The two insert routes differ **only when the knob is unset**, and that asymmetry is the point:

| Route | Explicit `audience` | Knob set | Knob unset |
|---|---|---|---|
| `mint` (new credential) | used | knob value | **400** — "no audience configured" |
| `import` (legacy key carried forward) | used | knob value | `public` |

- `mint` must never silently default to `public`. A privacy deployment that forgets the knob would
  otherwise mint credentials publishing every process they ingest to every authenticated caller — a
  fail-open default, and exactly the failure a universally-readable audience invites. Requiring an
  explicit choice matches the umbrella plan's activation story ("every operator makes a conscious
  choice").
- `import` defaults to `public`, but that default is only a *default* — an explicit `audience`
  field or the `MICROMEGAS_DEFAULT_KEY_AUDIENCE` knob both still override it, exactly as for
  `mint`. `public` earns the fallback slot because continuity is the safe assumption for a legacy
  key: for a key imported at migration time, its already-ingested history is what the v6 backfill
  just set to `public`, so defaulting the *new* rows to the same value keeps one key's data under
  one audience rather than splitting its history in two. For a key imported well after the
  migration, there is no prior history to preserve, but `public` is still the least-surprising
  no-decision default for "a pre-existing key with no configured audience" — the same shape of
  default `mint` deliberately refuses to make for a *new* credential's *entire future*, but here the
  data may already be flowing unstamped.

An open deployment therefore sets `MICROMEGAS_UNSTAMPED_AUDIENCE=public` and
`MICROMEGAS_DEFAULT_KEY_AUDIENCE=public` and is done — no grant map needed at all, since `public` is
built in.

### 6. Routes — `ingestion_keys.rs`

Both insert sites must supply the `NOT NULL` column (umbrella plan step 10: "every insert site must
supply an audience").

- `MintRequest { name, audience: Option<String> }`,
  `ImportRequest { name, key, audience: Option<String> }`.
- One shared helper implements the table above:

  ```rust
  fn resolve_audience(
      state: &IngestionKeysState,
      requested: Option<&str>,
      fallback: Option<&str>,   // None for mint, Some(PUBLIC_AUDIENCE) for import
  ) -> Result<String, IngestionKeyError>
  ```

  A missing field or an empty string counts as absent (the empty string is not a name — it fails
  `is_valid_audience` either way); anything else is taken **verbatim**, no case folding, so
  the audience an operator typed is the audience that gets stored. It then applies
  `state.default_audience`, then `fallback`, errors `BadRequest` when nothing resolves, and validates
  with `is_valid_audience`. Both routes are
  `AdminUser`-gated, and `AudienceMintPolicy`'s admin arm accepts any valid audience — so this is the
  same decision Stage 6 will make through the policy, and Stage 6 replaces the helper body with
  `MintPolicy::resolve_audience` without changing either request shape. The optional `audience` on
  `mint` is in scope now, not deferred to #1374: `mint_key`'s `INSERT` must supply the `NOT NULL`
  column regardless (`ingestion_keys.rs:186-189`), the Testing Strategy already requires an
  explicit per-key override to work end to end, and without a request field a per-key audience
  would mean restarting `analytics-web-srv` with a different `MICROMEGAS_DEFAULT_KEY_AUDIENCE` for
  every mint — defeating §1's motivating scenario. Keeping the field now is what lets Stage 6 change
  only the helper's *body*.
- `MintResponse` / `ImportResponse` / `KeyListEntry` each gain `audience`. On import's already-present
  (`imported: false`) path the response reports the **existing** row's audience — the audience is
  immutable, so an import never rewrites it; both branches already share `ImportedRow`, which gains
  the field.
- `revoke_key` is unchanged.

### 7. CLI + Python client

- `WebClient.import_ingestion_api_key(name, key, audience=None)` — omits the field when `None`.
  `import_analytics_api_key` is untouched.
- `micromegas-import-keys`:
  - `--audience AUD`, valid only with `--table ingestion` (`parser.error` otherwise —
    `analytics_api_keys` has no such column).
  - Per-key choice: a keyring entry may carry an optional `"audience"` field, which wins over
    `--audience`. `read_keyring` returns `(name, key, audience)` triples unconditionally — the field
    is read regardless of `--table` — and a non-string `audience` is a `parser.error` like the other
    field validations. The new triple arity ripples through every other function that destructures
    `read_keyring`'s output: `select_entries` (`--only`/`--exclude` filtering), `import_one`, and
    `run_import`'s per-key loop.
  - **A per-entry `"audience"` combined with `--table analytics` is a `parser.error`, same as the
    `--audience` flag form** — one entry-level check in `main`, right after `read_keyring` and
    before entries reach `select_entries`/`run_import`, so a keyring built for ingestion isn't
    silently reused against the analytics table with its audience dropped, and so the batch is
    rejected up front rather than partway through (`main` already holds `parser`, which the check
    needs for `parser.error`; `run_import(client, table, entries)` receives neither). `import_one`
    reflects this split: it passes `audience` to
    `WebClient.import_ingestion_api_key(name, key, audience)` only on the ingestion branch, and calls
    `import_analytics_api_key(name, key)` (no `audience` parameter) on the analytics branch, since
    that call is only ever reached once the per-entry check above has already rejected a non-`None`
    audience for that table.
  - Neither given ⇒ the field is omitted and the server applies `public` — the zero-decision path.
  - `run_import`'s per-key line gains the audience the server reports.

### 8. Web app

- `api-keys-shared.ts`: `ApiKeyListEntry` and `MintApiKeyResponse` gain `audience?: string`
  (optional — analytics rows never carry one); `mint(name, audience?)` sends the field only when set.
- `ApiKeysAdminPageConfig` gains `showAudience?: boolean`: an **Audience** table column plus an
  audience input in the mint dialog (placeholder `public`, helper text naming
  `MICROMEGAS_DEFAULT_KEY_AUDIENCE`). `IngestionApiKeysPage.tsx` sets it; the analytics page is
  untouched.
- The 400 a mint gets with neither an explicit audience nor the knob surfaces through the existing
  `ErrorClass` path, and its message names the knob to set.

### Flow after this stage

```
config:  {prefix}_AUDIENCE_GRANTS = {"team-alpha": ["group:eng"]}   ← editable, after the fact
                                                    │
mint   (AdminUser)  audience? ─yes→ is_valid_audience ─→ INSERT audience ('team-alpha')
                              └no─→ MICROMEGAS_DEFAULT_KEY_AUDIENCE  └→ unset: 400
import (AdminUser)  audience? ─no─→ knob ─→ unset: 'public'   (continuity with the v6 backfill)
                                                    │
ingestion request: Bearer <key> ────────────────────┼──→ DbApiKeyAuthProvider (Ingestion)
                                                    │      UPDATE … RETURNING key_id, name, audience
                                                    └──→ AuthContext { bound_audience: Some(aud),
                                                             allow_delegation: false, is_admin: false }
                                                                │
                                      (#1373, Stage 5) stamps micromegas.audience = 'team-alpha'
                                                                │
                                      (#1370, Stage 2) OwnershipRewrite filters on it against
                                                       AudienceReadPolicy's grant lookup
```

## Implementation Steps

1. **Opaque audiences.** `policy.rs`: `is_valid_audience` replacing `is_well_formed_audience`,
   `PUBLIC_AUDIENCE`. `read_scope.rs`: the same relaxation in its copy. This inverts the premise of
   four existing tests in `rust/analytics/tests/ownership_rewrite_config_tests.rs`
   (`malformed_unstamped_audience_is_rejected`, `well_formed_unstamped_audience_is_accepted`,
   `prefixed_unstamped_audience_wins_over_unprefixed_fallback`,
   `unprefixed_unstamped_audience_used_when_prefixed_is_unset`) — see Testing Strategy.
2. **Grant map.** `policy.rs`: `AudienceGrants` (parse + lookup for `{prefix}_AUDIENCE_GRANTS`,
   via a new `audience_grants_var(prefix)` helper), rewrite `AudienceReadPolicy::resolve` and
   `AudienceMintPolicy::resolve_audience` around it, delete `identity_and_group_audiences`,
   `parse_implicit_groups`, `default_self_audience`, `is_well_formed_audience`, and
   `default_provider::implicit_groups_var`.
3. **Migration v6.** `sql_migration.rs`: `upgrade_data_lake_schema_v6`, bump
   `LATEST_DATA_LAKE_SCHEMA_VERSION`, add the `if 5 == current_version` arm.
4. **Provider.** `db_api_key.rs`: `ApiKeyTable::has_audience`, `KeyRow.audience`,
   table-conditional `RETURNING`, `bound_audience` + `allow_delegation`; refresh the doc comments
   that say "no Stage 4 grant yet".
5. **Routes.** `ingestion_keys.rs`: `default_audience` on the state, `resolve_audience`, `audience`
   on both requests, both `INSERT`s, `ImportedRow`, `KeyListEntry`, all three responses.
   `web_server.rs:643`: resolve the knob at startup.
6. **Python.** `web_client.py`; `cli/import_keys.py`: `read_keyring`'s new `(name, key, audience)`
   triple and every site that destructures it — `select_entries`, `import_one`, `run_import`.
7. **Web app.** `api-keys-shared.ts`, `ingestion-api-keys-api.ts`, `ApiKeysAdminPage.tsx`,
   `IngestionApiKeysPage.tsx`.
8. **Vocabulary sweep.** `group:everyone` / `user:`-prefixed examples out of
   `monolith/src/main.rs:252`, `policy.rs` doc comments, `ownership_rewrite.rs` module docs,
   `read_scope.rs`'s own doc comments (`:30`, the `ReadScope::Audiences` variant doc's
   `"user:<email>" / "group:<id>"` example; `:90`, `unstamped_audience`'s `"group:everyone"`
   example; and `:112-118`, `parse_comma_separated_list`'s doc comment, which also loses its
   dangling cross-reference to `micromegas_auth::policy`'s `MICROMEGAS_IMPLICIT_GROUPS` parser once
   step 2 deletes `parse_implicit_groups` and the knob — restate the comparison against the new
   `AudienceGrants` parser or drop the cross-reference), the Unreleased Stage 2 CHANGELOG entry (the
   Stage 1 entry is released and stays untouched), the umbrella plan, and
   `mkdocs/docs/admin/{flight-sql, monolith}.md`'s env-var tables (see
   [Documentation](#documentation)).
9. **Tests**, then **docs + CHANGELOG**, per the sections below.

## Files to Modify

| File | Change |
|---|---|
| `rust/auth/src/policy.rs` | opaque audiences, grant map, both policies rewritten |
| `rust/auth/src/default_provider.rs` | drop `implicit_groups_var` |
| `rust/analytics/src/lakehouse/read_scope.rs` | validator relaxed to `is_valid_audience`; doc-comment vocabulary sweep (`:30`, `:90`, `:112-118` — the last drops or restates its dangling reference to the deleted `MICROMEGAS_IMPLICIT_GROUPS` parser) |
| `rust/ingestion/src/sql_migration.rs` | migration v6 |
| `rust/auth/src/db_api_key.rs` | `has_audience`, `KeyRow.audience`, `bound_audience`, `allow_delegation` |
| `rust/auth/src/types.rs` | doc comments on `bound_audience` / `groups` |
| `rust/analytics-web-srv/src/{ingestion_keys,web_server}.rs` | audience on mint/import/list; knob at startup |
| `rust/monolith/src/main.rs` | config comment |
| `rust/public/src/servers/flight_sql_server.rs` | update both `AudienceReadPolicy::new(vec![])` call sites (injected-provider and disabled-auth default-policy branches) and their doc comments (`:143`, `:270`) for the grant-map constructor |
| `python/micromegas/micromegas/{web_client.py,cli/import_keys.py}` | `audience` param, `--audience` |
| `analytics-web-app/src/lib/{api-keys-shared,ingestion-api-keys-api}.ts` | types + `mint(name, audience?)` |
| `analytics-web-app/src/components/ApiKeysAdminPage.tsx` | `showAudience` column + input |
| `analytics-web-app/src/routes/IngestionApiKeysPage.tsx` | `showAudience: true` |
| `rust/auth/tests/{policy,db_api_key,default_provider}_tests.rs`, `rust/analytics-web-srv/tests/{ingestion_keys,routing}_tests.rs`, `rust/analytics/tests/*ownership_rewrite*`, `analytics-web-app/src/**/__tests__/*`, `python/**/tests` | per Testing Strategy; `routing_tests.rs:405`'s `IngestionKeysState { pool: None }` literal needs the new `default_audience` field |
| `rust/public/tests/read_policy_threading_tests.rs` | update `AudienceReadPolicy::from_env("MICROMEGAS_1369_THREADING_TESTS_UNSET")` call/assertions for the grant-map constructor |
| `rust/ingestion/tests/sql_migration_test.rs` (new) | live-DB, `#[ignore]`d migration v6 coverage — see Testing Strategy |
| `mkdocs/docs/admin/{api-keys,authentication}.md` | audiences + grants + DDL + CLI |
| `mkdocs/docs/admin/{flight-sql,monolith}.md` | remove `MICROMEGAS_IMPLICIT_GROUPS`/`MICROMEGAS_ANALYTICS_IMPLICIT_GROUPS` rows, restate `UNSTAMPED_AUDIENCE`'s format as an opaque label (not `user:<id>`/`group:<id>`), add a `{prefix}_AUDIENCE_GRANTS` row, add an upgrade note that the previously-recommended `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` now fails startup under the relaxed charset; `monolith.md` additionally gets the `MICROMEGAS_DEFAULT_KEY_AUDIENCE` row (its `web` role reads the knob) — `flight-sql.md` does not, since `flight-sql-srv` never reads it |
| `mkdocs/docs/admin/web-app.md` | add the `MICROMEGAS_DEFAULT_KEY_AUDIENCE` row to the Environment Variables → Optional table (`:55-61`) — this is the service that actually resolves the knob for `IngestionKeysState` |
| `CHANGELOG.md` | new Unreleased entry; amend the Unreleased Stage 2 (#1370) entry only — the Stage 1 (#1369) entry is in the released `v0.29.0` section and stays untouched |
| `tasks/data_isolation/audience_based_access_control_plan.md` | model change recorded; Stage 4 landed |

## Trade-offs

- **Opaque audiences + grants vs. the shipped prefixed encoding.** The encoding needs no grant
  relation, which is why Stage 1 chose it; it pays for that by making access a property of the data
  itself. Since data is immutable and grants are not, that is the wrong thing to freeze. Cost of
  changing now: ~150 lines of shipped policy code, its tests, and a docs sweep. Cost of changing
  after #1373 ships: a restamping migration over already-ingested processes, which the plan
  elsewhere rules out as impractical (§"Query-time coalesce ... vs. a backfill script").
- **Grant map in env vs. going straight to the `group_read_grants`/`group_mint_grants` store.** The
  store is the recorded end state and needs nested-group closure, cycle handling, cached resolution
  with a stated latency, and an admin CRUD surface — a stage of its own. The env map keeps the same
  read/mint split as the two tables (§2's bare-array shorthand is the read axis; an explicit `"mint"`
  list is the mint axis) rather than collapsing back to one relation consulted by both policies, which
  the umbrella plan calls a security regression relative to the read-only phrasing — so the store
  replaces one function body per axis, not the whole map. What matters is that no third grant
  mechanism appears later; this is mechanism #2 of the two the umbrella plan permits (principal-level
  and group-level).
- **`[A-Za-z0-9_-]` vs. length-only.** Length-only is the minimal rule for an opaque label, and the
  enforcement path is already escape-safe (`ScalarValue` literals). The charset is chosen anyway so
  that no *future* consumer — a URL segment, a CLI flag, a comma-separated knob, a filesystem or
  object-store prefix if audiences ever become a physical boundary (umbrella plan step 15) — has to
  re-open the question. Cost: emails are not valid names, which is what removes the self-audience
  rule.
- **`public` built in, everything else granted.** One built-in rule instead of two: the alternative
  (a self-audience rule) needs either a lossy email derivation or a `subject` key that lets an
  admin mint read access by naming a key after an audience. Cost: per-user isolation needs a grant
  per user rather than zero config. See [Open Questions](#open-questions).
- **`mint` requires a choice, `import` defaults to `public`** — different subjects: a new credential
  has no prior visibility to preserve (defaulting it would publish), while `import`'s default is a
  continuity assumption for a pre-existing key, overridable like any other default; for a key
  imported at migration time it also matches what the v6 backfill just stamped its existing rows
  with, though that specific coincidence doesn't hold for an import performed later.
- **Backfill literal vs. reading the knob in the migration.** Reading env inside a migration makes
  stored content depend on which process ran it, and would duplicate the resolver into
  `micromegas-ingestion`, which does not (and should not) depend on `micromegas-auth`.
- **No `audience` on the env keyring** (`MICROMEGAS_API_KEYS`), per the umbrella plan. The
  consequence must be documented, not left implicit: **data ingested with an env-keyring key is never
  stamped**, so it is visible only through `MICROMEGAS_UNSTAMPED_AUDIENCE`.

## Documentation

- `mkdocs/docs/admin/api-keys.md` — DDL block (`:47-70`) gains `audience` + the `CHECK`; mint/import
  bodies gain the field; the `micromegas-import-keys` section gains `--audience` and the per-entry
  keyring field; a new **"What audience does a key carry"** section: audiences are opaque labels,
  `public`'s meaning, immutability of the binding, the `MICROMEGAS_DEFAULT_KEY_AUDIENCE` knob and why
  `mint` has no built-in default, the env-keyring consequence, and the cache-TTL latency of a hand
  edit. The grant recipe (`:304-318`) needs no change.
- **Migration ordering**, same page: `analytics-web-srv` writes these rows but never runs the
  telemetry-DB migration — mint/import return 500 until ingestion or the monolith has taken the
  schema to v6. Same wording as the v5 note in `default_provider.rs:169-174`. This also corrects two
  existing pages that currently pin **v5** as the precondition for these same routes and go stale
  once the `NOT NULL` `audience` column lands: `mkdocs/docs/admin/monolith.md:42` ("a `--roles
  web`-only monolith never runs the v5 migration itself…") and `mkdocs/docs/admin/web-app.md:59`
  ("point at a telemetry DB where the v5 migration has already run") both need "v5" restated as
  "v6" — a v5-only schema now makes mint/import fail with a 500 on the missing column, not just a
  missing table.
  A `NOT NULL` column with no default breaks the *opposite* order too, and needs its own explicit
  callout rather than falling out of the v5→v6 restatement above: once the schema reaches v6, a
  not-yet-upgraded `analytics-web-srv` process's mint/import `INSERT`s (which list columns
  explicitly and, pre-#1372, omit `audience`) start failing with a `NOT NULL` violation (500),
  same symptom as the missing-column case but the opposite cause. Both pages therefore state the
  deploy order as a requirement, not just a sequencing note: **upgrade `analytics-web-srv` to this
  change in the same deploy that runs the v6 migration** — running the migration first without also
  rolling the web service (or rolling the web service first against a still-v5 schema, which
  reproduces the existing missing-column 500) both produce an outage window. Ingestion and key
  validation insert no columns and are unaffected by ordering either way.
- **New section, not a new page: "Audiences and Grants" in `mkdocs/docs/admin/authentication.md`**,
  immediately after the existing "Audience Filtering Activation" section (`:152-180`) — that section
  already documents the `MICROMEGAS_UNSTAMPED_AUDIENCE`/`MICROMEGAS_IMPLICIT_GROUPS` mechanism this
  plan rewrites (per the sweep above) and is the closest existing anchor for the model. A new page
  under `mkdocs/docs/admin/` would also need a `nav:` entry added to `mkdocs/mkdocs.yml`'s explicit
  tree (`:129-144`, the "Administration" list) — avoided by keeping this a section on an existing,
  already-navigable page. Covers: the model (label vs. grant), the `{prefix}_AUDIENCE_GRANTS` shape
  and its unprefixed fallback, the two built-in rules, worked open and privacy profiles, and the
  "re-share after the fact by editing grants, never by restamping" property that motivates it.
- `mkdocs/docs/admin/authentication.md` — ingestion keys carry an audience and are no longer
  delegating service accounts. Its `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` recipe (`:162-180`)
  is restated using an opaque audience name instead.
- `mkdocs/docs/admin/flight-sql.md` and `mkdocs/docs/admin/monolith.md` — both document the knob
  this plan deletes and a value format the new validator rejects, so both need the same sweep:
  drop the `MICROMEGAS_IMPLICIT_GROUPS` / `MICROMEGAS_ANALYTICS_IMPLICIT_GROUPS` env-table rows;
  restate `MICROMEGAS_UNSTAMPED_AUDIENCE` / `MICROMEGAS_ANALYTICS_UNSTAMPED_AUDIENCE`'s description
  as an opaque audience name rather than `user:<id>`/`group:<id>`; add a `{prefix}_AUDIENCE_GRANTS`
  row; and add an upgrade note that a previously-recommended
  `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` now fails startup (`:` is outside
  `[A-Za-z0-9_-]`). `MICROMEGAS_DEFAULT_KEY_AUDIENCE` is **not** added to `flight-sql.md`:
  `flight-sql-srv` builds with `ProviderBuilder::new("")` + `ApiKeyTable::Analytics`
  (`flight_sql_server.rs:279-280`) and hosts no mint/import route, so it never reads the knob.
  `monolith.md` does gain the `MICROMEGAS_DEFAULT_KEY_AUDIENCE` row, since its `web` role does.
- `mkdocs/docs/admin/web-app.md` — the knob is read only by `analytics-web-srv`, which this page
  documents; add a `MICROMEGAS_DEFAULT_KEY_AUDIENCE` row to its Environment Variables → Optional
  table (`:55-61`), alongside `MICROMEGAS_SQL_CONNECTION_STRING`.
- `CHANGELOG.md` under Unreleased — schema v6; the audience-model change **including amending the
  Unreleased Stage 2 (#1370) entry**, which currently documents `MICROMEGAS_IMPLICIT_GROUPS=everyone`
  as part of the escape-hatch pair (that entry hasn't shipped in any release, so it is safe to edit in
  place). The Stage 1 (#1369) entry lives in the already-released `v0.29.0` section and is **left
  untouched** — it accurately documents what that release shipped (`user:`/`group:` audiences,
  `{prefix}_IMPLICIT_GROUPS`); instead, add a new Unreleased entry recording the model change, the
  removal of `MICROMEGAS_IMPLICIT_GROUPS` (an operator-facing config break, pre-GA, noting it was
  introduced in v0.29.0), the new knobs, the new request/response fields, the `allow_delegation`
  change, and the **Minor breaking change** clause for `micromegas-auth`'s policy surface (including
  its two production `AudienceReadPolicy::new(vec![])` call sites in `flight_sql_server.rs`),
  `KeyRow`/`ApiKeyTable`, and `IngestionKeysState`.
- `tasks/data_isolation/audience_based_access_control_plan.md` — replace the prefixed-audience model
  in §1, §2, §3, the config table and both deployment stories; note that the long-term grant-store
  section is now the direct continuation of what ships here; mark Stage 4 landed.

## Testing Strategy

**`rust/auth/tests/policy_tests.rs`** (the bulk of the new coverage):

- `is_valid_audience` accepts `public`, `team-alpha`, `Team_Alpha`, `a`, and a 255-byte name;
  rejects `""`, 256 bytes, `alice@example.com`, `team alpha`, `a,b`, `["x"]`, `it's`. A companion
  test pins the *absence* of normalization — `team-alpha` and `Team-Alpha` resolve as two distinct
  audiences — since a later "helpful" case-fold would silently merge buckets.
- Resolve: `public` is always present; a `group:` selector matches a claim value; a `user:` selector
  matches the email; `*` grants everyone; an audience with no matching selector is absent; a caller
  gets **no** audience merely for being named like one (the absence of a self rule, tested
  explicitly — including an API key named `team-alpha`, which must not read `team-alpha`).
- **The motivating case, end to end in one test**: resolve `alice-laptop` for bob ⇒ absent; add
  `{"alice-laptop": ["group:leads"]}` with bob in `leads` ⇒ present. *No data changed.*
- Mint: `read_audiences` never enters the mintable set; `PUBLIC_AUDIENCE` never enters a non-admin's
  mintable set even though it is always in their readable set; admins may mint any valid audience,
  `public` included; non-admins may not mint an audience they hold no **mint** grant for — including
  one they hold a bare-array (read-only) grant for, testing explicitly that a read grant never
  confers mint authority; a `"mint"` entry does grant it, independent of `"read"`.
- Malformed `{prefix}_AUDIENCE_GRANTS` / `MICROMEGAS_AUDIENCE_GRANTS` (not an object; a per-audience
  value that is neither a bare array nor a `{"read": [...], "mint": [...]}` object; a non-array
  `read`/`mint` field; non-string selector) ⇒ `Err`, so a typo fails startup rather than shipping an
  inert knob. The prefixed-falls-back-to-unprefixed resolution itself gets the same test coverage as
  `implicit_groups_var`.

**`rust/auth/tests/db_api_key_tests.rs`** (live-Postgres cases `#[ignore]`d, per the file's
convention): its `insert_live_key` helper (`:229-249`) currently inserts into `ingestion_api_keys`
with no `audience`, which every existing live test using it (`:268`, `:323`, `:355`, `:382`, `:409`,
`:422`, `:435`) would fail at runtime against a v6-migrated schema, since the column is `NOT NULL`
with no default — a failure the compiler cannot catch. `insert_live_key` gains an `audience: &str`
parameter (every ingestion-table call site passes a literal, e.g. `PUBLIC_AUDIENCE`), threaded into
the `INSERT`'s column list; analytics-table call sites are unaffected since that table has no such
column. New coverage: the audience reaches `bound_audience` unchanged (the issue's first test);
ingestion keys give `allow_delegation: false` while analytics keys still give `true` and
`bound_audience: None`; the audience survives a cache hit; the unreachable-pool cases still yield
`ProviderUnavailable`.

**`rust/auth/tests/default_provider_tests.rs`**: same live-DB issue as `db_api_key_tests.rs` above —
its own `insert_live_key` helper (`:45-59`) inserts into `ingestion_api_keys` with no `audience`,
which its two call sites (`:109`, `:136`) would fail at runtime against a v6-migrated schema. Gains
the same `audience: &str` parameter, threaded through with a literal (e.g. `PUBLIC_AUDIENCE`); no
new coverage needed here, since this file's assertions are about `ProviderBuilder` construction and
existence checks, not `bound_audience`. (Its other, throwaway-schema table used for the
existence-query tests at `:196` is unaffected — it never touches `ingestion_api_keys`.)

**`rust/analytics-web-srv/tests/ingestion_keys_tests.rs`**: `mint` with no audience and no knob ⇒
400 naming the knob; with the knob ⇒ that value; explicit ⇒ that value (the issue's per-key
override). `import` with neither ⇒ `public`. An invalid audience ⇒ 400 before any DB access.
`#[ignore]`d live-DB: importing an already-present hash reports the **existing** audience.

**Migration** — new `rust/ingestion/tests/sql_migration_test.rs` (this crate has no migration test
today; `analytics-web-srv/tests/migration_test.rs` covers the unrelated **app_db** v3→v4 chain and
is a *style* reference only — same live-DB, `#[ignore]`d convention, different database and
different `execute_migration`): against a live data-lake DB seeded with a v5-era row,
`execute_migration` leaves the row's audience `public`, not NULL (the issue's "backfills … rather
than staying NULL"), and a hand-written `INSERT` of `''` is rejected by the `CHECK`.

**`OwnershipRewrite`** — `MICROMEGAS_UNSTAMPED_AUDIENCE=public` and `=team-alpha` both parse (the
latter would have been rejected before) and produce the same coalesce predicate. This also rewrites
four existing, currently-passing tests in `rust/analytics/tests/ownership_rewrite_config_tests.rs`
whose premises the relaxed `[A-Za-z0-9_-]` charset inverts, not just adds coverage alongside:
`malformed_unstamped_audience_is_rejected` (`:84`) asserts the *unprefixed* `"everyone"` is an
`Err` under the old `user:`/`group:`-prefix rule — under the new charset it is well-formed, so this
case moves to the accepted list and the test is rewritten (or dropped in favor of a still-invalid
example, e.g. an empty string or `"a:b"`); `well_formed_unstamped_audience_is_accepted` (`:104`)
and the two fallback-resolution tests, `prefixed_unstamped_audience_wins_over_unprefixed_fallback`
(`:122`) and `unprefixed_unstamped_audience_used_when_prefixed_is_unset` (`:141`), all set
`group:`-prefixed values (`"group:everyone"`, `"group:prefixed"`, `"group:unprefixed"`) that fail
the new parser (`:` is outside `[A-Za-z0-9_-]`) — each is rewritten to use an opaque value (e.g.
`"everyone"`, `"prefixed"`, `"unprefixed"`) so the fallback-resolution behavior they actually test
keeps its coverage.

**Python** — `read_keyring` triples with and without a per-entry audience; the `--audience` +
`--table analytics` guard; the per-entry `"audience"` + `--table analytics` guard in `main`, run
before any import call (same rejection, checked up front rather than mid-batch); per-entry
precedence over `--audience`; `import_one` calls `import_analytics_api_key(name, key)` with no
`audience` argument on the analytics branch.

**Web app** — `mint` omits `audience` when unset and includes it when set; the Audience column
renders on the ingestion page and the analytics page does not regress.

**Manual** — `start_services.py --monolith`, mint a key with an explicit audience through the web UI,
ingest with it, confirm the audience reaches the auth context (trace-level `db api key validated`);
end-to-end stamping is #1373.

Full CI: `python3 build/rust_ci.py`, `python3 build/python_ci.py`, `yarn lint && yarn type-check &&
yarn test`.

## Open Questions

1. **Split the model change out of #1372?** As argued in [Scope note](#scope-note-this-amends-shipped-stage-1),
   steps 1–2 amend #1369. Fold in (one PR, values never persist in the old vocabulary), or land a
   companion issue immediately before this one?
2. **How does a privacy deployment provision per-user audiences?** With no self-audience rule, each
   user needs an audience plus a grant. Stage 6 (#1374) mints the personal key anyway, so the grant
   can be created there — but that means Stage 6 needs a *writable* grant store, which the env map
   is not. Either Stage 6 brings the grant store forward, or a deployment scripts the map. Worth
   deciding before #1374 is specced. (If per-user isolation should stay zero-config, the smallest
   change is adding `@` and `.` to the charset — both are escape-free everywhere the charset was
   chosen to protect — and restoring the email-keyed self rule.)
3. **Should `public` be neutralizable?** As designed an operator cannot configure it away. The lever
   for a deployment that wants "no `public`, ever" would be a startup check rejecting it in
   `MICROMEGAS_DEFAULT_KEY_AUDIENCE` plus a mint-time refusal — not a read-side switch, which would
   hide already-ingested data rather than prevent publication.
