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
strings. This plan replaces that with **an audience as an opaque label on data, whose readers are
separate configuration that can be changed after the fact.**

The prefixed encoding is a shortcut around building a grant relation, and it is paid for in the one
currency this system cannot refund: **immutable history**. Once a process is stamped
`user:alice@example.com`, that data can never be shared with her team, because sharing would mean
restamping already-ingested processes. Under a grant model the same data stays stamped `alice` and
an operator edits one line of config.

**How this stands relative to the umbrella plan — one agreement and two deliberate overrides.** The
agreement is the load-bearing one, recorded verbatim at
`audience_based_access_control_plan.md:597-601`: *"Grants are the only authority; the
`user:`/`group:` value prefixes are naming convention. Once grants exist, no consumer may infer
authorization from an audience's prefix."* Its long-term schema is already
`group_read_grants(group_id, audience TEXT)` **and** `group_mint_grants(group_id, audience TEXT)`
(`:635-636`) — read and mint grants kept in two separate relations, deliberately never collapsed
into one — which this plan carries as one env map rather than two tables (§2). So the direction of
travel is the recorded one; this just gets there before any audience value becomes durable.

The two overrides are stated here rather than left implicit, because the umbrella's target-state
section says the opposite of each and both need rewriting (see [Documentation](#documentation)):

- **`:601-603` says "The prefixes stay"** ("because they encode the *default* (identity) grant and
  keep emails from colliding with group ids"). §1's `[A-Za-z0-9_-]` charset makes
  `user:alice@example.com` unrepresentable. The collision concern it names is answered instead by
  audiences living in a single flat namespace with byte-exact identity, and the default-grant
  concern by §2's explicit grant entry.
- **`:584`'s target formula retains the identity term**, `readable(caller) = ⋃ { read_grants(g) : g
  ∈ closure(caller) } ∪ {user:<email>}`. §2 abolishes it — that is the "no self-audience rule"
  decision, argued below on charset and `subject`-collision grounds.

Neither override changes the umbrella's grant-store end state; both are vocabulary and one union
term. Recording them as overrides is what keeps the umbrella honest once this ships.

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

**Decided: not split.** Steps 1–2 land in this same plan/PR rather than a companion issue amending
#1369 immediately before it — the model change and the column that makes an audience value durable
ship together, so no value is ever persisted in the vocabulary being abandoned.

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
- `rust/analytics-web-srv/src/ingestion_keys.rs` — the only **key-administration** write surface for
  `ingestion_api_keys`, and the only place that inserts rows: `mint_key` (`:170-212`), `list_keys`
  (`:235-285`), `revoke_key` (`:302-330`), `import_key` (`:375-441`). Both insert sites list their
  columns explicitly (`:186-189`, `:393-398`). State is
  `IngestionKeysState { pool: Option<PgPool> }` (`:51-54`), built in `web_server.rs:643-645`.
  (`db_api_key.rs:272-275`'s `UPDATE … SET last_used_at` is the other production writer of this
  table — a touch, not administration; §4 is what changes it.)
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
- **Uniqueness is byte-exact identity**: every consumer compares verbatim, so `team-alpha` names one
  bucket and nothing else. Note what the grant map does *not* give you here: a parsed JSON object
  cannot hold two identical keys, but `serde_json` reaches that state by **silently keeping the last
  value**, so `{"team-alpha": ["group:a"], "team-alpha": ["group:b"]}` discards the first grant list
  without a word. That is the one typo class §2's content validation structurally cannot see, and it
  contradicts this plan's own "never a silently-inert entry" standard — so `AudienceGrants` parsing
  **rejects a repeated key**, naming it, rather than resting on "unique by construction". (Repo
  precedent runs the other way and is worth breaking with: `api_key.rs:57-64` builds its keyring
  `HashMap` with a plain `insert`, silently overwriting a duplicate; `MICROMEGAS_ADMINS` is a
  `Vec<String>`, not a map, so it offers no precedent at all. The target store makes it impossible by
  construction — `PRIMARY KEY (group_id, audience)` — and the env map should not be strictly weaker.)
  The failure is fail-closed either way (a lost read grant is less access, never more), which is why
  this is a parse-time `Err` rather than anything more elaborate. If an audience registry table lands
  later (the natural home for a description or an owner), it carries a `UNIQUE` index on the name;
  nothing else changes.
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
`MICROMEGAS_OIDC_CONFIG`). Keys are audience names, validated and de-duplicated at parse time (§1).

Each value carries an explicit **intent axis**, not one list consulted by both policies: a bare
array is shorthand for **read-only** selectors (the common case, and the only thing most audiences
need), and an object form, `{"read": [...], "mint": [...]}`, adds an explicit mint list when one is
needed. One relation per axis, kept in a single env map for now only because there is no store yet
to split them across two tables — a 1:1 stand-in for the umbrella plan's `group_read_grants` /
`group_mint_grants` (`audience_based_access_control_plan.md:635-636`, and "Read and write finally
separate" at `:668-673` for why they are never one relation). An omitted `"mint"` list is therefore
always empty, never defaulted from `"read"`.

**Why the mint axis is in the format now, and not deferred to #1374 with its first real consumer.**
Not because "the policy would otherwise drift" — `AudienceMintPolicy` has to be rewritten against
the new model regardless, since it must compile. The reason is falsifiability: with a
read-only, bare-array-only format, the non-admin mint arm resolves over a set that is **empty by
construction and unpopulatable**, so the invariant this section turns on — a read grant confers no
mint authority, and an omitted `"mint"` list is never derived from `"read"` — cannot be expressed as
a passing-and-failing test pair, only as "denies everything". The Testing Strategy's mint bullets
below are exactly those tests, and they are unwritable without the axis. Landing the split *and*
pinning it with a test in the same change that abandons the derived model is the cheap moment;
re-opening a documented env format once `authentication.md` carries worked profiles is the expensive
one. Cost is an untagged two-variant serde enum plus the *same* selector validator applied to both
lists. What is deferred to #1374: worked **mint** profiles in the docs — this stage documents the
format grammar and the read-side profiles only.

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

**Parsing validates both axes of the map, not just its JSON shape** — same reason every other knob
here fails startup on a typo rather than shipping inert:

- **Every key must satisfy `is_valid_audience`.** `{"group:everyone": ["*"]}` is the single most
  likely typo in this whole change — it is exactly the value operators are migrating *from* — and it
  can never match a stamped audience, because the v6 `CHECK` and `resolve_audience` both reject `:`.
  An unvalidated key would be a grant that reads as configured and grants nothing.
- **Every selector must be `*`, `user:<non-empty>`, or `group:<non-empty>`.** An unprefixed `"eng"`
  (or a mistyped `"users:alice@example.com"`) matches no identity axis and is likewise inert. The
  prefixes are a closed set here precisely so an unrecognized one is an error rather than a
  never-matching string.

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
that same flow is a natural extension of it rather than new machinery. **Decided: this plan does
nothing further here.** A privacy deployment has no way to provision per-user audiences until users
can mint their own keys — that arrives with Stage 6, not before — so per-user isolation is out of
scope for #1372 by construction, not an open gap. See [Open Questions](#open-questions-resolved).

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

`AudienceMintPolicy` remains what it is today: **test-only, with no production construction site and
no `from_env`.** §6's route helper does the mint-time decision inline, and #1374 is what builds a
real instance and calls the trait. Rewriting the policy now anyway is what keeps the two axes from
drifting — the mint arm has to be written against the same `AudienceGrants` the read arm resolves, or
#1374 inherits a policy that still speaks the deleted vocabulary.

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
UPDATE migration SET version=6;
```

- **The trailing `UPDATE migration SET version=6;` is part of the statement list, not boilerplate to
  infer**: every `upgrade_data_lake_schema_vN` in this file ends with it (`:52`, `:72`, `:86`,
  `:137`), `execute_migration` re-reads the version *inside the same transaction* before committing,
  and both `execute_migration:256` and `remote_data_lake.rs:40` then `assert_eq!` on it — so omitting
  it panics at startup rather than erroring.
- **No `DEFAULT` on the column, deliberately** — unlike v4's one-statement
  `ADD COLUMN format TEXT NOT NULL DEFAULT 'micromegas-transit'` (`:81`). A default would let a
  not-yet-upgraded `analytics-web-srv` keep inserting rows that silently take `public`, which is
  exactly the fail-closed property §6 and umbrella step 10 rely on. Hence the three-step
  add / backfill / `SET NOT NULL` form, and hence the deploy-order requirement below.

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
  `VARCHAR(255)` already caps the length, so the regex carries the character set. The two bounds
  cannot disagree: `VARCHAR(255)` counts characters and `is_valid_audience` counts bytes, but the
  ASCII-only charset makes them equal for every value that can be stored (a multibyte value fails
  the charset check before length is ever in question). This is why `audience` needs no analogue of
  `MAX_NAME_BYTES`' bytes-vs-chars note (`ingestion_keys.rs:38-40`), where `name` is free-form UTF-8.
  Counting §1's
  deliberate `read_scope.rs` copy, "valid audience" is therefore stated in three places
  (`policy.rs`, `read_scope.rs`, this `CHECK`) — two crate-boundary-forced Rust copies plus the SQL
  one, each the same two rules, documented together in one place so they stay in step. No *fourth*
  definition, and no place where the rules differ.
- `analytics_api_keys` is untouched: its read-side mirror is `read_audiences` (Stage 4b), a
  set-valued grant in the opposite direction.
- **`NOT NULL` with no default breaks the opposite deployment order too.** The migration ships in
  `telemetry-ingestion-srv`/monolith, but `analytics-web-srv`'s mint/import `INSERT`s list columns
  explicitly (`ingestion_keys.rs:186-189`, `:393-398`) and — pre-#1372 — omit `audience`. In a split
  deployment where the two are separate processes, running the migration before rolling
  `analytics-web-srv` means every mint/import against the now-v6 schema hits the `NOT NULL`
  constraint until the web service is upgraded. The [Documentation](#documentation) upgrade note
  below states the required order: roll `analytics-web-srv` to this change in the same deploy that
  runs the v6 migration, not before and not after.
- **Key *validation* is safe for a different reason than "it never inserts".** §4 makes the
  ingestion provider's loader *read* the column (`RETURNING key_id, name, audience`), so an upgraded
  ingestion binary against a still-v5 schema would fail **every** ingestion-key validation into
  `ProviderUnavailable` — a total ingestion-auth outage, not a per-route 500. That cannot happen
  because the processes that host an `ApiKeyTable::Ingestion` provider are the same ones that run the
  data-lake migration themselves at startup, before serving: `migrate_db`
  (`rust/ingestion/src/remote_data_lake.rs:21-42`) via `connect_to_remote_data_lake` (`:45`, calling
  it at `:60`), called from
  `telemetry-ingestion-srv/src/main.rs:52` and `monolith/src/main.rs:183`. So the binary that reads
  the column is always the binary that just created it. `flight-sql-srv` runs no data-lake migration
  but binds `ApiKeyTable::Analytics` (`flight_sql_server.rs:279-280`), whose `RETURNING` is unchanged
  — the table-conditional `RETURNING` in §4 is what keeps that true. Writing telemetry payloads
  touches no key column at all and is unaffected either way.

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
- `allow_delegation: matches!(self.table, ApiKeyTable::Analytics)` — **false for ingestion keys**,
  unchanged (`true`) for analytics keys. Written against the table identity, **not** as
  `!self.table.has_audience()`: "carries an audience column" and "is a delegating service account"
  are unrelated properties that coincide only because `Ingestion` happens to be the sole
  audience-carrying table today, and a reader shouldn't have to reconstruct that coincidence.
  `allow_delegation` only governs `x-user-*` attribution on the gRPC path — set into the
  `x-allow-delegation` metadata header at `tower.rs:132`, read at `user_attribution.rs:142`/`:163`,
  reached solely from `flight_sql_service_impl.rs:636` — which an ingestion key can never reach: it
  lives in the other table, and the HTTP/ingestion middleware (`axum.rs:78`) only *strips* that
  header, never sets it. So the new value is behaviorally inert today; it is correct rather than
  load-bearing. An ingestion write credential is not a delegating service account.

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
  pub fn resolve_audience(
      state: &IngestionKeysState,
      requested: Option<&str>,
      fallback: Option<&str>,   // None for mint, Some(PUBLIC_AUDIENCE) for import
  ) -> Result<String, IngestionKeyError>
  ```

  `pub`, not module-private, so the resolution matrix above is unit-testable without a database — see
  Testing Strategy for why that is the difference between three assertions in `cargo test` and three
  in `#[ignore]`d live tests. It is sync and touches no pool, which is what makes that possible.

  **Called after `require_pool` and `validate_name`, not before them.** The order is
  `require_pool` → `validate_name` → `resolve_audience` → `INSERT` for `mint`, and
  `require_pool` → `validate_name` → empty-`key` check (`ingestion_keys.rs:382-386`) →
  `resolve_audience` → `INSERT` for `import`, so a request that is wrong in two ways gets the message
  for the more basic mistake. (Both are `BadRequest`, so the status and `{code}` are identical either
  way; only the message differs, and nothing asserts on it. `import_400_for_empty_key`
  (`ingestion_keys_tests.rs:179`) posts no `audience`, and import's `PUBLIC_AUDIENCE` fallback means
  `resolve_audience` can't fail on absence, so that test is safe under either order.) This keeps the
  existing
  `NotConfigured` precedence intact: `mint_503_when_pool_unconfigured`
  (`ingestion_keys_tests.rs:218`) and `import_503_when_pool_unconfigured` (`:251`) both post a body
  with no `audience` against `IngestionKeysState { pool: None }`, and both must still get 503, not
  the new 400. This costs nothing against the "400 before any DB access" requirement — `require_pool`
  (`ingestion_keys.rs:135`) only clones an `Option<PgPool>` and never touches the database, so an
  invalid audience is still rejected before any query runs.

  A missing field or an empty string counts as absent (the empty string is not a name — it fails
  `is_valid_audience` either way); anything else is taken **verbatim**, no case folding, so
  the audience an operator typed is the audience that gets stored. It then applies
  `state.default_audience`, then `fallback`, errors `BadRequest` when nothing resolves, and validates
  with `is_valid_audience`. Both routes are
  `AdminUser`-gated, and `AudienceMintPolicy`'s admin arm accepts any valid audience — so this is the
  same decision Stage 6 will make through the policy, and Stage 6 replaces the helper's
  *validation/authorization* step with `MintPolicy::resolve_audience`, keeping the
  `state.default_audience` → `fallback` chain in the helper (the trait method's
  `resolve_audience(caller, requested: Option<&str>)` has no channel for either `state.default_audience`
  or import's `PUBLIC_AUDIENCE` fallback, so it can only ever be the validation half once the
  fallback chain has already produced a candidate), without changing either request shape. The
  optional `audience` on `mint` is in scope now, not deferred to #1374: `mint_key`'s `INSERT` must
  supply the `NOT NULL` column regardless (`ingestion_keys.rs:186-189`), the Testing Strategy
  already requires an explicit per-key override to work end to end, and without a request field a
  per-key audience would mean restarting `analytics-web-srv` with a different
  `MICROMEGAS_DEFAULT_KEY_AUDIENCE` for every mint — defeating §1's motivating scenario. Keeping the
  field now is what lets Stage 6 change only that inner step.
- `MintResponse` / `ImportResponse` / `KeyListEntry` each gain `audience`. On import's already-present
  (`imported: false`) path the response reports the **existing** row's audience — the audience is
  immutable, so an import never rewrites it; both branches already share `ImportedRow`, which gains
  the field.
- `KeyListEntry` is `sqlx::FromRow`, so its new field needs the column added to **both** of
  `list_keys`' `SELECT`s (`ingestion_keys.rs:261` and `:272`, the `include_revoked` true/false
  branches) — a column missing from one of them fails at *runtime* with a 500, not at compile time.
  Same for `ImportedRow`'s two queries (`:397` `RETURNING`, `:414` fallback `SELECT`).
- **Doc-comment constraint on this file.** `mint_statement_names_ingestion_table_only`
  (`ingestion_keys_tests.rs:274-286`) asserts on `src/ingestion_keys.rs`'s **source text**: it
  requires `"INSERT INTO ingestion_api_keys"` to still appear verbatim (unaffected by adding a
  column) and forbids the substrings `"INTO analytics_api_keys"` / `"UPDATE analytics_api_keys"`
  anywhere in the file, **including comments**. The new doc comment explaining why the analytics
  table carries no audience must therefore say `analytics_api_keys` on its own, never
  `INTO analytics_api_keys`.
- `revoke_key` is unchanged.

### 7. CLI + Python client

- `WebClient.import_ingestion_api_key(name, key, audience=None)` — omits the field when `None`.
  The current body posts an inline literal (`json={"name": name, "key": key}`,
  `web_client.py:113`), so omit-when-`None` means building a local `payload` dict first — the same
  shape `create_screen` (`:55-74`) and `update_screen` (`:76-89`) already use for
  `managed_by`/`folder_path`. `import_analytics_api_key` (`:119`) is untouched.
- `micromegas-import-keys`:
  - `--audience AUD`, valid only with `--table ingestion` (`parser.error` otherwise —
    `analytics_api_keys` has no such column). `--table` is `choices=["ingestion", "analytics"]`,
    `required=True`, no default (`import_keys.py:214-219`), so a `!= "ingestion"` guard is total; the
    natural home is beside the existing post-parse cross-flag check at `:261-262`
    (`--source file requires --path`). Two cautions: the guard tests `args.audience is not None`, not
    truthiness, so `--audience ""` is rejected by the validator rather than silently omitted (this
    file's `folder_path` convention treats the empty string as a transmitted value, not an absence);
    and the flag's help text must disambiguate it from the **OIDC token** audience already threaded
    through this same file as `audience=conn.oidc_audience` (`:156`, `MICROMEGAS_OIDC_AUDIENCE`).
  - Per-key choice: a keyring entry may carry an optional `"audience"` field, which wins over
    `--audience`. `read_keyring` returns `(name, key, audience)` triples unconditionally — the field
    is read regardless of `--table` — and a non-string `audience` is a `parser.error` like the
    existing `name`/`key` `isinstance` checks (`:89-96`). The new triple arity ripples through every
    other site that destructures `read_keyring`'s output, and there are **four in `select_entries`
    alone**, not one filtering expression: `:106` and `:115` (`known = {name for name, _ in entries}`,
    which raise `ValueError: too many values to unpack` on a triple) and `:112` and `:121` (the
    `[(name, key) for name, key in entries if …]` comprehensions) — plus `run_import`'s per-key loop
    (`:189`). `import_one` (`:171`) takes `name, key` positionally rather than unpacking, so its
    ripple is a signature change.
  - **A per-entry `"audience"` combined with `--table analytics` is a `parser.error`, same as the
    `--audience` flag form** — one entry-level check, so a keyring built for ingestion isn't silently
    reused against the analytics table with its audience dropped, and so the whole batch is rejected
    **up front rather than partway through a series of live HTTP imports**. That up-front property is
    the whole reason for the placement; it is *not* a matter of `parser` scope — `read_keyring(args,
    parser)` (`:45`) and `select_entries(entries, args, parser)` (`:101`) both hold `parser`, and only
    `run_import(client, table, entries)` (`:183`) does not. `read_keyring`'s existing per-entry
    validation loop (`:88-97`) already has both `parser` and `args.table` (read at `:64`) in hand and
    is the natural home; putting it in `main` after `read_keyring` is equally correct. Either way it
    runs before `select_entries` (`:265`), which means an entry carrying an `"audience"` aborts the
    run even when `--only`/`--exclude` would have dropped that entry — intended, and stated here so it
    isn't a surprise. `import_one` reflects this split: it passes `audience` to
    `WebClient.import_ingestion_api_key(name, key, audience)` only on the ingestion branch, and calls
    `import_analytics_api_key(name, key)` (no `audience` parameter) on the analytics branch, since
    that call is only ever reached once the check above has already rejected a non-`None`
    audience for that table.
  - Neither given ⇒ the field is omitted and the server applies `public` — the zero-decision path.
  - `run_import`'s per-key line (`:199`, `:201`, `:204`) gains the audience the server reports.

### 8. Web app

- `api-keys-shared.ts`: `ApiKeyListEntry` (`:11-19`) and `MintApiKeyResponse` (`:21-27`) gain
  `audience?: string` (optional — analytics rows never carry one); `mint(name, audience?)` sends the
  field only when set, both in the `ApiKeysApi<T>` interface (`:63`) and in `createApiKeysApi`'s
  closure (`:97-104`). Because that factory is shared, `mintAnalyticsApiKey`
  (`analytics-api-keys-api.ts:50`) inherits the optional parameter too. That is accepted rather than
  gated: the analytics *page* never passes it (`showAudience` is unset there, so no input exists),
  and a `supportsAudience` flag on `ApiKeysApiConfig` would be a second knob guarding a caller that
  doesn't exist. `ingestion-api-keys-api.ts` needs **no** edit — its entries are pure aliases
  (`IngestionApiKeyListEntry = ApiKeyListEntry` `:32`, `mintIngestionApiKey = api.mint` `:54`) and
  inherit the change.
- `ApiKeysAdminPageConfig` (`ApiKeysAdminPage.tsx:32-48`) gains `showAudience?: boolean`: an
  **Audience** table column (header row `:294-310`, currently 5 columns) plus an audience input in
  the mint dialog (`:206-253`, alongside the Name input at `:226-233`) — placeholder `public`,
  helper text naming `MICROMEGAS_DEFAULT_KEY_AUDIENCE`. Two edits in the same component that
  `showAudience` alone does not cover: **`mintKey` widens** to
  `(name: string, audience?: string) => Promise<MintApiKeyResponse>` (`:46`) so `handleMint` can
  pass the value (`:106`, today `config.mintKey(mintName.trim())`) — a TS error otherwise; and
  `openMintForm` (`:96-100`), which already resets `mintName`/`mintError`, must reset the new
  `mintAudience` state too, or a previous mint's audience leaks into the next one.
  `IngestionApiKeysPage.tsx` sets `showAudience: true`; the analytics page is untouched. List rows
  predating the column render `undefined` — render `{key.audience ?? '—'}` **inline**, matching the
  `'—'` the component already uses at `:26` but *not* routing through `formatDate` itself: that
  helper is `(iso: string | null) => string` (`:25`) and does `new Date(iso).toLocaleString()`
  (`:27-29`), so under this app's `strict` tsconfig an `audience?: string` (i.e. `string |
  undefined`) is not even assignable to it, and only its `'—'` branch was ever the reusable part.
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

**Read this first: what green CI will not tell you.** Most of this change fails loudly — a new struct
field breaks every literal at compile time (all 15 `IngestionKeysState` sites), `formatDate`'s typing
breaks `yarn type-check`, a forgotten `UPDATE migration SET version=6;` panics at startup, and the
strict `toEqual` mint-body assertions break `yarn test`. Four ripples do **not**: they live in
`#[ignore]`d live-Postgres tests that `python3 build/rust_ci.py` never runs, so a full-green CI run is
not evidence they work. Verify these by hand against a live DB (`cargo test -- --ignored`):

- `rust/auth/tests/db_api_key_tests.rs`'s `insert_live_key` (`:232-251`) — table-generic `INSERT`
  with no `audience`; fails at all nine call sites against a v6 schema.
- `rust/auth/tests/default_provider_tests.rs`'s own `insert_live_key` (`:45-59`) — same problem, two
  call sites, easy to miss because it is a second copy of the helper.
- `db_api_key_tests.rs:282`'s `assert!(ctx.allow_delegation)` — **inverts** under §4.
- `ingestion_keys_tests.rs:309`'s `live_mint_list_revoke_round_trip` — POSTs no `audience` and
  asserts `CREATED` (`:327`), which §6 turns into the 400 case.

All four are edits to **existing** live tests, not new ones. Write the non-live coverage first: the
`allow_delegation` rule and `has_audience()` are pure functions of `ApiKeyTable` and get their own unit
tests, and §5's resolution matrix is unit-tested against `pub fn resolve_audience` rather than through
a route that must complete an `INSERT`. That demotes `:282` from sole guard to confirmation and puts
the resolution matrix in default `cargo test`.

**This change adds exactly one new live test file and one new live assertion**, both justified
individually in the Testing Strategy: `rust/ingestion/tests/sql_migration_test.rs` (nothing but
Postgres can evaluate a `CHECK` regex or `SET NOT NULL` ordering, and it is the only executable link
between the SQL copy of the charset rule and the Rust one), and "the audience reaches `bound_audience`"
in `db_api_key_tests.rs` (the `RETURNING` list and `try_get("audience")` are string-typed end to end).
Everything else that touches a live test is a modification of one that already exists.

Two more ripples are runtime-only but caught by any live run: a `KeyListEntry`/`ImportedRow` column
added to one query but not its twin (`ingestion_keys.rs:261`/`:272`, `:397`/`:414`) 500s instead of
failing to compile, and the Python tuple-arity change surfaces no error until the test executes.

1. **Opaque audiences.** `policy.rs`: add `is_valid_audience`, `PUBLIC_AUDIENCE` (`is_well_formed_audience`
   itself is deleted in step 2, alongside its other callers, since steps 1–2 land as a single
   compiling change). `read_scope.rs`: the same relaxation in its copy. This inverts the premise of
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
   triple and every site that destructures it — **four in `select_entries` alone** (`:106`, `:112`,
   `:115`, `:121`) plus `run_import`'s loop (`:189`); `import_one` (`:171`) takes its arguments
   positionally, so it needs a signature change rather than an unpack fix — plus
   `tests/cli/test_import_keys.py`'s `FakeClient`/`Client` arities, `make_args` defaults, and ~20
   2-tuple literals (all runtime failures, not import-time; see Testing Strategy).
7. **Web app.** `api-keys-shared.ts`, `ApiKeysAdminPage.tsx` (including `mintKey`'s widened
   signature), `IngestionApiKeysPage.tsx`. `ingestion-api-keys-api.ts` inherits the change with no
   edit.
8. **Vocabulary sweep.** `group:everyone` / `user:`-prefixed examples out of
   `monolith/src/main.rs:248-253` (the whole comment block, not just `:252`'s knob pair — `:249`
   names both `IMPLICIT_GROUPS` forms and `:250` asserts the prefixed model's "resolved scope is
   just their own identity"), `policy.rs` doc comments **and its runtime error string** (`:259`,
   `"must be 'user:<id>' or 'group:<id>'"`), `ownership_rewrite.rs:187`'s doc comment (`"user:alice"
   sorts below "group:everyone"` — a doc comment mid-file, not the top-of-file module doc),
   `read_scope.rs`'s own doc comments (`:30`, the `ReadScope::Audiences` variant doc's
   `"user:<email>" / "group:<id>"` example; `:90`, `unstamped_audience`'s `"group:everyone"`
   example; `:175`, `from_env`'s own bail message, which repeats the same prefix vocabulary and is
   rewritten with the validator in step 1; and `:112-118`, `parse_comma_separated_list`'s doc
   comment, which also loses its
   dangling cross-reference to `micromegas_auth::policy`'s `MICROMEGAS_IMPLICIT_GROUPS` parser once
   step 2 deletes `parse_implicit_groups` and the knob — restate the comparison against the new
   `AudienceGrants` parser or drop the cross-reference), the Unreleased Stage 2 CHANGELOG entry (the
   Stage 1 entry is released and stays untouched — note it carries *two* artifacts: the escape-hatch
   knob pair and a forward-reference to this plan, `"each key is assigned exactly one write audience
   (Stage 4)"`, which now needs the grants model), the umbrella plan,
   **`tasks/data_isolation/crypto_based_data_isolation_plan.md`** — which needs a *design* edit at
   `:113-118`, not just a word swap, because its `InstanceKind` classification discriminates on the
   `user:`/`group:` prefix (an *active* sibling design doc in the same folder as the umbrella plan,
   not a historical record — unlike `tasks/completed/{1369,1370}_*.md`, which keep their prefixed
   vocabulary because they record what shipped) — and
   `mkdocs/docs/admin/{flight-sql, monolith}.md`'s env-var tables. Exact targets for the last three
   are in [Files to Modify](#files-to-modify) and [Documentation](#documentation); those two lists are
   authoritative, this step is the index.
9. **Tests**, then **docs + CHANGELOG**, per the sections below.

## Files to Modify

| File | Change |
|---|---|
| `rust/auth/src/policy.rs` | opaque audiences, grant map, both policies rewritten |
| `rust/auth/src/default_provider.rs` | drop `implicit_groups_var` |
| `rust/analytics/src/lakehouse/read_scope.rs` | validator relaxed to the same rules as `is_valid_audience` (its own copy — this crate does not depend on `micromegas-auth`), preserving `from_env`'s existing trim-before-validate order (`:169`) so an all-whitespace value still resolves to `None` rather than becoming a malformed audience; vocabulary sweep at `:30`, `:90`, `:175` (the bail message) and `:112-118` (the last drops or restates its dangling reference to the deleted `MICROMEGAS_IMPLICIT_GROUPS` parser) |
| `rust/ingestion/src/sql_migration.rs` | migration v6 |
| `rust/auth/src/db_api_key.rs` | `has_audience`, `KeyRow.audience`, `bound_audience`, `allow_delegation` |
| `rust/auth/src/types.rs` | doc comments on `bound_audience` (`:59-61`, "`None` for every principal today"), `groups` (`:73-74`, which says the policies "map each entry to `group:<id>`" — under §2 the policies match `group:<g>` *selectors* against raw claim values instead), **and `allow_delegation` (`:55-57`), whose "API keys/service accounts: true (can act on behalf of others)" §4 makes false for every ingestion key** — the one doc comment the change actually invalidates. `read_audiences` (`:63-66`) stays accurate as written |
| `rust/analytics-web-srv/src/{ingestion_keys,web_server}.rs` | audience on mint/import/list; knob at startup |
| `rust/monolith/src/main.rs` | config comment (`:248-253`); `AudienceReadPolicy::from_env("MICROMEGAS_ANALYTICS")` at `:255` compiles unchanged — `from_env(prefix)` keeps its signature |
| `rust/public/src/servers/flight_sql_server.rs` | update both `AudienceReadPolicy::new(vec![])` call sites for the grant-map constructor — `:274` (injected-provider branch) and `:305` (disabled-auth default-policy branch) — plus their "implicit groups" comments at `:144-145` (the `with_read_policy` builder doc, whose phrase spans both lines) and `:271`. `from_env("")` at `:293` is unchanged. Note both `new(...)` sites are only *defaults*: `:309`'s `self.read_policy.unwrap_or(default_policy)` means an injected policy always wins, and the only injector is `monolith/src/main.rs:320` passing the `from_env("MICROMEGAS_ANALYTICS")` policy built at `:254-258` — so `{prefix}_AUDIENCE_GRANTS` is live in both production binaries, never silently inert |
| `python/micromegas/micromegas/{web_client.py,cli/import_keys.py}` | `audience` param (built via a `payload` dict so it can be omitted, `web_client.py:113`), `--audience` |
| `analytics-web-app/src/lib/api-keys-shared.ts` | types + `mint(name, audience?)` on both the `ApiKeysApi` interface (`:63`) and the `createApiKeysApi` closure (`:97-104`). `ingestion-api-keys-api.ts` needs no edit — pure aliases |
| `analytics-web-app/src/components/ApiKeysAdminPage.tsx` | `showAudience` column (`:294-310`) + mint-dialog input (`:206-253`); `mintKey` widens to `(name, audience?)` (`:46`, called at `:106`); `openMintForm` (`:96-100`) resets the new `mintAudience` state |
| `analytics-web-app/src/routes/IngestionApiKeysPage.tsx` | `showAudience: true` |
| `rust/auth/tests/{policy,db_api_key,default_provider}_tests.rs`, `rust/analytics-web-srv/tests/{ingestion_keys,routing}_tests.rs`, `rust/analytics/tests/*ownership_rewrite*`, `analytics-web-app/src/**/__tests__/*`, `python/**/tests` | per Testing Strategy. **15 `IngestionKeysState { … }` literals need the new `default_audience` field**, not one: `routing_tests.rs:405`, `web_server.rs:643`, and 13 in `ingestion_keys_tests.rs` (`:96`, `:111`, `:126`, `:144`, `:166`, `:181`, `:199`, `:219`, `:229`, `:239`, `:252`, `:312`, `:375`) |
| `rust/public/tests/read_policy_threading_tests.rs` | doc-comment vocabulary sweep only (`:445-450`, "implicit-groups env var unset"); the `AudienceReadPolicy::from_env("MICROMEGAS_1369_THREADING_TESTS_UNSET")` call (`:455`) and its single assertion (`:475`, that `SELECT 1 AS one` returns one row) are unaffected. Its `RecordingReadPolicy` synthesizes a test-only `"subject:<subject>"` audience (`:235`) that never reaches a validator, so it stays valid |
| `rust/ingestion/tests/sql_migration_test.rs` (new) | live-DB, `#[ignore]`d migration v6 coverage — see Testing Strategy |
| `mkdocs/docs/admin/{api-keys,authentication}.md` | audiences + grants + DDL + CLI |
| `mkdocs/docs/admin/{flight-sql,monolith}.md` | remove the `MICROMEGAS_IMPLICIT_GROUPS` (`flight-sql.md:32`) / `MICROMEGAS_ANALYTICS_IMPLICIT_GROUPS` (`monolith.md:50`) rows; restate the `UNSTAMPED_AUDIENCE` rows (`flight-sql.md:33`, `monolith.md:51`) — **both the `user:<id>`/`group:<id>` format parenthetical *and* the bolded "Required, together with `…_IMPLICIT_GROUPS=everyone`" clause embedded inside them, which would otherwise survive as an instruction to set a deleted knob**; add a `{prefix}_AUDIENCE_GRANTS` row, add an upgrade note that the previously-recommended `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` now fails startup under the relaxed charset; `monolith.md` additionally gets the `MICROMEGAS_DEFAULT_KEY_AUDIENCE` row (its `web` role reads the knob) — `flight-sql.md` does not, since `flight-sql-srv` never reads it. **`monolith.md` also needs `:42`'s "never runs the v5 migration itself" restated as v6** (inside the `MICROMEGAS_SQL_CONNECTION_STRING` table cell) — see [Documentation](#documentation) |
| `mkdocs/docs/admin/web-app.md` | add a commented `export MICROMEGAS_DEFAULT_KEY_AUDIENCE=…` entry to the `### Optional` block (`:44-68`, next to `MICROMEGAS_SQL_CONNECTION_STRING` at `:62`) — this is the service that actually resolves the knob for `IngestionKeysState`; **and restate `:59`'s "where the v5 migration has already run" as v6** — see [Documentation](#documentation) |
| `CHANGELOG.md` | new Unreleased entry; amend the Unreleased Stage 2 (#1370) entry only — the Stage 1 (#1369) entry is in the released `v0.29.0` section and stays untouched |
| `tasks/data_isolation/audience_based_access_control_plan.md` | model change recorded; Stage 4 landed. Sections carrying the prefixed model, beyond §1/§2/§3 and the config table: the vocabulary table (`:132` defines **audience** as "`user:<email>` or `group:<id>`", `:133`'s **grant** row says "in v1 both derive from IdP group membership"), the target-state section (`:582`'s "label: audience stamped on data (unchanged)", `:584`'s `∪ {user:<email>}` formula, `:601-603`'s "**The prefixes stay**"), "Encoding (decided 2026-08-12)" (`:562-568`, which decides the encoding of a knob being deleted), **Stage 1 step 5** (`:1010-1018`, "Policy source (decided): IdP `groups` claim + `MICROMEGAS_IMPLICIT_GROUPS` only. No local grants table in v1" plus its "Consequence — write/read collapse to membership", which the umbrella's own `:668-673` already flags as the thing the split undoes), the confidentiality-relaxation bullet (`:1285-1287`), and the open-profile integration recipe (`:1355-1360`) — plus §2's own body at `:213`, `:244`, `:261` |
| `tasks/data_isolation/crypto_based_data_isolation_plan.md` | **one design dependency plus a vocabulary sweep.** The dependency is `:113-118`: `enum InstanceKind { Global, Process(Uuid), Audience(String) }` classifies a `view_instance_id` *by the prefix* — "`"global"` → `Global`; a `user:`/`group:`-prefixed string → `Audience`; otherwise parse as a `Process(Uuid)`" — which `[A-Za-z0-9_-]` breaks twice over: an opaque audience has no prefix and falls through to a failing UUID parse, and the literal `global` becomes a *legal audience name* colliding with the `Global` variant. That section needs a different discriminator (an explicit `audience:` sigil on the `view_instance_id` argument, or a reserved-name rule for `global`), not a word swap. Vocabulary, in the same pass: `:6` ("reuses that plan's vocabulary — … the `user:`/`group:` value shape"), `:94`'s "fourth kind: an audience (`user:<email>` / `group:<id>`)", `:95`'s `view_instance('log_entries', 'group:teamA')`, `:150`'s `make_view("group:teamA")`, `:236`'s claim that `user:alice@example.com` round-trips through the encoding, `:238-239` ("v1 constrains the audience charset to path-safe … where audience ids are already prefixed and validated" — the one place §1's charset *satisfies* the sibling doc and should be restated, not deleted), `:362`, `:379`. An active sibling design doc, not a `tasks/completed/` record |

## Trade-offs

- **Opaque audiences + grants vs. the shipped prefixed encoding.** The encoding needs no grant
  relation, which is why Stage 1 chose it; it pays for that by making access a property of the data
  itself. Since data is immutable and grants are not, that is the wrong thing to freeze. Cost of
  changing now: ~150 lines of shipped policy code, its tests, and a docs sweep. Cost of changing
  after #1373 ships: a restamping migration over already-ingested processes. The umbrella plan's
  "Query-time coalesce for unstamped data vs. a backfill script" trade-off (`:1219-1222`) rules out
  the *adjacent* case — attributing never-stamped data by backfill — for the reason that transfers
  directly here: it would require re-materializing the `processes` partitions. Restamping
  already-stamped rows is strictly worse, so this is an extension of that recorded decision rather
  than a citation of it.
- **Grant map in env vs. going straight to the `group_read_grants`/`group_mint_grants` store.** The
  store is the recorded end state and needs nested-group closure, cycle handling, cached resolution
  with a stated latency, and an admin CRUD surface — a stage of its own. The env map keeps the same
  read/mint split as the two tables (see [§2](#2-access-is-a-grant-map-with-a-readmint-axis-prefix_audience_grants)
  for why the mint axis is in the format now), so the store replaces one function body per axis, not
  the whole map. What matters is that no third grant mechanism appears later; this is mechanism #2 of
  the two the umbrella plan permits (principal-level and group-level).
- **`[A-Za-z0-9_-]` vs. length-only.** Length-only is the minimal rule for an opaque label, and the
  enforcement path is already escape-safe (`ScalarValue` literals). The charset is chosen anyway so
  that no *future* consumer — a URL segment, a CLI flag, a comma-separated knob, a filesystem or
  object-store prefix if audiences ever become a physical boundary (umbrella plan step 15) — has to
  re-open the question. Cost: emails are not valid names, which is what removes the self-audience
  rule.
- **`public` built in, everything else granted.** One built-in rule instead of two: the alternative
  (a self-audience rule) needs either a lossy email derivation or a `subject` key that lets an
  admin mint read access by naming a key after an audience. Cost: per-user isolation needs a grant
  per user rather than zero config, and no deployment can provision that grant until Stage 6
  (#1374) lets a user mint their own key — accepted as out of scope for this stage. See
  [Open Questions](#open-questions-resolved).
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
  schema to v6. Same wording as the v5 note in `default_provider.rs:169-174`. This also corrects
  three existing pages that currently pin **v5** as the precondition for these same routes and go
  stale once the `NOT NULL` `audience` column lands: `mkdocs/docs/admin/api-keys.md:156` itself
  ("Precondition: the telemetry DB must already have the v5 migration"), `mkdocs/docs/admin/monolith.md:42`
  ("a `--roles web`-only monolith never runs the v5 migration itself…"), and
  `mkdocs/docs/admin/web-app.md:59` ("point at a telemetry DB where the v5 migration has already
  run") — all three need "v5" restated as "v6": a v5-only schema now makes mint/import fail with a
  500 on the missing column, not just a missing table.
  A `NOT NULL` column with no default breaks the *opposite* order too, and needs its own explicit
  callout rather than falling out of the v5→v6 restatement above: once the schema reaches v6, a
  not-yet-upgraded `analytics-web-srv` process's mint/import `INSERT`s (which list columns
  explicitly and, pre-#1372, omit `audience`) start failing with a `NOT NULL` violation (500),
  same symptom as the missing-column case but the opposite cause. **All three pages above** therefore
  state the deploy order as a requirement, not just a sequencing note: **upgrade `analytics-web-srv`
  to this change in the same deploy that runs the v6 migration** — running the migration first without
  also rolling the web service (or rolling the web service first against a still-v5 schema, which
  reproduces the existing missing-column 500) both produce an outage window. Key *validation* is
  unaffected for the reason §3 spells out — the processes that read the new column are the same ones
  that run the migration themselves at startup — not because it inserts no columns; the runbook
  states that reason explicitly so an operator never concludes an ingestion binary may run ahead of
  the schema.
  **The v5→v6 sweep is targeted, not mechanical**: the three lines above are the only ones that pin
  v5 as a *precondition for the mint/import routes*. Four other v5 mentions describe the
  table-*creating* migration and flight-sql's `key_store_has_live_rows` existence probe, which needs
  only the table and not the column, so they **stay v5** — `api-keys.md:341` ("The migration creates
  the tables (schema v5)"), `:351`/`:353` ("a schema still short of v5", "a v5-short schema"),
  `authentication.md:701` ("creates `ingestion_api_keys` / `analytics_api_keys` (schema v5)") — as
  does the same probe's wording in `default_provider.rs:169-174` and `:180`.
- **New section, not a new page: "Audiences and Grants" in `mkdocs/docs/admin/authentication.md`**,
  immediately after the existing "Audience Filtering Activation" section (`:152-181`) — that section
  already documents the `MICROMEGAS_UNSTAMPED_AUDIENCE`/`MICROMEGAS_IMPLICIT_GROUPS` mechanism this
  plan rewrites (per the sweep above) and is the closest existing anchor for the model. A new page
  under `mkdocs/docs/admin/` would also need a `nav:` entry added to `mkdocs/mkdocs.yml`'s explicit
  tree (`:133-144`, the "Administration" list under `- Operations:` at `:129`) — avoided by keeping this a section on an existing,
  already-navigable page. Covers: the model (label vs. grant), the `{prefix}_AUDIENCE_GRANTS` shape
  and its unprefixed fallback, the two built-in rules, worked open and privacy profiles, and the
  "re-share after the fact by editing grants, never by restamping" property that motivates it.
- `mkdocs/docs/admin/authentication.md` — ingestion keys carry an audience and are no longer
  delegating service accounts. Two separate edits inside "Audience Filtering Activation", not one:
  - The **recipe** (`:162-168`, with `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` at `:163`) is
    restated with an opaque audience name and loses its `MICROMEGAS_IMPLICIT_GROUPS` half.
  - The **admonition that follows it** (`:170-181`, "API keys and no-`email`-claim OIDC tokens need
    the `MICROMEGAS_IMPLICIT_GROUPS` half specifically") is **rewritten or deleted, not restated**:
    the deleted knob is its entire subject ("Without `MICROMEGAS_IMPLICIT_GROUPS` naming a group
    these callers belong to, that set is empty…"), and its closing forward-reference to "a future
    per-key/read grant … a narrower, non-blanket alternative" is *this plan*. Its replacement is one
    sentence: an API-key or no-email caller's readable set is now `{public}` plus whatever
    `group:`/`*` selectors in `{prefix}_AUDIENCE_GRANTS` match it, so `public` alone restores legacy
    visibility with no second knob.
- `mkdocs/docs/admin/flight-sql.md` and `mkdocs/docs/admin/monolith.md` — both document the knob
  this plan deletes and a value format the new validator rejects, so both need the same sweep:
  drop the `MICROMEGAS_IMPLICIT_GROUPS` (`flight-sql.md:32`) / `MICROMEGAS_ANALYTICS_IMPLICIT_GROUPS`
  (`monolith.md:50`) env-table rows; restate `MICROMEGAS_UNSTAMPED_AUDIENCE` /
  `MICROMEGAS_ANALYTICS_UNSTAMPED_AUDIENCE` (`flight-sql.md:33`, `monolith.md:51`) — their
  description as an opaque audience name rather than `user:<id>`/`group:<id>`, **and the bolded
  "Required, together with `…_IMPLICIT_GROUPS=everyone`" clause each of those two surviving rows
  carries inline**, which deleting the `IMPLICIT_GROUPS` rows does not remove and which would
  otherwise leave both pages telling operators to set a knob that no longer exists; add a
  `{prefix}_AUDIENCE_GRANTS`
  row; and add an upgrade note that a previously-recommended
  `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` now fails startup (`:` is outside
  `[A-Za-z0-9_-]`). `MICROMEGAS_DEFAULT_KEY_AUDIENCE` is **not** added to `flight-sql.md`:
  `flight-sql-srv` builds with `ProviderBuilder::new("")` + `ApiKeyTable::Analytics`
  (`flight_sql_server.rs:279-280`) and hosts no mint/import route, so it never reads the knob.
  `monolith.md` does gain the `MICROMEGAS_DEFAULT_KEY_AUDIENCE` row, since its `web` role does.
  Adding that upgrade note also repairs a pre-existing dangling reference:
  `authentication.md:164-167` already tells readers to "see the matching env-var rows **and upgrade
  note**" on these two pages, and neither page has ever had one (both rows point at the CHANGELOG
  instead). Worth knowing so the note lands under a heading the cross-reference can actually name.
- `mkdocs/docs/admin/web-app.md` — the knob is read only by `analytics-web-srv`, which this page
  documents; add a commented `export MICROMEGAS_DEFAULT_KEY_AUDIENCE=…` entry to the `### Optional`
  block (`:44-68`), next to `MICROMEGAS_SQL_CONNECTION_STRING`.
- `CHANGELOG.md` under Unreleased — schema v6; the audience-model change **including amending the
  Unreleased Stage 2 (#1370) entry**, which currently documents `MICROMEGAS_IMPLICIT_GROUPS=everyone`
  as part of the escape-hatch pair (that entry hasn't shipped in any release, so it is safe to edit in
  place). The Stage 1 (#1369) entry lives in the already-released `v0.29.0` section and is **left
  untouched** — it accurately documents what that release shipped (`user:`/`group:` audiences,
  `{prefix}_IMPLICIT_GROUPS`); instead, add a new Unreleased entry recording the model change, the
  removal of `MICROMEGAS_IMPLICIT_GROUPS` (an operator-facing config break, pre-GA, noting it was
  introduced in v0.29.0), the new knobs, the new request/response fields, and the `allow_delegation`
  change.
  The **Minor breaking change** clause covers **published API only**, per the convention all 12
  existing clauses follow (each annotated "(published API, `path`)"):
  `micromegas_auth::policy`'s surface — `AudienceReadPolicy::new`/`AudienceMintPolicy::new` change
  shape, `MintPolicy::resolve_audience`'s `None` contract becomes an error, and
  `is_well_formed_audience` is replaced by `is_valid_audience` — plus the **removal of
  `pub fn implicit_groups_var`** (`micromegas_auth::default_provider`), which is the one item in this
  change that deletes a published symbol outright; and, separately attributed,
  `IngestionKeysState` (published, all-public-fields, `analytics-web-srv` — **not** `micromegas-auth`)
  gaining a required `default_audience` field. Deliberately **not** in the clause: `KeyRow`, which is
  private to `db_api_key.rs` (`:160-164`, no `pub`) so a new field breaks nothing, and `ApiKeyTable`,
  which only gains an additive method. Mention the two production
  `AudienceReadPolicy::new(vec![])` call sites in `flight_sql_server.rs` as *what needs updating*, not
  as part of the API break itself.
- `tasks/data_isolation/audience_based_access_control_plan.md` — replace the prefixed-audience model
  across every section [Files to Modify](#files-to-modify) enumerates for that file (not only §1–§3,
  the config table and the two deployment stories: also the vocabulary table, the target-state
  section's "the prefixes stay" and `∪ {user:<email>}`, the decided-encoding section, **Stage 1 step
  5's "no local grants table in v1" plus its write/read-collapse consequence**, the
  confidentiality-relaxation bullet, and the open-profile integration recipe). Record the two
  overrides named in the [Overview](#overview) as overrides rather than silently editing them away;
  note that the long-term grant-store section is now the direct continuation of what ships here; mark
  Stage 4 landed.
- `tasks/data_isolation/crypto_based_data_isolation_plan.md` — the sibling design doc, targets
  enumerated in [Files to Modify](#files-to-modify). Listed here too because one of them (`:113-118`'s
  `InstanceKind` prefix-based classification, plus `global` becoming a legal audience name) is a
  **design** change to that plan, not a vocabulary sweep, and should not be batched with the wording
  edits as if it were.

## Testing Strategy

**`rust/auth/tests/policy_tests.rs`** is rewritten wholesale, not merely extended: the model change
inverts the premise of all 13 of its existing tests, not just the four already called out for
`ownership_rewrite_config_tests.rs`. In particular: `read_policy_every_element_is_prefixed` (`:71`)
asserts every resolved audience starts with `user:`/`group:` — the exact property §1 abolishes, and
is deleted rather than adapted; `read_policy_resolves_singleton_when_no_groups` (`:40`) and
`from_env_with_unset_var_resolves_the_caller_singleton` (`:130`) assert the identity-derived
singleton `{user:alice@example.com}` that §2's grant-map lookup has no equivalent for, and are
replaced by the "no self rule" coverage below; `mint_policy_defaults_to_user_email_when_no_requested_audience`
(`:157`) asserts `resolve_audience(ctx, None) == "user:alice@example.com"`, which §2 changes to
`Err`, and is replaced by a test asserting exactly that `Err`; and
`mint_policy_rejects_a_malformed_audience_for_admin_and_non_admin` (`:233`) asserts
`"not-a-well-formed-audience"` is refused, which is a *valid* name under the new
`[A-Za-z0-9_-]` charset, so it is replaced with a still-invalid example (e.g. containing `:` or a
space). The remaining eight — `read_policy_resolves_union_of_identity_groups_and_implicit_groups`
(`:51`), `read_policy_resolves_empty_set_for_a_caller_with_no_grants` (`:89`),
`read_policy_service_account_grant_has_no_user_element` (`:103`),
`read_policy_service_account_with_empty_grant_resolves_implicit_only` (`:142`), and the four mint
tests at `:168`, `:186`, `:199`, `:213` — are each built on either identity derivation or the
implicit-groups list, so all eight are replaced too; the two whose *properties* must survive the
replacement (fail-closed exactness, `read_audiences` folding) get their own bullets below rather than
being lost with their old phrasing. The new tests below are what the file's contents become, not
additions alongside the old ones.

- `is_valid_audience` accepts `public`, `team-alpha`, `Team_Alpha`, `a`, and a 255-byte name;
  rejects `""`, 256 bytes, `alice@example.com`, `team alpha`, `a,b`, `["x"]`, `it's`. A companion
  test pins the *absence* of normalization — `team-alpha` and `Team-Alpha` resolve as two distinct
  audiences — since a later "helpful" case-fold would silently merge buckets.
- Resolve: `public` is always present; a `group:` selector matches a claim value; a `user:` selector
  matches the email; `*` grants everyone; an audience with no matching selector is absent; a caller
  gets **no** audience merely for being named like one (the absence of a self rule, tested
  explicitly — including an API key named `team-alpha`, which must not read `team-alpha`).
- **The fail-closed guarantee, restated for the new model.** `read_policy_resolves_empty_set_for_a_caller_with_no_grants`
  (`:89`) is today's version of it — a grantless caller resolves to the *empty* set, "never anything
  permissive". `public` being built in changes the value but not the property, so its successor
  asserts the exact set: a caller matching no selector anywhere resolves to **`{public}` and nothing
  else** — not a superset, not the empty set. Without an exactness assertion, "`public` is always
  present" would pass just as well for a policy that over-grants.
- **`read_audiences` still folds into the read axis**, per §2's formula. Two existing tests are its
  only coverage — `read_policy_service_account_grant_has_no_user_element` (`:103`) and
  `read_policy_service_account_with_empty_grant_resolves_implicit_only` (`:142`) — and both are
  written around identity derivation, so they are replaced rather than kept: a caller with
  `read_audiences: ["team-a", "team-b"]` and no grant-map entry resolves to
  `{public, team-a, team-b}`, which pins both the folding and the absence of a `user:` element.
- **The motivating case, end to end in one test**: resolve `alice-laptop` for bob ⇒ absent; add
  `{"alice-laptop": ["group:leads"]}` with bob in `leads` ⇒ present. *No data changed.*
- Mint: `read_audiences` never enters the mintable set; `PUBLIC_AUDIENCE` never enters a non-admin's
  mintable set even though it is always in their readable set; admins may mint any valid audience,
  `public` included; non-admins may not mint an audience they hold no **mint** grant for — including
  one they hold a bare-array (read-only) grant for, testing explicitly that a read grant never
  confers mint authority; a `"mint"` entry does grant it, independent of `"read"`.
- Malformed `{prefix}_AUDIENCE_GRANTS` / `MICROMEGAS_AUDIENCE_GRANTS` ⇒ `Err`, so a typo fails
  startup rather than shipping an inert knob. Shape errors: not an object; a per-audience value that
  is neither a bare array nor a `{"read": [...], "mint": [...]}` object; a non-array `read`/`mint`
  field; a non-string selector. **Content errors, per §2 — the ones a real operator actually
  writes**: a key that fails `is_valid_audience` (`{"group:everyone": ["*"]}`, the migration-from
  value, and `{"": ["*"]}`), a selector that is neither `*` nor `user:`/`group:`-prefixed
  (`["eng"]`, `["users:alice@example.com"]`, `["group:"]`), and a **repeated key**
  (`{"team-alpha": ["group:a"], "team-alpha": ["group:b"]}`, per §1 — the case a plain map
  deserialize would silently resolve to the *last* list, discarding a grant). Each must be an `Err`
  naming the offending key or selector, not a silently-inert entry.
- The prefixed-falls-back-to-unprefixed resolution gets its own tests — set only
  `MICROMEGAS_AUDIENCE_GRANTS` and confirm a prefixed `from_env` reads it; set both and confirm the
  prefixed name wins; the same pair `ownership_rewrite_config_tests.rs` has for `UNSTAMPED_AUDIENCE`
  (`:122`, `:141`). (Not "the same coverage as `implicit_groups_var`" — that helper has no tests at
  all, which is the gap this closes rather than the bar to match.)

**`rust/auth/tests/db_api_key_tests.rs`** (live-Postgres cases `#[ignore]`d, per the file's
convention): its `insert_live_key` helper (`:232-251`) currently inserts into `ingestion_api_keys`
with no `audience`, which every existing live test using it at its nine call sites (`:268`, `:302`,
`:341`, `:374`, `:398`, `:454`, `:497`, `:505`, `:546`) would fail at runtime against a v6-migrated
schema, since the column is `NOT NULL` with no default — a failure the compiler cannot catch.
`insert_live_key` gains an `audience: &str` parameter (every ingestion-table call site passes a
literal, e.g. `PUBLIC_AUDIENCE`), threaded into the `INSERT`'s column list. Note the helper's
`INSERT` is **table-generic** (`format!` over `table.table_name()`, `:240-243`), so this is not a
plain new parameter: the column list has to branch on the table, since the `:505` call site
(`ApiKeyTable::Analytics` in `live_surface_separation_both_directions`) targets a table with no
`audience` column and would otherwise fail on the added column.

One **existing assertion in this file inverts** and is not merely new coverage:
`live_row_authenticates_with_expected_context` asserts `assert!(ctx.allow_delegation)` at `:282` for
an `ApiKeyTable::Ingestion` key (`:268`/`:270`), which §4 makes `false`. It must be flipped to
`assert!(!ctx.allow_delegation)`, and it is the regression test for that half of §4 — the compiler
cannot catch it.

New **live** coverage — exactly one assertion: **the audience reaches `bound_audience` unchanged** (the
issue's first test). Justified because the loader builds its `RETURNING` list as a `&'static str`
chosen by `table.has_audience()` and then reads the column back with `try_get("audience")` **by name**
— a string-typed path end to end, where getting the table branch wrong yields a runtime
`LookupError::Db` rather than a compile error. There is no seam to inject a fake row (the query lives
inside `validate_request`'s moka closure, welded to `sqlx::query` + `PgPool`), and adding one would be
a larger change than the feature.

Two things previously listed here as new live coverage are **not**, and should not be written:

- **"The audience survives a cache hit" — cut.** It is tautological under this design: `KeyRow` *is*
  the cached unit, so if `KeyRow.audience` exists and `AuthContext` is built from `row.audience`, a
  cache hit cannot return anything else. It would test moka, not this change. (Nor should the
  neighbouring TTL claim in §4 be tested — asserting hand-edit latency needs a sleep or clock control,
  the exact antipattern `tasks/completed/1252_test_quality_timing_tests_plan.md` exists to remove.)
- **The unreachable-pool `ProviderUnavailable` cases already exist and are not live at all** —
  `:122`, `:142-151`, `:162-166`, `:179`, `:195-211`, all built on `unreachable_pool()` and none
  `#[ignore]`d. They keep passing; there is nothing to add.

New **non-live** coverage, and this is the important half: `ApiKeyTable::has_audience()` and the
`allow_delegation` derivation are **pure functions of the enum** (`matches!` over two variants, no
pool, no async), so they get plain unit tests — `has_audience()` is `true` for `Ingestion` and `false`
for `Analytics`, and the `allow_delegation` rule inverts that. Do **not** leave these covered only
transitively through the `#[ignore]`d live tests, which is how they are reached today. This is exactly
why `:282`'s inversion above is the most dangerous item in this change: `python3 build/rust_ci.py`
never runs `--ignored`, so a full-green CI run currently proves nothing about either property. A
three-line test on the enum moves both into default `cargo test`, and then the live assertion at
`:282` is confirmation rather than the sole guard.

**`rust/auth/tests/default_provider_tests.rs`**: same live-DB issue as `db_api_key_tests.rs` above —
its own `insert_live_key` helper (`:45-59`) inserts into `ingestion_api_keys` with no `audience`,
which its two call sites (`:109`, `:136`) would fail at runtime against a v6-migrated schema. Gains
the same `audience: &str` parameter, threaded through with a literal (e.g. `PUBLIC_AUDIENCE`); no
new coverage needed here, since this file's assertions are about `ProviderBuilder` construction and
existence checks, not `bound_audience`. (Its other, throwaway-schema table used for the
existence-query tests at `:196` is unaffected — it creates its own minimal `ingestion_api_keys` in
a throwaway schema the migration never runs against.)

**`rust/analytics-web-srv/tests/ingestion_keys_tests.rs`**.

**Test §5's resolution matrix against `resolve_audience` directly, not through the routes.** The
helper is sync, takes no pool, and reads only `state.default_audience` — the entire four-row table
(explicit / knob / `import`'s `PUBLIC_AUDIENCE` fallback / `mint`'s 400) is a **pure unit test**. This
matters because of how this file is built: every one of its nine non-`#[ignore]`d tests asserts a
403/400/503, i.e. a rejection that short-circuits *before* the `INSERT`, and its module doc states the
rule outright — "Every test here uses a lazily-connected pool … and **never actually reaches the
database**" (`:14-21`). So a route-level assertion that mint *succeeded* with audience X has to get
past the write, which forces it `#[ignore]`d and out of default CI. Testing the helper keeps the three
success rows in `cargo test` where they belong, and only the 400 row would have worked route-level
anyway. `resolve_audience` therefore needs to be `pub` rather than module-private — blessed explicitly
by `CLAUDE.md`'s "making a private item `pub` … is all acceptable", and the file already imports
`analytics_web_srv::ingestion_keys::{IngestionKeysState, ingestion_keys_router}` (`:24`) through the
crate's `[lib]` target, so nothing new is needed to reach it.

Route-level coverage then narrows to what only a route can add, all of it still non-live: an invalid
audience ⇒ 400 **before any DB access**; the no-audience-no-knob 400 actually surfaces as a 400 whose
body names `MICROMEGAS_DEFAULT_KEY_AUDIENCE`; and the `require_pool` → `validate_name` →
`resolve_audience` precedence below.

**No new live test here.** The immutability property — importing an already-present hash reports the
**existing** audience, never the request's — folds into `live_import_is_idempotent` (`:372`), which
already imports the same key twice and already asserts `imported: true` → `false` with a stable
`key_id`. Strengthen it instead: send a *different* `audience` on the second import and assert the
first one survives. That is two lines on a test that already does all the setup, and it is where the
`ON CONFLICT DO NOTHING` → fallback-`SELECT` path (`:397`/`:414`) is exercised — the twin-query hazard
that 500s if `audience` is added to one and not the other.

Two **existing** tests in this file change, beyond the 13 mechanical `IngestionKeysState` literals:

- `live_mint_list_revoke_round_trip` (`:309`, `#[ignore]`d) POSTs `{"name": …}` with no `audience`
  and asserts `CREATED` (`:327`) — under §6 that is now the 400 case. It gets an explicit
  `"audience"` in its body (the round-trip is about mint→list→revoke, not about defaulting), and its
  list assertion additionally checks the audience round-trips through `KeyListEntry`.
- `mint_503_when_pool_unconfigured` (`:218`) and `import_503_when_pool_unconfigured` (`:251`) keep
  asserting 503, which is what §6's stated `require_pool` → `validate_name` → `resolve_audience`
  order guarantees; they are the regression test for that order and must not be relaxed to accept a
  400.

`live_import_is_idempotent` (`:372`) is the one existing live test that gains a real assertion rather
than just the state-literal field — see the immutability point above. Its first import still sends no
audience and still succeeds, now defaulting to `public`.

**Migration** — new `rust/ingestion/tests/sql_migration_test.rs` (this crate has no migration test
today; `analytics-web-srv/tests/migration_test.rs` covers the unrelated **app_db** v3→v4 chain and
is a *style* reference only — same live-DB, `#[ignore]`d convention, different database and
different `execute_migration`): against a live data-lake DB seeded with a v5-era row,
`execute_migration` leaves the row's audience `public`, not NULL (the issue's "backfills … rather
than staying NULL"), leaves `read_data_lake_schema_version` reporting **6** (which is also what
catches a forgotten `UPDATE migration SET version=6;` — see §3 — before it becomes a startup panic),
and a hand-written `INSERT` of `''` is rejected by the `CHECK`.

**Why this one has to be live, and why it is worth adding at all** — this is the only *new* live test
file in the change, so it should carry an explicit justification rather than inherit the file
convention:

- **No cheaper mechanism exists.** The workspace has no `testcontainers`, no embedded Postgres, no
  migration harness of any kind (checked across every `Cargo.toml`). For anything whose subject *is*
  Postgres behavior, an `#[ignore]`d live test is the only tool available.
- **The subject genuinely is Postgres behavior.** `ADD COLUMN` → `UPDATE` → `SET NOT NULL` ordering,
  and ARE regex evaluation inside a `CHECK`, have no Rust-side representation to unit-test. There is
  no pure function here to extract, unlike §5's `resolve_audience`.
- **The load-bearing assertion is the third-copy check.** "Valid audience" is stated in three places
  (§3: `policy.rs`, `read_scope.rs`, this `CHECK`), and `micromegas-ingestion` **cannot** depend on
  `micromegas-auth` — so the SQL literal and the Rust validator are structurally independent, with no
  shared constant to bind them. A live comparison is the only executable link between them. Assert
  agreement on a couple of representative values (`''`, and one containing `:`), **not** the full
  accept/reject table, which the `is_valid_audience` unit tests already pin — a second enumeration can
  rot in one place while passing in the other.
- **Argued against, and accepted anyway.** v6 is a first for this crate in two ways — the first
  migration that backfills data, and the first that adds a constraint (v1–v4 create tables or add a
  column *with* a default; v5 creates). Those are the new risk classes. But note honestly that v5,
  which created these very tables, shipped with no migration test, and that an `#[ignore]`d test has
  near-zero *regression* value since nothing runs it automatically. Its value is one-time, at
  implementation, plus documenting the issue's own acceptance criterion ("backfills … rather than
  staying NULL") executably for whoever writes v7. That is a modest but real return for one small file;
  if it were any larger, the Manual section would be the better home.

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
keeps its coverage. The file has exactly ten tests, so the other **six** are unaffected
(`unset_vars_resolve_to_default` `:43`, `all_whitespace_unstamped_audience_resolves_to_none` `:66`,
and the four `public_view_sets_*` tests at `:160`, `:178`, `:200`, `:220`) — in particular
`all_whitespace_unstamped_audience_resolves_to_none` (`:66`) survives **only** because `from_env`
trims and short-circuits on empty *before* validating (`read_scope.rs:169`); step 1 must keep that
ordering, or `"   "` becomes a malformed audience and this test inverts too.

**Python** — new behavior: `read_keyring` triples with and without a per-entry audience; the
`--audience` + `--table analytics` guard, including that `--audience ""` is rejected rather than
silently omitted; the per-entry `"audience"` + `--table analytics` guard, run before any import call
(same rejection, checked up front rather than mid-batch — and firing even when `--only`/`--exclude`
would have dropped the offending entry); per-entry precedence over `--audience`; `import_one` calls
`import_analytics_api_key(name, key)` with no `audience` argument on the analytics branch. Plus
`web_client.py` payload tests — the `audience` field omitted when `None`, included when set —
following the omitted/empty-string/set triple `test_web_client.py` already has for `folder_path`
(`TestCreateScreenFolderPath` `:18-35`, mirrored by `TestUpdateScreenFolderPath` `:38-55`), reusing
its `_make_client()` MagicMock helper (`:8-15`); that file has no coverage of either import method
today.

The tuple-arity change also breaks `python/micromegas/tests/cli/test_import_keys.py` at runtime
(Python catches none of this at import time, unlike the Rust helpers above), so this is a listed
step, not an implied one:

- `FakeClient.import_ingestion_api_key(self, name, key)` (`:29`) and `import_analytics_api_key`
  (`:32`) are 2-arg, and `_handle` records `(name, key)` (`:23`) — a positional `audience` raises
  `TypeError`. Same for the ad-hoc `Client` at `:354-361`.
- `make_args` (`:36-47`) has no `"audience"` default, so every test built from it hits
  `AttributeError` the moment anything reads `args.audience`.
- 2-tuple literals and equality assertions: `:98`, `:107`, `:121`, `:137`, `:144`, `:190` (the
  `read_keyring` assertions), `:218`'s `ENTRIES` feeding `select_entries` (`:223`, `:228`, `:236`,
  `:245`, `:251`), `run_import`'s literals (`:282`, `:292`, `:308`, `:321`, `:342`, `:364-365`), and
  the recorded-call assertions (`:324`, `:345`, `:366`). Note `:228` and `:236` are the
  `assert import_keys.select_entries(...) == [` statement heads — the tuple literals themselves are on
  `:229-230` and `:237-238`, so editing only the cited lines misses them.

**Web app** — `mint` omits `audience` when unset and includes it when set. **Two** existing tests pin
the exact mint body with `toEqual({ name: 'new-key' })`, not one — `IngestionApiKeysPage.test.tsx:146`
and, identically, `AnalyticsApiKeysPage.test.tsx:146` — so the omission must be strict on both pages:
sending `audience: ''` for a blank input breaks the first, and any unconditional `audience` key breaks
the second. (`JSON.stringify` drops an `undefined` value, so `audience: undefined` is safe; `''` and
`null` are not.) Since it is the *shared* `createApiKeysApi` closure that changes, the analytics
assertion is the regression guard for that sharing and stays untouched. Also: the Audience column
renders on the ingestion page, with `'—'` for a row whose
`audience` is `undefined`; and the analytics page does not regress — asserted on **both** axes, no
Audience column *and* no audience input in its mint dialog, since `showAudience` gates both. The
knob-naming 400 renders through `config.ErrorClass` rather than `handleMint`'s generic
`'Failed to mint key'` fallback (`ApiKeysAdminPage.tsx:119-120`). All of this lands in the two route
tests (`src/routes/__tests__/{Ingestion,Analytics}ApiKeysPage.test.tsx`) — there is no
`ApiKeysAdminPage` test file, and the two `lib/__tests__/*-api-keys-api.test.ts` files cover `list`
only.

**Manual** — `start_services.py --monolith`, mint a key with an explicit audience through the web UI,
ingest with it, confirm the audience reaches the auth context (trace-level `db api key validated`);
end-to-end stamping is #1373.

Full CI: `python3 build/rust_ci.py`, `python3 build/python_ci.py`, `yarn lint && yarn type-check &&
yarn test`.

## Open Questions (resolved)

1. **Split the model change out of #1372?** — **No.** Steps 1–2 amend #1369's model in this same
   plan/PR rather than a companion issue landing first: one PR, values never persist in the old
   vocabulary. See [Scope note](#scope-note-this-amends-shipped-stage-1).
2. **How does a privacy deployment provision per-user audiences?** — **It doesn't, not in this
   plan.** With no self-audience rule, each user needs an audience plus a grant, and the env map
   isn't a writable store a route can update. Rather than bring the grant store forward or have a
   deployment script the map by hand, this is deferred wholesale to Stage 6 (#1374): a privacy
   deployment gets no per-user audience provisioning until users can mint their own keys, at which
   point the mint route is the natural place to create the matching grant. The charset-widening
   alternative (add `@`/`.`, restore the email-keyed self rule) is not taken.
3. **Should `public` be neutralizable?** — **Not for now; keep it simple.** `public` stays a
   built-in, non-removable read grant exactly as designed in [§2](#2-access-is-a-grant-map-with-a-readmint-axis-prefix_audience_grants).
   No startup check or mint-time refusal is added. Revisit if a deployment actually needs to
   eliminate it.
