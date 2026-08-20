# AbAC: audience-based access control for telemetry

> **Supersedes** [`per_user_data_isolation_plan.SUPERSEDED.md`](per_user_data_isolation_plan.SUPERSEDED.md). That document analyzed the
> per-user case and remains the reference for the confidentiality/integrity analysis and the
> current-state audit; this plan generalizes its mechanism. Where they disagree, this document wins.

> **Revised 2026-07-30** for deployment staging (issue #1334 follow-up): the `self|audience` mode
> enum and its `self` default are replaced by a single grant-configured policy engine (open
> deployments are an `everyone`-group configuration, not a query-side bypass); implementation steps
> are re-ordered into deployment stages so existing team-wide deployments migrate with zero behavior
> change; and the code audit is refreshed (see Appendix B for drift found since 2026-07-21).

> **Re-staged 2026-07-30**: the DB-backed key store is split out of Stage 4 into a new **Stage 0**
> that depends on nothing and ships standalone operational value (revocation/rotation without
> redeploy, no cleartext keys in env); Stage 4 keeps only the `audience` column, which genuinely
> needs the policy seam. The env keyring's per-request full scan cannot scale to per-user keys, so
> the store is a precondition for Stage 6, not a convenience.

> **Keys track revised 2026-07-30** (issue #1383 discussion): write and read credentials get
> **separate tables** (`ingestion_api_keys` / `analytics_api_keys`) — one key is never valid on both
> surfaces, because the risk is asymmetric and only ingestion keys carry an `audience`. Key
> management is an **OIDC-authenticated HTTP API**, pulled forward from
> Stage 6 into Stage 0 (create/revoke/list, no audience); Stage 6 now only adds audience resolution
> to a route that already exists. Admin-gated lakehouse UDFs were considered for key management and
> rejected — see Stage 0. (The API shipped on ingestion and has since moved to `analytics-web-srv`
> — see the 2026-08-12 note below.)

> **Long-term model recorded 2026-08-12.** The grant source in v1 is flat IdP membership, where a
> `groups` claim entry *is* the grant. The target model separates membership from grants — users
> belong to groups, groups nest, and a group is granted a set of audiences it may read — and today's
> rule is that model's degenerate "identity grant" case. See
> [Long-term model](#long-term-model--groups-nested-membership-and-grants). Nothing in Stages 1–7
> changes shape for it, provided Stage 1 keeps four properties listed there.

> **Analytics keys are service accounts (decided 2026-08-12).** This plan previously asserted that
> read scope never comes from a key and that `analytics_api_keys` never gains an audience column.
> That is superseded: analytics keys are **service-account credentials with a configurable set of
> readable audiences** (`read_audiences`), landing as new **Stage 4b** — the read-side mirror of
> Stage 4. Ingestion keys still carry exactly one *write* audience; the two columns have opposite
> meaning and cardinality. Affected paragraphs (Stage 0 two-table rationale, Stage 4 step 9,
> Security, Resolved Decisions) are updated in place.

> **Renamed 2026-07-30** from "policy-based data isolation". The design was described as *RBAC*
> throughout, but it has no roles — see [Naming](#naming) below. The model is **AbAC,
> audience-based access control**; `Rbac*` identifiers become `Audience*`.

> **Stage 5 landed (#1373).** Ingestion now stamps `micromegas.audience` server-side from
> `AuthContext.bound_audience` at both process-insert sites, stripping any client-supplied
> `micromegas.*` property, and OTLP-derived `process_id`/`block_id` are audience-scoped (folded
> in alongside the stamp, not a separate follow-up) to close the cross-audience collision the
> stamp would otherwise expose. Full design in `tasks/1373_ingestion_stamping_plan.md`; see the
> revised Stage 5 section below for what changed from the original step 11 sketch — notably, that
> step's premise that OTLP/Firehose had no auth wiring was stale (OTLP already ran under
> `auth_middleware`; Firehose already authenticated but discarded the resulting context). The
> cross-audience `insert_stream`/`insert_block` write-injection gap (§7 of the 1373 plan) is
> tracked as a follow-up issue (Stage 5b), not closed by this stage.

> **Mint surface moved 2026-08-12** (#1411 / #1458, `tasks/completed/simplify_ingestion_api_key_admin_plan.md`).
> This plan was written assuming key management lives on the **ingestion** service at
> `POST/GET/DELETE /auth/api_keys`. Those routes shipped in Stage 0 and were then **removed**:
> `rust/public/src/servers/api_keys.rs` is deleted and key management now lives on
> **`analytics-web-srv`** at `{base_path}/api/ingestion-api-keys` (plus `/api/analytics-api-keys`),
> writing both tables directly through the telemetry-DB pool it already opens. The affected
> paragraphs below (Stage 0c/0d, Stage 4 step 10, Stage 6 step 12, Files to Modify, Testing,
> Resolved Decisions) are updated in place; see **Appendix C** for the full drift audit. Nothing in
> §1–§6 changes — only *where* `MintPolicy::resolve_audience` gets called.

> **Stage 6a landed (#1489) 2026-08-19.** The audience grant map moves out of startup env config
> (`{prefix}_AUDIENCE_GRANTS`) and into a new DB-backed store, `audience_grants` (data-lake schema
> v7) — a flat, selector-based table that is a 1:1 stand-in for the long-term model's
> `group_read_grants`/`group_mint_grants` tables described above, not an implementation of nested
> groups itself. This is what makes a per-user grant creatable without a service restart: the env
> map is kept as the static/bootstrap layer, unioned additively with the store. Full design in
> `tasks/1489_db_audience_grant_store_plan.md`.

> **Stage 6 landed (#1374) 2026-08-20.** `POST {base_path}/api/ingestion-api-keys` is no longer
> purely `AdminUser`-gated: a non-admin caller with a matching `mint` grant can now mint their own
> key, with `MintPolicy::resolve_audience` (Stage 1, #1369; `AudienceMintPolicy`, Stage 4, #1372)
> as the authorization — the trait's first production call site. Mint-side grants are resolved by
> an uncached, per-request point query against `audience_grants`, deliberately asymmetric with the
> read side's ~60s cached snapshot (Stage 6a): a mint audience declared only in
> `{prefix}_AUDIENCE_GRANTS` is invisible to this stage's claim logic and so is never honored for
> mint. A non-admin caller who names a brand-new, never-before-granted audience explicitly claims
> it atomically as part of the same mint request (writing `user:<email>` on both the `mint` and
> `read` axes), rather than requiring an admin to pre-create the grant — this is what makes minting
> actually self-service given Stage 6a's grant store has no non-admin write path otherwise. Both the
> grant-based mint path and the lazy claim are gated behind one off-by-default knob,
> `MICROMEGAS_SELF_SERVICE_MINT`, plus two per-caller bounds
> (`MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER`/`MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER`),
> so an existing deployment's authorization surface never silently widens on upgrade. A new
> `GET {base_path}/api/audience-grants/my-audiences` route (caller-scoped, not admin-gated) lets a
> non-admin caller discover what they can mint into, and a new `micromegas-setup-telemetry` CLI
> does an OIDC login, mints a personal key, and prints the OTLP exporter env vars needed to send a
> user's own telemetry — prefixing a non-admin's fresh claim into a namespace derived from their
> own email, a client-side convention that shrinks (but does not eliminate) the audience-squatting
> surface an unauthenticated name choice would otherwise have. Full design in
> `tasks/1374_self_service_mint_plan.md`.

## Overview

Isolate telemetry so that data produced under one identity is only **readable** by principals
authorized to read it. The mechanism is a small, general **AbAC seam**. There is **no mode enum and
no default policy**: one policy engine, configured by grants, spans the whole deployment spectrum —
from an **open (team-wide) deployment**, where an implicit `everyone` group preserves today's
everyone-reads-everything behavior with no data migration, to a **privacy deployment**, where each
user reads only their own data and sharing happens through the IdP `groups` claim. Moving along
that spectrum is configuration, not code — no API change, no rewrite of the enforcement rule.

The design rests on one structural decision: model everything as a **generic principal stamped on
data** (`audience`) plus a **set-valued read check** (`audience IN (readable principals)`), never an
equality (`owner = caller`). In a privacy deployment with no group grants the readable set is a
singleton, so the executed query plan is identical to the simple per-user design — but the code is
already the general form and never has to be reworked.

### The three-relation model

Authorization decomposes into three independent relations:

1. **`write(subject → group)`** — checked **once, at mint time** (key issuance). Grants the right to
   mint an ingestion key that stamps `audience = group`.
2. **`read(subject → group)`** — checked **on every query**. Grants visibility of data whose
   `audience = group`.
3. **`audience`** — the label physically stamped on data by the key; the link between the two.

In v1 both grants are read off flat IdP membership, which makes `read(subject → group)` and
"may read audience `group:G`" the same statement. The target model splits them —
`membership(subject → group)`, transitive, and `read_grant(group → audiences)`, many-to-many — with v1 as
the degenerate case where each group grants exactly its own label. See
[Long-term model](#long-term-model--groups-nested-membership-and-grants).

**Per-user isolation is the restriction of this model to singleton self-groups**: every principal is
its own group, with `write(u → u)` and `read(u → u)` the only grants, and `audience` always the
minter's own email. **Open (team-wide) deployments are the other end of the same model**: an
implicit group `everyone` that every authenticated principal belongs to, with shared data stamped
`group:everyone` — including the audience assigned to existing ingestion API keys when they are
imported into the key store (Stage 0 imports them; Stage 4 assigns the audience). Every point on the
spectrum runs the *same* code; only the configuration differs.

### Load-bearing property preserved

Confidentiality rides entirely on **OIDC identity + `ReadPolicy`**, evaluated per-query. The write
key governs **integrity only**:

- A stolen write key lets an attacker *write* data labeled with some `audience` (pollutes that
  audience's view — an integrity problem). It grants **zero** read power: reading requires the
  caller's own OIDC identity to satisfy `read(caller → audience)`.
- There is no write→read escalation: a holder of `write(caller → G)` who lacks `read(caller → G)`
  cannot read G — not even the rows they themselves wrote (audience is `G`; reading `G` needs
  `read`).

Because the write grant is frozen into the key at mint and ingestion never re-checks it, **write
keys can be eternal** (the current use case). No per-write policy lookup; no `minted_by` bookkeeping
in v1 (see Deferred / Trade-offs for when that changes).

### Naming

This model is **AbAC — audience-based access control**. Earlier revisions called it RBAC, which was
wrong: there is no role anywhere in the design. RBAC's defining feature is the indirection
`subject → role → permission`, where a role is a named, reusable bundle of permissions. Nothing
here bundles permissions. A subject carries a set of audiences; data carries one audience; access
is set membership.

Structurally the closest industry analogue is AWS-style **ABAC** (attribute-based access control):
a tag on the resource matched against a tag on the principal, exactly like
`aws:PrincipalTag/team == aws:ResourceTag/team`. This design is that pattern degenerated to a
single attribute — hence the distinct casing **AbAC**, and hence *audience*-based rather than
*attribute*-based: naming the one attribute is more honest than implying a general attribute
engine with conditions and a deny effect, none of which exist here.

Vocabulary used throughout:

| Term | Meaning |
|---|---|
| **audience** | the label stamped on data at ingestion — an opaque `[A-Za-z0-9_-]{1,255}` name (e.g. `team-alpha`), no `user:`/`group:` prefix, no principal semantics (revised by Stage 4, #1372 — see the status update above; this row originally read `user:<email>` or `group:<id>`). Distinct from `AuthContext.audience` (the OIDC token audience); see "Naming collision to avoid" below. |
| **grant** | one of the two relations — `write(subject → group)` at mint, `read(subject → group)` at query. As of Stage 4 (#1372), both are resolved from an explicit `AudienceGrants` map (`{prefix}_AUDIENCE_GRANTS`) rather than derived from IdP group membership (this row's original claim) — see the status update above. |
| **readable set** | the audiences a caller may read; the `ReadScope` resolved per request. |
| **role** | reserved for the *capability* axis (`is_admin`, issues #1376/#1377), which is orthogonal to audience scope. Not used for data isolation. |

Two names deliberately **not** used: *RBAC* (no roles — and reserving the word keeps the admin
capability axis distinct), and the bare acronym *ABAC* (taken by attribute-based access control;
write "audience-based access control" or `AbAC`, never `ABAC`).

Roles would become meaningful here if read and write ever separate: today group membership grants
both, so there is nothing for a role to bundle. If the deferred grants table or a second write-role
claim lands (see Deferred / Trade-offs), `viewer`/`minter` per group becomes a real role and the
term earns its place.

### Status update (2026-08-17): Stage 4 (#1372) landed, revising the audience model

Stage 4 (below) landed and, per its own plan's "Scope note," amended §1–§3 of this document rather
than waiting for the long-term grant store: **an audience is now an opaque label
(`[A-Za-z0-9_-]{1,255}`, no normalization) with no `user:`/`group:` prefix, and there is no
identity-derived audience anywhere** — not `{user:<email>}`, not a mintable/readable set derived
from `caller.groups`/`MICROMEGAS_IMPLICIT_GROUPS`. Access is a separate, explicit grant map
(`AudienceGrants`, `{prefix}_AUDIENCE_GRANTS`), with `public` the sole built-in read grant.
`MICROMEGAS_IMPLICIT_GROUPS` is removed. This is **two deliberate overrides** of what this document
originally said, recorded here rather than silently edited away:

- **§2's "the prefixes stay" claim (below) is overridden.** `[A-Za-z0-9_-]` makes
  `user:alice@example.com` unrepresentable; the collision concern it named is answered instead by
  audiences living in a single flat namespace with byte-exact identity, and the default-grant
  concern by an explicit grant entry.
- **§2's target formula's `∪ {user:<email>}` term (below) is abolished** — the "no self-audience
  rule" decision. Reasons: the charset removes email as a candidate value, and keying the rule on
  `subject` instead would let an admin mint themselves read access by naming a key after an
  audience (API-key principals have no email but do have `subject`).

Neither override changes this document's long-term grant-store end state (`group_read_grants`
/ `group_mint_grants` below) — Stage 4's env-map grants are explicitly a 1:1 stand-in for those two
tables, kept in one map only because no store exists yet to split them across. §1–§3 below are left
as the historical record of the model *as shipped in Stage 1* (#1369); read them with the override
above in mind. See `tasks/1372_audience_on_keys_plan.md` for the full design and
`rust/auth/src/policy.rs` for what actually ships.

## Current State

(Condensed from the superseded plan; verified against current code.)

### Query path — authentication real, authorization absent
- `make_session_context()` takes **no caller identity**
  (`rust/analytics/src/lakehouse/query.rs:186-228`) and executes SQL verbatim (`ctx.sql(sql)`,
  `flight_sql_service_impl.rs:389`).
- The only injected analyzer rule is `TableScanRewrite`
  (`rust/analytics/src/lakehouse/table_scan_rewrite.rs`), which adds **time-range predicates only**
  and, critically, **only rewrites `MaterializedView` table providers** — it early-returns
  `Transformed::no` for table functions (`table_scan_rewrite.rs:37-43`). Any ownership rule must
  handle the table-function case explicitly.
- Per-request caller identity **is** available: `validate_and_resolve_user_attribution_grpc` yields
  `attr.user_email` before the session context is built (`flight_sql_service_impl.rs:317`,
  used for audit at `349-366`). It is never passed to `make_session_context`.
- `SessionConfigurator` (`rust/analytics/src/lakehouse/session_configurator.rs`) is a per-**service**
  object (`self.session_configurator.clone()`), not per-request — it cannot by itself carry
  per-caller identity.

### Ingestion path — key gate, identity discarded
- API keys come from the static `MICROMEGAS_API_KEYS` env var
  (`rust/auth/src/api_key.rs`), parsed into an in-memory `HashMap<Key, name>`; constant-time compare;
  no runtime add/revoke. On match: `AuthContext { subject: name, email: None, issuer: "api_key",
  audience: None, expires_at: None, auth_type: ApiKey, is_admin: false, allow_delegation: true }`
  (`api_key.rs:116-127`).
- Providers compose via `MultiAuthProvider` in `default_provider::provider_with_prefix`
  (`rust/auth/src/default_provider.rs:51-119`).
- No ingestion handler reads `AuthContext`; identity gates the request and is dropped.

### Data model — no owner dimension
- `processes` table (`rust/ingestion/src/sql_telemetry_db.rs`): no owner/tenant column.
- `processes_view` is a SQL view exposing `process_id … properties`
  (`rust/analytics/src/lakehouse/processes_view.rs`). Properties are row-level queryable via
  `property_get` but cannot prune partitions.

### Naming collision to avoid
`AuthContext` **already has** an `audience` field (`rust/auth/src/types.rs:26`) — it holds the *OIDC
token audience* (API audience / client id). The data-isolation principal is a **different concept**.
To avoid confusion, this plan uses:
- `audience` — the principal **stamped on data** (process property / column). New concept.
- `bound_audience: Option<String>` — a **new** `AuthContext` field: the principal a credential is
  bound to write as. Do **not** overload the existing `audience` field.

## Design

Four seams. Two are trait objects (the policy seam); two are the mechanical stamp + enforce.

### 1. `MintPolicy` — who may stamp which audience (mint-time, ingestion side)

```rust
/// Resolves the audience a mint request is permitted to bind to a key.
#[async_trait]
pub trait MintPolicy: Send + Sync + std::fmt::Debug {
    /// `caller` is the authenticated OIDC context of the mint request.
    /// `requested` is the optional audience the caller asked for.
    /// Returns the audience to bind, or Err if not permitted.
    async fn resolve_audience(
        &self,
        caller: &AuthContext,
        requested: Option<&str>,
    ) -> anyhow::Result<String>;
}
```

**As shipped in Stage 1 (#1369), superseded by Stage 4 (#1372) — see the status update above.** The
shipped-then impl (`AudienceMintPolicy`) permitted `requested` iff it was in the caller's **mintable
set**: `{user:<caller email>} ∪ {group:G : G ∈ caller's IdP groups claim} ∪ {group:G : G ∈
MICROMEGAS_IMPLICIT_GROUPS}`. With `requested = None`, the audience defaulted to
`user:<caller email>`. As of Stage 4, the mintable set is instead the grant map's `mint` list for
the requested audience (`AudienceGrants`, admin callers exempted per the admin arm below), there is
no identity-derived term of any kind, `MICROMEGAS_IMPLICIT_GROUPS` no longer exists, and
`requested = None` is always an `Err` — there is no "myself" audience for an opaque label to
default to.

### 2. `ReadPolicy` — which audiences a caller may read (query-time, flight-sql side)

```rust
/// Resolves the set of audiences a caller is permitted to read.
#[async_trait]
pub trait ReadPolicy: Send + Sync + std::fmt::Debug {
    async fn readable_principals(&self, caller: &AuthContext) -> anyhow::Result<ReadScope>;
}

/// Result of a ReadPolicy. Explicit `All` variant models the daemon bypass
/// without a magic sentinel string.
pub enum ReadScope {
    /// Unfiltered. Produced ONLY for the internal maintenance daemon — never for a user session.
    All,
    /// Filter to `audience IN (principals)`. May be a singleton (per-user default).
    Principals(Vec<String>),
}
```

**As shipped in Stage 1 (#1369), superseded by Stage 4 (#1372) — see the status update above.** The
shipped-then impl (`AudienceReadPolicy`) returned the caller's **readable set** as:

```
ReadScope::Principals(
    caller.read_audiences                          // grant carried by a service-account credential
  ∪ {user:<caller email>}   if email present       // human OIDC caller
  ∪ {group:G : G ∈ caller's IdP groups claim}
  ∪ {group:G : G ∈ MICROMEGAS_IMPLICIT_GROUPS}
)
```

As of Stage 4, this is instead a pure grant-map lookup —
`{public} ∪ {a : selector ∈ grants[a].read matches caller} ∪ caller.read_audiences` — with `public`
the sole built-in and no identity-derived term (no `∪ {user:<email>}`, no implicit groups).

The union is **branch-free — no `auth_type` check anywhere**: an OIDC caller carries no
`read_audiences`, and an API key carries no email and no groups claim (`api_key.rs:116-127`,
`db_api_key.rs:318-328`). `caller.read_audiences` is the analytics-key service-account grant
(Stage 4b); it is empty for every OIDC principal and for any key minted without a grant, so unset
config stays fail-closed.

`resolve` is `async` and fallible because the long-term grant source is a store, not a claim — see
[Long-term model](#long-term-model--groups-nested-membership-and-grants). Callers must **deny on
`Err`**; a resolution failure is never an empty-or-permissive scope.

`ReadScope::All` is **never** produced by this policy — it exists only for the internal maintenance
daemon's contexts (§5). Under the shipped-then (Stage 1) model, a privacy deployment (no implicit
groups, no groups claim) resolved the singleton `{user:<caller email>}`; an open deployment
(`MICROMEGAS_IMPLICIT_GROUPS=everyone`) resolved `group:everyone` for every caller. As of Stage 4,
the equivalent profiles are: privacy — an explicit grant map, no self rule, so per-user isolation
needs a per-user audience *and* a grant, deferred to Stage 6 (#1374); open — every caller's set
already includes `public` (the built-in), with `MICROMEGAS_UNSTAMPED_AUDIENCE=public` covering
never-stamped legacy data — no grant map entry needed at all. See
`mkdocs/docs/admin/authentication.md#audiences-and-grants` for the worked profiles.

### 3. Ingestion stamps `audience`

- The mint route runs `MintPolicy::resolve_audience` **once** and records the resolved audience on
  the key (env keyring: not applicable — see key-store note; DB keyring: an `audience` column). That
  route is `POST {base_path}/api/ingestion-api-keys` on `analytics-web-srv`
  (`rust/analytics-web-srv/src/ingestion_keys.rs::mint_key`), **not** ingestion — see the 2026-08-12
  note at the top. The import route in the same module takes the same treatment (Stage 4 step 10).
- Key auth sets `AuthContext.bound_audience = Some(key.audience)`.
- Ingestion handlers (native `rust/public/src/servers/ingestion.rs`, OTLP
  `rust/public/src/servers/otlp.rs`) read `AuthContext.bound_audience` (currently discarded) and
  write it onto the process. **No policy lookup at write time** — the audience is already vetted and
  frozen into the key.
- Client-supplied `process.owner` / `host.*` stay **display metadata only**, never the audience.
  (Note: OTel already lands these as `otel.resource.process.owner` / `otel.resource.host.name`
  properties — `otel-ingestion/src/block.rs:467-475`. Those remain display-only; the trusted
  `micromegas.audience` is written server-side from `bound_audience`.)

**Audience value shape (resolved, Q4).** Property values are arbitrary `TEXT`; `property_get` returns
dict-encoded Utf8 usable directly in `IN` predicates (case-insensitive key match). No user/group
discriminator exists in the codebase today, so **namespace the value**: `user:<email>` for personal
audiences, `group:<id>` for groups. This prevents a group id from ever colliding with a user email in
the one `audience` field and makes intent explicit to consumers. The key name `micromegas.audience`
follows the existing dotted-namespace precedent (`otel.resource.*`).

Storage of the stamped audience (v1 vs later) mirrors the superseded plan's open decision:
- **v1: reserved property** `micromegas.audience` on the process — zero schema migration, flows
  through existing property plumbing. In-tree usage of `property_get` in WHERE predicates is equality
  only (`rust/public/src/client/query_processes.rs:73`); the `IN (...)` form relies on DataFusion's
  dictionary-type coercion (`property_get` returns `Dictionary(Int32, Utf8)`,
  `rust/datafusion-extensions/src/properties/property_get.rs:48,87-92`).
- **later: first-class `audience` column** on `processes` + propagate through views — enables
  partition pruning and a physical boundary.

### 4. Query enforcement — two prongs (resolved by research; see Appendix A)

Enforcement **cannot** be a single analyzer rule. UDTF table functions surface as
`LogicalPlan::TableScan`, but the span/metadata functions (`process_spans`, `perfetto_trace_chunks`,
`list_partitions`, `parse_block`) **do not carry their owner id in the output schema**, bake the
`process_id`/`stream_id` opaquely into the provider at plan time, and some ignore pushed-down filters
(`process_spans_table_function.rs:384`). A predicate-injecting rule has no column to filter on for
them. So enforcement is two-pronged, both fed the same per-request `ReadScope`:

**Prong A — `OwnershipRewrite` analyzer rule** (for `MaterializedView`-backed scans). A new mandatory
`AnalyzerRule` beside `TableScanRewrite`, non-bypassable (operates on the logical plan below the SQL
text). Constructed with the resolved `ReadScope`.
- `ReadScope::All` → no-op (bypass; see §5).
- `ReadScope::Principals(ps)`:
  - **`processes` view** (carries `audience` as a property in v1): `audience IN (ps)` via
    `property_get(properties, 'micromegas.audience') IN (ps)`. When
    `MICROMEGAS_UNSTAMPED_AUDIENCE` is configured, the effective audience is
    `coalesce(property_get(properties,'micromegas.audience'), '<unstamped>')` — data ingested
    before stamping existed is attributed to the configured audience at query time (the
    zero-migration path for open deployments: set it to `group:everyone` and legacy data stays
    visible with no backfill and no retention wait). Unset (privacy deployments), `NULL` audiences
    fail the `IN` and unstamped data is hidden — fail-closed.
  - **`process_id`-keyed views** (`streams`, `blocks`, `log_entries`, `measures`, span views):
    semi-join, **not** a materialized id list —
    `process_id IN (SELECT process_id FROM processes WHERE <same audience predicate as above>)`.
    No ceiling on owned processes (streaming-friendly; matches the project's no-hard-limits stance).
  - **`view_instance('<set>', <id>)`** already surfaces as a `TableScan<MaterializedView>` and is
    caught by this rule exactly like a named view — the same predicate applies. (This is why the
    existing `TableScanRewrite` can already rewrite `view_instance`.)
  - **Public view sets (opt-in):** if the scanned view set is on the public allowlist, inject **no**
    predicate — see §5b. Default allowlist is empty, so this branch is inert unless configured.

**Implemented (Stage 2, #1370) — corrections to the sketch above, recorded here now that Prong A has
landed as `OwnershipRewrite` (`rust/analytics/src/lakehouse/ownership_rewrite.rs`):**
- **One audience per process, not per row.** The `process_id IN (SELECT process_id FROM processes
  WHERE <predicate>)` construction above re-admits a process via *any one* of its historical
  (possibly pre-stamping, unstamped) partition rows — including its own `processes` scan, since a
  per-row filter there leaks the same way. The shipped construction instead first collapses
  `__processes__partitions` to one row per `process_id` via `Aggregate(GROUP BY process_id,
  MAX(audience) AS resolved_audience)` (`MAX` over a nullable column ignores `NULL`s, so a stamped
  row always outranks an unstamped one), then filters *that* — uniformly, including `processes`'s
  own scan, which gets no separate per-row branch. This assumes a process is stamped with at most
  one distinct audience over its lifetime; Stage 3 (#1371) should revisit if that assumption
  changes.
- **`async_events`/`thread_spans` are covered by Prong A, not deferred to Prong B's caches.** Both
  are process/stream-scoped but carry no `process_id` (`async_events`) or `process_id`/`stream_id`
  (`thread_spans`) column to semi-join on. Rather than leaving them unfiltered until Prong B's
  caches land, `OwnershipRewrite` covers both now via a literal-valued, uncorrelated `EXISTS`
  keyed on `MaterializedView::get_view().get_view_instance_id()` (the process_id string for
  `async_events`; the stream_id string for `thread_spans`, resolved through `streams` into its
  owning process) — a plan-time literal either way, costing no runtime cache.
- **`OwnershipRewriteConfig` (`MICROMEGAS_UNSTAMPED_AUDIENCE`/`MICROMEGAS_PUBLIC_VIEW_SETS`) rides on
  a new `CallerContext.ownership_config: Arc<OwnershipRewriteConfig>` field**
  (`rust/analytics/src/lakehouse/read_scope.rs`), not a new `make_session_context` parameter — the
  same shape §6 below already settled for `ReadScope`/`is_admin`: per-request resolved values ride
  the context, per-service objects live on the service.
  Both knobs are **parsed in `micromegas-analytics`, not `micromegas-auth`**
  (`OwnershipRewriteConfig::from_env`), mirroring Stage 1's own "parse where consumed" reasoning for
  keeping `ReadScope` out of the `micromegas-auth` crate boundary.
- **Fail-loud fallback.** A view set matching none of the branches above (not `processes`, no
  `process_id` column, not `async_events`/`thread_spans`, not on the public allowlist) makes
  `analyze()` return `Err` naming the unhandled view set, rather than silently planning an
  unfiltered scan — the next added view set is caught at development/test time, not as a silent
  confidentiality gap.

**Prong B — construction-time guard inside each UDTF `call_with_args`** (for the span/metadata
functions Prong A can't reach). The owner id literal is available there via `exp_to_string` before
the provider is built (`process_spans_table_function.rs:110`, `perfetto_trace_table_function.rs:71`).
Thread `ReadScope` into `register_lakehouse_functions` (`query.rs:95-163`) and into each function
struct, then:
- **Arg-addressed functions** (`process_spans`, `perfetto_trace_chunks`, `parse_block`): the guard
  captures `(named_process_id, ReadScope)`. Since `call_with_args` is **synchronous** and the
  process→audience mapping needs metadata, perform the actual check at **scan time** (async) inside
  the execution plan: resolve the process's `audience` and fail closed if `∉ ReadScope`. Fails at
  plan time only if the check can be satisfied from already-resolved data. The **`get_payload` UDF**
  (`query.rs:165` — added since the original audit) is the same shape in scalar form: it reads a raw
  block payload by id, so it gets the identical async guard via the `block_id → process_id →
  audience` cache chain (§4 "Prong B performance").
- **Listing functions:** `list_partitions` has no owner arg but exposes a generic `view_instance_id`
  Utf8 column whose contents depend on the view set — per `view.rs:56`, "`view_instance_id` can be a
  process_id, a stream_id or 'global'" — leaking the existence/size/timing of other principals' data
  if left unfiltered. It **must be row-filtered**, per row kind:
  - **`process_id`-keyed rows** (`log_entries`, `measures`, `async_events`, `net_spans`, ... instance
    partitions): resolve `view_instance_id` as a `process_id` through the `process_id → audience`
    cache (§4 "Prong B performance"); keep the row iff its audience `∈ ReadScope`.
  - **`stream_id`-keyed rows** (`thread_spans` — the one view set with no `process_id`-scoped
    alternative, per `view_factory.rs`): resolve via a `stream_id → process_id` lookup (added to the
    cache design below), then the same `process_id → audience` cache; same keep-iff-readable rule.
  - **`'global'` rows** (the unscoped aggregate partitions — `processes`, `streams`, `blocks`, and the
    global `log_entries`/`measures` instances): carry no single audience to check. Per the fail-closed
    posture (§5), these rows are **hidden** from a `ReadScope::Principals` session — visible only
    under `ReadScope::All` (maintenance daemon), **or** when the row's view set is on the public
    allowlist (§5b), **or** when the configured `MICROMEGAS_UNSTAMPED_AUDIENCE` is in the caller's
    `ReadScope` (a global aggregate has no single audience, so it is treated like unstamped data —
    in an open deployment every caller reads `group:everyone`, so global rows stay visible, matching
    today's behavior; in a privacy deployment the knob is unset and they stay hidden).
    Otherwise `list_partitions` never shows a row it cannot resolve to a readable audience.

  `list_view_sets` **stays unfiltered (decided):** it returns view-set schema/definitions only, which
  contain no PII or per-principal data.
- **Mutating functions (decided, revised 2026-07-30): maintenance-only unless no admin principal
  can exist for the deployment.** The mutating set is now **five** entries: `retire_partitions`
  (`query.rs:120`) destructively deletes `lakehouse_partitions` rows for a
  `(view_set_name, view_instance_id)` pair (`write_partition.rs:116`), and `view_instance_id` is a
  `process_id` for process-scoped view sets — the same opaque, unchecked argument as `process_spans`,
  but destructive rather than read-only: naming another principal's id destroys their partitions (an
  integrity/availability hole, not a confidentiality one). `materialize_partitions`
  (`query.rs:132`) takes no per-process id — it materializes a *global* view
  (`view_factory.get_global_view`) over an insert-time range, so it can't target another principal's
  data, but it is an unbounded write/compute operation with no legitimate use from a read session.
  `regenerate_partitions` (`query.rs:139` — added since the original audit) and the scalar UDFs
  `retire_partition_by_file` / `retire_partition_by_metadata` (`query.rs:168,170` — the original
  audit covered UDTFs only) are likewise mutating/destructive. None is a read, so none gets an
  audience filter; instead `register_lakehouse_functions` registers the set only when the session
  is an internal maintenance context (`ReadScope::All`), **or** the caller's authenticated
  `AuthContext.is_admin` is set, **or** no admin principal can exist for the deployment's resolved
  auth provider — derived once at startup, not an operator-set knob (see the Stage 3 correction
  below). Otherwise a user calling any of them gets "function not found".
  The admin arm is tracked independently as issue #1377 (it closes a hole that exists today,
  before any isolation work: every authenticated caller can invoke these functions) and may land
  ahead of this stage; `is_admin` must be threaded from the authenticated `AuthContext`, never
  from client-claimed attribution. (Recorded caveat: this stays deployment-wide — none of the five
  functions filters by audience, so in a hybrid deployment mixing an everyone-group with personal
  audiences, an admin-less auth provider hands every authenticated caller destructive access to
  personal audiences too; tighten to per-audience checks if hybrid becomes real.)

**Implemented (Stage 3, #1371) — corrections to the sketch above, recorded here now that Prong B
has landed as `AudienceGuard`/`AudienceIndex` (`rust/analytics/src/lakehouse/audience_guard.rs`):**
- **One cache, not three.** The sketch above described a `process_id → audience` cache plus two
  chained resolutions (`block_id → process_id`, `stream_id → process_id`). The shipped
  `AudienceIndex` instead has a single `moka::future::Cache` keyed on `(IdKind, Uuid)` — `Process`,
  `Block`, and `ProcessOrStream` are three disjoint (plus one derived) slices of one keyspace, not
  three separate caches with separate eviction policies. Keying on the bare `Uuid` (as the sketch's
  chained caches implicitly would) is unsafe here: ids are client-supplied at ingestion with no
  cross-table uniqueness constraint, so the same `Uuid` can be a `process_id` in one audience and a
  `stream_id`/`block_id` in another, and a wrong-kind cache hit would authorize a guard against the
  wrong owner.
- **No `block_id → process_id → audience` chain for `get_payload`.** The sketch assumed `get_payload`
  needed the same cache chain as `parse_block`. It doesn't: `get_payload(process_id, stream_id,
  block_id)` already takes `process_id` as its own argument and builds
  `blobs/{process_id}/{stream_id}/{block_id}` directly, so checking that argument alone is both
  necessary and complete — a caller who names a readable process cannot reach another process's
  blob, since the foreign block simply isn't under that prefix. One `IdKind::Process` resolution,
  no `blocks` join at all.
- **Guard-then-internal-caller, not scope inheritance, for the three UDTFs' inner sessions.** The
  original audit (and the parent plan's own §5) assumed the three recursive-context call sites
  inside `process_spans`/`perfetto_trace_chunks`/`parse_block` must inherit the caller's resolved
  `ReadScope`. Stage 3 deviates deliberately: each inner session instead runs under a witness type
  (`Authorized`, constructible only by a successful `AudienceGuard::authorize` call)'s
  `internal_caller()`, which still resolves `ReadScope::All`. Every statement those inner sessions
  run is server-constructed and confined to the id the guard already authorized, so inheriting the
  caller's scope would add nothing to the confidentiality argument while introducing a second,
  independent daemon-materialization dependency (`OwnershipRewrite`'s `processes`/`streams`
  freshness requirement) on top of one two of the three functions already have. See
  `tasks/1371_udtf_udf_guards_plan.md` §6 for the full argument and its accepted trade-off (losing
  a second, independent filter as defense-in-depth inside those three functions specifically).
- **Postgres, not the materialized `processes`/`streams` snapshot, is Prong B's audience source.**
  `find_process` (cited above) reads via a connection pool straight against Postgres — fresher than
  Prong A's daemon-materialized copy, and free of Prong A's "the maintenance role must have caught
  up" precondition. The two prongs consequently read different copies of the same property in the
  general case (documented as an accepted trade-off in `tasks/1371_udtf_udf_guards_plan.md` §11,
  not fixed here).
- **No `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS` knob — the registration gate derives
  `admin_principal_possible` instead.** The sketch above specified an operator-set boolean opening
  the five mutating functions to non-admin sessions. It didn't ship, because the fact it asked the
  operator to declare is one the server already knows: `AuthProvider` gained a `can_grant_admin()`
  method (`rust/auth/src/types.rs`, default `false`); `OidcAuthProvider` returns `true` only when
  its admin-users list is non-empty, and `MultiAuthProvider` returns `true` if any provider in its
  chain does (`rust/auth/src/oidc.rs`, `rust/auth/src/multi.rs`) — both API-key providers keep the
  default, since an API key can never be admin. `FlightSqlServer` derives
  `admin_principal_possible` once at startup from the resolved auth provider (a `None` provider
  means auth is disabled, where every caller is already admin by the absent-header convention, so
  it derives `true`) and threads it onto every `CallerContext`; the gate in `query.rs` is
  `caller.is_admin || !caller.admin_principal_possible`. Deriving removes two configurations the
  knob would have let an operator express that should never exist: OIDC with admins *and* the knob
  on (destructive cross-audience access silently handed to every authenticated caller), and
  no-OIDC with the knob off (the five functions registered for nobody at all, which is not a
  security posture, just the gap #1382 opened, preserved). One capability is deliberately not
  carried over: a hardened deployment that wants the five functions permanently unreachable over
  the wire (relying only on the in-process maintenance daemon, which never goes through this gate)
  can no longer express that — a conscious call, with no extension point until something needs it.

In a privacy deployment (no implicit groups, no groups claim), `ps` is a singleton, so Prong A
reduces to `… IN ('user:alice@…')` — the exact per-user filter, same DataFusion plan — and Prong B
checks membership in a one-element set.

**Prong B performance.** The scan-time check is fast because **`process_id → audience` is immutable**
(stamped once at ingestion, never mutated). Add an in-memory `process_id → audience` cache — a
`moka::future::Cache` mirroring `metadata_cache.rs` (moka is already a workspace dep), backed on miss
by `find_process` (`rust/analytics/src/metadata.rs:241`, a primary-key point query). Because the mapping is immutable the
cache **needs no invalidation** — bound it by size (LRU) only. Warm hit = O(1) in-memory lookup;
cold miss = one indexed PG query, at most once per process ever. An entry is ~60 B, so caching far
more than the "thousands of users" population costs a few MB. The membership test itself is an O(1)
hash lookup against `ReadScope`, which is resolved once per query from the JWT `groups` claim plus
the configured implicit groups (no server-side lookup, independent of user count). `parse_block`
and `get_payload` add one more immutable `block_id → process_id` resolution, cached the same way. `list_partitions`' `thread_spans` rows need
one further immutable resolution, **`stream_id → process_id`** — a stream's owning process is fixed
at stream creation and never mutated — backed on miss by a primary-key point query against `streams`
(mirroring `find_process`); cache it the same size-bounded, invalidation-free way, then chain into the
existing `process_id → audience` cache to reach the audience for the membership test.

### 5. Bypass paths

- **Maintenance daemon** materializing global views must run with `ReadScope::All` (internal
  materialization path, never a user session). This is the **only** producer of `ReadScope::All`.
  **There is no single chokepoint** (drift vs. the original audit): the daemon never calls
  `make_session_context` itself — internal session contexts are built per-view at ~10 sites, each
  hardcoding `NoOpSessionConfigurator` (`view.rs:109`, `merge.rs:101`, `sql_batch_view.rs:87,154`,
  `export_log_view.rs:118,171`, `batch_partition_merger.rs:133`, `metadata.rs:182,287`). All of
  these are internal-only and get `ReadScope::All` at construction. Three further context-building
  sites are **reachable from user queries** (`parse_block_table_function.rs:81`,
  `process_spans_table_function.rs:254`, `perfetto_trace_execution_plan.rs:232` — UDTFs that
  recursively build a context to run their inner query): these must **inherit the caller's
  `ReadScope`**, never `All`, or they become bypasses.
- **No human-admin query-path bypass (decided).** `is_admin` does **not** map to `ReadScope::All`; an
  admin's FlightSQL session is filtered like any other. Rationale: an operator with lakehouse/object-
  store access can read the raw parquet directly, so a query-path bypass adds attack surface and audit
  burden for no confidentiality gain. Admins needing cross-principal reads use direct storage access,
  not the query path. (`is_admin` never feeds `ReadScope`; it does get threaded to the
  session for the mutating-function registration gate — §4 Prong B, issue #1377 — an
  integrity/availability control, not a read bypass.)

### 5b. Public (audience-agnostic) views — optional, opt-in

Some aggregate views carry no per-principal PII (e.g. a metrics rollup or a fleet-wide health
summary derived across all audiences). It is useful to expose such views to **every** authenticated
caller regardless of their `ReadScope`, without granting `ReadScope::All`. This is a deliberate,
per-view-set confidentiality relaxation — **off by default, fail-closed**: a view set is private
unless an operator explicitly lists it.

Mechanism (reuses the existing per-view-set branch point, no new enforcement seam):
- A configured allowlist of **public view-set names** is resolved once per request alongside
  `ReadScope` and threaded to both prongs.
- **Prong A** already branches per view set — `OwnershipRewrite` can read the view set via
  `MaterializedView::get_view_set_name()` (`materialized_view.rs:77`). For a view set on the public
  allowlist it injects **no** predicate (neither the `processes` audience filter nor the
  `process_id` semi-join); for every other set it filters exactly as before.
- **Prong B** — `list_partitions` shows the view set's `'global'` aggregate rows (otherwise hidden
  from a `ReadScope::Principals` session, §4) when that set is public. The arg-addressed UDTFs
  (`process_spans`, `perfetto_trace_chunks`, `parse_block`) are inherently **process-scoped**, not
  aggregate, so the public exemption never applies to them — they always audience-check.
- `ReadScope::All` (maintenance daemon) is unaffected; it already sees everything.

Constraints (operator responsibility — the allowlist is a confidentiality decision):
- **Only genuinely aggregated / non-PII view sets** may be listed. The unscoped **global
  `log_entries` / `measures`** instances carry raw per-principal bodies across all audiences —
  listing those would expose every principal's raw telemetry and **must not** be done. The
  allowlist is meant for derived rollups, not raw global views.
- **Public means "any authenticated caller," not unauthenticated.** The query path always
  authenticates via OIDC; truly anonymous access is out of scope.
- **Fail-closed:** the default allowlist is empty, so with no configuration the plan is
  byte-for-byte the design above (every view set private).

Config: `MICROMEGAS_PUBLIC_VIEW_SETS` (comma-separated view-set names, default empty), resolved by
the same factory as the isolation grant knobs (see Config surface). This can be deferred past v1
with no rework — an empty allowlist is the current behavior, and the branch point
(`get_view_set_name`) is already required by Prong A.

### 6. Threading identity into the session context

`make_session_context` currently takes no identity. Add the resolved `ReadScope` as a parameter
(no `is_admin` needed — there is no admin query bypass):

```
make_session_context(lakehouse, part_provider, query_range, view_factory, configurator, read_scope)
```

- `flight_sql_service_impl` already resolves `attr.user_email` per request
  (`flight_sql_service_impl.rs:318`); call `ReadPolicy::readable_principals` there and pass the
  result into both `make_session_context` call sites (`:372`, `:842`).
- **Two identity holes must be closed as part of this threading work** (found in the 2026-07-30
  audit; either would be a full enforcement bypass):
  - The **prepared-statement path** (`flight_sql_service_impl.rs:842`,
    `do_action_create_prepared_statement`) builds its session context with **no identity
    resolution at all**. It must resolve the caller and a `ReadScope` exactly like the
    `do_get` path.
  - `validate_and_resolve_user_attribution_grpc` (`rust/auth/src/user_attribution.rs:108`) **falls
    back to client-claimed identity** when the `x-auth-subject` header is absent (`:125-133`).
    That fallback is acceptable for audit attribution but must **never** feed `ReadScope`:
    the scope is resolved from the authenticated `AuthContext` only. Note the tower `AuthService`
    currently stringifies identity into gRPC metadata (`x-auth-subject`, `x-auth-email`,
    `x-allow-delegation`); the `groups` claim must cross that boundary too (new header or,
    better, the `AuthContext` request extension directly — it is in-process middleware).
- **The scope must reach Prong B too.** `make_session_context` calls `register_functions` →
  `register_lakehouse_functions` (`query.rs:95-163`), which is where UDTFs are registered. Thread the
  `ReadScope` down that path so each `TableFunctionImpl` is constructed with it. `call_with_args` is
  **synchronous**, so pass the already-resolved `ReadScope` value (not a policy object needing async
  I/O) — the arg-addressed functions defer the actual audience check to async scan time.
- The `ReadPolicy` object itself is a per-**service** dependency (like `session_configurator`),
  stored on `FlightSqlServiceImpl`; the **resolved scope** is per-request.
- Do **not** try to smuggle identity through `SessionConfigurator` — it is shared across requests.

### Config surface

**No mode enum, no default policy** (revised 2026-07-30 — replaces the former
`MICROMEGAS_ISOLATION_POLICY=self|audience` knob and its `self` default). One AbAC engine, configured
by grants; every knob is fail-closed when empty/unset:

**Table below is as originally designed (Stage 1); superseded by Stage 4 (#1372) — see the status
update near the top of this document.** `MICROMEGAS_IMPLICIT_GROUPS` no longer exists, replaced by
the `{prefix}_AUDIENCE_GRANTS` grant map; `MICROMEGAS_UNSTAMPED_AUDIENCE`'s value shape is now an
opaque audience name (`public`, not `group:everyone` — `:` is outside the charset and now fails
startup).

| Knob | Meaning | Open deployment | Privacy deployment |
|---|---|---|---|
| `MICROMEGAS_IMPLICIT_GROUPS` (removed, #1372) | comma-separated groups every authenticated principal belongs to (added to both readable and mintable sets) | `everyone` | unset |
| `MICROMEGAS_UNSTAMPED_AUDIENCE` | audience attributed at query time to data with no `micromegas.audience` property, and the visibility rule for `'global'` partition rows (§4) | `public` (was `group:everyone`) | unset (unstamped data hidden) |
| `MICROMEGAS_PUBLIC_VIEW_SETS` | §5b public view-set allowlist | — | optional |
| `{prefix}_AUDIENCE_GRANTS` (added, #1372) | JSON grant map, keyed by audience name — see `mkdocs/docs/admin/authentication.md#audiences-and-grants` | unset (`public` alone covers it) | e.g. `{"team-alpha": ["group:eng"]}` |

Registration of the five mutating UDTFs/UDFs (§4) for non-admin user sessions is **not** a knob:
Stage 3 (#1371) derives it from whether the deployment's resolved auth provider can ever produce
an admin principal (`AuthProvider::can_grant_admin`) — no `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS`
ever shipped.

`MICROMEGAS_UNSTAMPED_AUDIENCE` is the migration-pain killer: an open deployment can turn
enforcement on **before any stamping exists** — legacy `NULL`-audience data coalesces to
`public` (was `group:everyone`), which every caller implicitly reads (`public` is a built-in read
grant for every authenticated principal, so no companion knob is needed the way
`MICROMEGAS_IMPLICIT_GROUPS` used to be). No backfill, no retention wait, no
mode flip, nothing ever disappears.

**Activation story:** while the stages ship, absence of all isolation config = enforcement
inactive (exactly today's behavior, plus a startup warning once the machinery exists). At the GA
release the configuration becomes **required** — startup error if the operator has not chosen a
posture (deliberately no default; every operator makes a conscious choice). Release notes document
the two profiles above.

Wiring lives next to `default_provider::provider_with_prefix` (a `mint_policy()` / `read_policy()`
factory reading the env vars; `from_env` precedent: `static_tables_configurator.rs:44-54`). The
trait seam permits asymmetric policies later (e.g. group reads, self-only mint) with no code
change.

**Encoding (decided 2026-08-12; the `MICROMEGAS_IMPLICIT_GROUPS` half of this decision is moot as of
Stage 4/#1372, which deletes that knob outright).** `MICROMEGAS_IMPLICIT_GROUPS` and
`MICROMEGAS_PUBLIC_VIEW_SETS` are
**comma-separated** flat lists; group names and view-set names may not contain a comma (validate and
reject at parse time, naming the offending entry). This deliberately differs from `MICROMEGAS_ADMINS`,
which is a JSON array: that variable is cited throughout as the precedent for *config-sourced
authorization data*, not for an encoding. Say which it is in the doc comment, because an operator
copying the `MICROMEGAS_ADMINS` shape into `MICROMEGAS_IMPLICIT_GROUPS` would silently configure one
group literally named `["everyone"]`. `MICROMEGAS_PUBLIC_VIEW_SETS` keeps this same comma-separated
encoding unchanged by #1372. `{prefix}_AUDIENCE_GRANTS` (#1372's replacement for the *access*
knob `MICROMEGAS_IMPLICIT_GROUPS` used to be) is deliberately the **other** encoding this section
argues against for a flat list — a `MICROMEGAS_ADMINS`-style JSON value — because it is not a flat
list: it needs the structured, per-audience `{"read": [...], "mint": [...]}` shape §2's own grant-map
section documents, which no comma-separated encoding could express.

## Long-term model — groups, nested membership, and grants

Recorded 2026-08-12. **Not** Stage 1–7 work; this section exists so the stages do not foreclose it,
and so the four Stage 1 properties it depends on are deliberate rather than lucky.

Everything above resolves a readable set from **flat, IdP-asserted membership**: the `groups` claim
*is* the grant, because `AudienceReadPolicy` maps group `G` straight to audience `group:G`. The target
model separates the two relations that identity collapses:

```
membership:  user → group,  group → group          (transitive)
grant:       group → { audience, ... }             (many-to-many)
label:       audience stamped on data              (the stamping *mechanism* is unchanged --
                                                      ingestion still writes one micromegas.audience
                                                      property per process from bound_audience -- but
                                                      what an audience *value* IS changed under Stage 4,
                                                      #1372: an opaque [A-Za-z0-9_-]{1,255} label, not a
                                                      user:/group: encoding; see the status update above)

readable(caller) = ⋃ { read_grants(g) : g ∈ closure(caller) }  ∪  {user:<email>}   [OVERRIDDEN below]
```

**The `∪ {user:<email>}` term above is overridden as of Stage 4 (#1372) — see the status update near
the top of this document.** There is no identity-derived term in the shipped formula at all: a
caller's readable set is `⋃ { read_grants(g) : g ∈ closure(caller) } ∪ {public}` (public built in,
not a closure-derived grant) `∪ caller.read_audiences`, full stop. A personal audience needs an
explicit grant like any other; there is no free `{user:<email>}` union member standing in for one.

Users belong to groups, groups belong to groups, and a group is granted a set of audiences it may
read. Nothing about the **stamp** changes — ingestion still writes one `micromegas.audience` per
process from the key's `bound_audience` — and nothing about **enforcement** changes, since both prongs
still receive a resolved audience set. This generalizes only the *resolution* side, which is the
cheapest place in the design to grow.

### Continuity: today is the degenerate case

Today's rule is this model with **identity grants** — `read_grants(G) = {group:G}`, auto-seeded per
group. The migration into the full model is therefore "seed one identity grant per existing group",
and no caller's readable set changes on the day the store lands. Two consequences:

- §2's formula is not a competing design to be replaced; it is the special case. `ReadPolicy` is the
  seam that lets a `GroupGraphReadPolicy` land beside `AudienceReadPolicy` with **zero** change to
  Prongs A/B. Stage 4's env-map `AudienceGrants` is exactly this special case, one stage early: a 1:1
  stand-in for `group_read_grants`/`group_mint_grants` below, kept as one map only because no store
  exists yet to split it across two.
- **"Grants are the only authority; the `user:`/`group:` value prefixes are naming convention" —
  overridden, not merely superseded, as of Stage 4 (#1372).** This section originally argued that
  prefixes would *stay* once grants existed, purely as naming convention with no authority of their
  own. Stage 4 went further and removed the prefixes outright: `[A-Za-z0-9_-]` makes
  `user:alice@example.com` unrepresentable as an audience value. The collision concern this passage
  named (a group id colliding with a user email in one flat namespace) is answered instead by
  byte-exact identity in that flat namespace, and the default-grant concern by requiring an explicit
  grant entry — see the status update near the top of this document.

### Where authority lives

The IdP is the source of **leaf membership only**; micromegas owns **composition and grants**:

- IdP `groups` claim → the caller's directly asserted groups. Never editable locally.
- Local store → group-in-group edges, and group → audience grants.

The alternative — asking the IdP for transitive groups and keeping grants there too — means
negotiating with IdP administrators for every audience-sharing change, and most IdPs cannot express
"may read audience X" at all. This split keeps the IdP answering *who is this* and micromegas
answering *what may they read*, the same boundary `MICROMEGAS_ADMINS` already draws.

**This is the deferred local grants table, promoted to the target state**, so it carries the cost that
deferral was avoiding: **grant editors join the TCB** (Stage 1 step 5 and Trade-offs both record this).
Accept it explicitly — admin-gated writes plus an audit trail — rather than rediscovering it during
implementation.

Namespace caveat: IdP group names and local group names occupy one namespace. Either prefix local
groups or give the store explicit ids and map claims onto them. A silent collision is a grant the
operator did not intend.

### Data model sketch

```sql
groups(group_id UUID PRIMARY KEY, name TEXT UNIQUE, description TEXT, created_at, created_by)
group_members(group_id UUID, member_kind TEXT CHECK (member_kind IN ('user','group','service')),
              member_id TEXT, PRIMARY KEY (group_id, member_kind, member_id))
group_read_grants(group_id UUID, audience TEXT, PRIMARY KEY (group_id, audience))
group_mint_grants(group_id UUID, audience TEXT, PRIMARY KEY (group_id, audience))
```

Four details decide whether an implementation is correct rather than merely plausible:

- **Edge direction, stated once and tested.** `member_of(A, B)` means *A is a member of B*: the closure
  walks **upward** from the caller and grants flow **downward** to members. Reversed traversal is the
  canonical nested-group bug, so it earns a doc comment and a three-level-chain test.
- **Cycles.** `WITH RECURSIVE` with a depth cap, or in-memory BFS with a visited set. Reject cycle
  creation at write time **and** tolerate cycles at read time — a store kept acyclic only by the
  goodwill of its writers will eventually contain a cycle.
- **Caching makes grant latency a property, not an accident.** Cache the per-subject closure in a
  bounded `moka` with a short TTL, mirroring the key-store cache: membership and grant changes then
  take effect within the TTL, and that must be stated the way 0b states revocation latency. Unlike the
  immutable `process_id → audience` caches (§4), this one *is* invalidatable within a single process —
  do not design around invalidation for multi-node deployments.
- **Store outage fails closed.** A resolution failure is a denial, or a retryable 503 via the
  `ProviderUnavailable` precedent (`db_api_key.rs:22-24`) — never an empty-or-permissive scope. This is
  why `ReadPolicy::resolve` is `async` and fallible from Stage 1 onward, and why Stage 1's resolver
  call site must already deny on `Err` (Stage 1 step 3).

### Service accounts in the target model

Stage 4b gives an analytics key a per-key `read_audiences` grant — a **principal-level direct grant**,
which this model keeps as a first-class case alongside group grants (that is why §2's union takes
`caller.read_audiences` rather than branching on credential kind). The tidier end state is
`member_kind = 'service'`: the key's principal becomes a group member and inherits grants like anyone
else, and the column becomes either a direct-grant fast path or dead weight to drop. Either is
coherent — Stage 4b picks the column because it needs no group store. What must **not** happen is a
third grant mechanism appearing later; whichever survives, there are exactly two (principal-level and
group-level).

### Read and write finally separate

"A group may **read** a set of audiences" breaks the collapse recorded in Stage 1 step 5, where
membership in `G` grants both `read:G` and `write:G`. Hence **two grant tables of the same shape**
(`group_read_grants`, `group_mint_grants`) rather than one relation consulted by both policies:
re-collapsing them later would be a security regression relative to the read-only phrasing, and the
split costs one table. This is also where **`role` earns its name** (see [Naming](#naming)) — `viewer`
/ `minter` per group is a genuine permission bundle, and the term stops being reserved. `MintPolicy`
and `ReadPolicy` staying independent traits is what makes the split a pure addition.

### Enforcement scaling

`readable(caller)` is no longer bounded by "groups the caller is in": one group may be granted hundreds
of audiences, and Prong A injects `audience IN (<literals>)`. That is plan bloat, not a correctness
problem, and it has the answer already used for process ids — resolve grants into a subquery/semi-join
instead of a literal list once sets get large. It is cheapest once the audience is a first-class column
(step 15), so it belongs there rather than as a pre-optimization.

### Admin surface

Groups / membership / grants CRUD on `analytics-web-srv` beside the key-admin pages, admin-gated, with
`created_by` / `revoked_by` audit columns in the house style — grant edits are confidentiality changes
and must be attributable.

One rule to design in from the start: **two-sided authorization.** Adding a member requires authority
over the *group*; granting an audience requires authority over the *audience*. With only the first
check, anyone who can edit a group's membership can add themselves to a group that reads everything.
Today's blanket `is_admin` gate satisfies both trivially — the rule matters the moment group ownership
is delegated, which is the natural next request once groups exist.

### What Stage 1 must keep for this to stay reachable

Four properties, all folded into Stage 1's steps below:

1. `ReadPolicy` / `MintPolicy` are **`async` and fallible** — a closure lookup over a store cannot live
   behind a sync infallible signature, and retrofitting it means migrating every call site.
2. The resolver call site **denies on `Err`**. With a claim-only policy that cannot fail, a permissive
   fallback is easy to write and invisible; once the policy does I/O, that line is a bypass.
3. `AuthContext.groups` is documented as **IdP-asserted leaf membership — an input to resolution,
   possibly incomplete**, never "the caller's groups".
4. **No group vocabulary crosses into `micromegas-analytics`.** `ReadScope` carries resolved audiences
   only, so the entire group model stays behind the policy seam.

## Implementation Steps — staged rollout (revised 2026-07-30)

Re-ordered from the original Phases 1–5 around one constraint: **every stage ships with zero
behavior change** until an operator sets config, so existing team-wide deployments upgrade
untouched. Two tracks run in parallel — **enforcement** (Stages 1–3) and **keys/stamping**
(Stages 0, 4–6) — converging at activation (Stage 7). Each stage maps to one GitHub issue.

**Stage 0 depends on nothing** and is the only stage that delivers value on its own; everything
else is inert until an operator configures it. Stages 2, 3 and 4 depend on Stage 1; Stages 5 and 6
depend on Stage 0. Stage 0 now carries the key-management routes as well as the store, so Stage 6 is
reduced to audience resolution on an existing route plus the setup script.

### Stage 0 — DB-backed API key store + key-management API (no audience yet; independent of the AbAC seam)

**Landed** as #1383 — see `tasks/completed/1383_db_api_key_store_plan.md` for what actually shipped
(this section is the design rationale, kept for the stages that hang off it). The import tool this
section assumes was split out to #1411 and **has since landed**, along with a web admin UI; #1458
then moved the whole key-management surface off ingestion onto `analytics-web-srv`
(`tasks/completed/simplify_ingestion_api_key_admin_plan.md`). 0c and 0d below describe the shipped
end state, not the original design.

Split out of the original Stage 4 (revised 2026-07-30). Moving the *key store* ahead of the policy
seam, and leaving only the *audience column* behind it, for four reasons:

- **Standalone value.** Stages 1–3 ship zero behavior change by design. This one fixes a live
  operational problem: keys are plaintext JSON in `MICROMEGAS_API_KEYS`, so grant/revoke means an
  env edit plus a redeploy of every service that validates keys — three construction sites
  (`telemetry-ingestion-srv/src/main.rs:51`, `flight_sql_server.rs:226`,
  `monolith/src/main.rs:193,207`). No revocation, no rotation, no per-key audit, no last-used.
- **The env keyring cannot survive Stage 6.** `ApiKeyAuthProvider::validate_request` deliberately
  scans **every** key with `ct_eq` on **every request** with no early exit
  (`api_key.rs:100-129`) — correct for a handful of team keys, but the mint route lands here and
  Stage 6 turns it into one key per user/machine, taking N from ~5 to thousands and charging
  ingestion N constant-time compares per request. A hash-indexed lookup is O(1). The store is
  therefore a *precondition* for minting at all, not a convenience.
- **It is the long pole of the keys track** (Stages 5 and 6 both hang off it) and the migration
  vehicle for open deployments — the earlier it lands, the longer it soaks before anything depends
  on it.
- **It does not shorten the path to enforcement.** This is parallel work, deliberately.

#### Two tables, split by risk class (decided 2026-07-30)

Write credentials and read credentials live in **separate tables** — `ingestion_api_keys` and
`analytics_api_keys` — and **one key is never valid on both surfaces**.

- **The security model is asymmetric, so the boundary should be too.** Per "Load-bearing property
  preserved", a stolen write key is an *integrity* problem: it pollutes an audience's view and
  grants zero read power. A read credential is a *confidentiality* problem. Stage 6 puts one
  ingestion key per user/machine on dev boxes and game clients, embedded in
  `OTEL_EXPORTER_OTLP_HEADERS` — the most widely distributed and most exposed credentials in the
  deployment, each needing exactly one surface. A shared table makes every one of them a read
  credential too, which is the wrong blast radius. API keys also carry `allow_delegation: true`
  (`api_key.rs:126`), so a leaked key can attribute traffic to arbitrary users
  (`user_attribution.rs:154-174`).
- **The columns genuinely differ** — and the 2026-08-12 service-account decision makes this argument
  *stronger*, not weaker. Ingestion keys carry `audience TEXT` (Stage 4): exactly one **write** label,
  immutable, `NOT NULL`. Analytics keys carry `read_audiences TEXT[]` (Stage 4b): a **read** grant,
  set-valued, defaulting to empty. One table would need both columns, each meaningful for exactly one
  discriminant value — a discriminated union in nullable columns, where the discriminant carries no
  information the table name doesn't.
- **The split is enforced in code today, with Postgres grants as a documented option, not a
  guarantee already in place.** #1383's implementation settled this: every service in a deployment
  as shipped shares one DB role via `MICROMEGAS_SQL_CONNECTION_STRING`, and the schema migration runs
  as the owner, so "Postgres grants enforce it" is not true out of the box. What ships is a
  *code*-level boundary instead — `api_keys_router` hardcodes `ingestion_api_keys`, and the analytics
  provider is constructed bound to `analytics_api_keys`, with no parameter either could point at the
  other table — plus a documented grant recipe (`mkdocs/docs/admin/api-keys.md`) for operators who do
  separate DB roles per service: the ingestion role would hold `INSERT` on `ingestion_api_keys` and
  **never** on `analytics_api_keys`, with each validating service granted only `SELECT` plus
  column-level `UPDATE (last_used_at)` on the one table it reads. Fail-closed-by-schema is the
  aspiration for operators who separate roles; fail-closed-by-code is what every deployment gets
  today.
- **Shared code, not a shared relation.** The hash lookup, the `moka` cache and the `last_used_at`
  write are identical, so one `DbApiKeyAuthProvider` parameterized by table serves both. That is
  where the single-implementation value lives.
- **Rejected: one table with a `scopes` column.** Once "never both" is a rule the column is always a
  singleton, and it downgrades a schema-level boundary to an application-level check on the mint
  path. Note the env layout can *already* express disjoint sets per surface
  (`MICROMEGAS_INGESTION_API_KEYS` vs `MICROMEGAS_ANALYTICS_API_KEYS`, `default_provider.rs:53-59`),
  so the split preserves an existing capability rather than inventing one — but see 0d for the one
  property it deliberately breaks.
- **Superseded 2026-08-12: `analytics_api_keys` is *not* transitional — it is the service-account
  table.** The original text reasoned that read grants derive from the IdP `groups` claim, an API key
  carries no claim, so a key-authenticated query would resolve implicit-groups-only (empty in a privacy
  deployment) and analytics keys would be a migration artifact. Two facts make that the wrong
  conclusion. **(1)** Key-only flight-sql is a documented, supported deployment — "a non-empty
  `analytics_api_keys` table counts as auth configured on its own, so this key-only deployment (no
  OIDC) is fully supported" (`mkdocs/docs/grafana/authentication.md`) — so attrition inside Stage 2 was
  never available. **(2)** The problem was never key-specific: Grafana's *other* auth mode is OAuth 2.0
  **client credentials** (`grafana/pkg/flightsql/oauth.go`), whose token takes the OIDC path with no
  `email` claim (`oidc.rs` `get_email` chain) and therefore resolves to the empty set too. A
  key-specific branch would have fixed half the problem.
  **Decided: analytics keys are service-account credentials with a configurable set of readable
  audiences** (`read_audiences`, Stage 4b). Separate tables remain the right shape — the two columns
  are opposites (one immutable write label vs. a set-valued read grant), and the read table's schema
  can evolve without touching the ingestion hot path.
  **Not chosen:** deriving scope from delegation headers. Keys carry `allow_delegation: true`
  (`api_key.rs:126`, `db_api_key.rs:326`) and Grafana already sends `x-user-id`/`x-user-email`
  (`grafana/pkg/flightsql/query_data.go`), so this is the tempting option — and it is hole #2 verbatim
  (§6): client-claimed identity must never widen (or narrow) a `ReadScope`. A service account's scope is
  its own grant; the delegation headers stay attribution-only.
- **`object-cache-srv` stays on env vars (decided 2026-07-30).** It is a fourth key-validating
  surface (`cli.rs:59`, same `parse_key_ring`) but it has **no database access at all** — no
  connection string in its CLI, so it cannot reach the key tables — and giving a cache service a
  Postgres pool purely to read a key table is not worth it. It keeps the env keyring and is out of
  the key-store scope entirely. Two consequences to record rather than rediscover:
  - **The env keyring is permanent, not transitional.** `ApiKeyAuthProvider` and `parse_key_ring`
    (`api_key.rs`) must not be deleted once the DB store lands — 0b's "compose during transition"
    applies to ingestion and analytics only. `object-cache-srv` remains a legitimate consumer
    indefinitely.
  - **Its keys are not revocable without a redeploy.** Accepted: they are service-held (flight-sql
    and the daemon hold them, per `CacheClientStore`), few, and never distributed to users or
    machines, so the operational pressure that motivates this stage does not apply. If it ever does,
    the fix is a mechanism for that service specifically, not a reshaping of these tables.

#### Key management is an HTTP API, not SQL (decided 2026-07-30; host service revised 2026-08-12)

Create/revoke/list are OIDC-authenticated HTTP routes (0c), pulled forward from
Stage 6 — **the table alone does not deliver Stage 0's claimed value**, since without an endpoint an
operator still cannot revoke anything without hand-written SQL against Postgres. Stage 6 then extends
the create route with audience resolution rather than introducing it.

**Which service hosts them changed after this plan was written.** Stage 0 shipped them on ingestion
as designed; #1411/#1458 then moved key management to `analytics-web-srv` and deleted ingestion's
`/auth/api_keys*` routes outright, on the grounds that ingestion should only do ingestion and that a
server-side proxy from the web service to ingestion (with a shared privileged service credential)
was worse than a direct write. The move **strengthens** the asymmetry argument below rather than
weakening it: the most-exposed, fleet-facing process no longer holds `INSERT` on any key table at
all, and every mint/revoke/import now records the acting admin's own OIDC identity in
`created_by`/`revoked_by` instead of a shared service credential. What it costs is 0c's claim that
"analytics keys are not mintable through this API" — see 0c.

**Rejected: admin-gated lakehouse UDFs** (`revoke_api_key(...)`, `import_api_key(...)`) on the
flight-sql path, despite the good precedent for admin-gated mutating functions (#1382,
`query.rs:150-165`, and API keys can never be admin — `api_key.rs:124`). Two reasons:

1. **Query text is logged and micromegas ingests its own logs.**
   `flight_sql_service_impl.rs:330` emits `sql={sql:?}` at info, and `:841` logs prepared-statement
   text, so any key material passed as a SQL literal lands in `log_entries` — readable by anyone with
   query access, and strictly worse than the env var this stage is replacing. Hashing client-side and
   passing only the digest would neutralize this, but then:
2. **A write UDF hands the *read* service write access to the key tables**, trading away the
   schema-level boundary above for operator convenience. And since the mint API is needed for Stage 6
   regardless, the UDF path buys nothing that the HTTP route does not already provide.

Steps:

0a. **Two tables** in the telemetry DB (migration precedent: `sql_telemetry_db.rs:5-12`), same shape:

```sql
CREATE TABLE ingestion_api_keys (
  key_id       UUID PRIMARY KEY,
  key_hash     BYTEA NOT NULL,          -- sha256 of the full key string
  name         VARCHAR(255) NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL,
  created_by   VARCHAR(255) NOT NULL,
  last_used_at TIMESTAMPTZ,
  revoked_at   TIMESTAMPTZ,
  revoked_by   VARCHAR(255)
);
CREATE UNIQUE INDEX ingestion_api_keys_key_hash ON ingestion_api_keys(key_hash);
-- analytics_api_keys: identical. It never gains Stage 4's single-valued *write* `audience`;
-- it gains a set-valued *read* grant instead (`read_audiences TEXT[]`, Stage 4b).
```

**`key_id` is a design change #1383 settled that this outline left implicit**: this schema originally
put `key_hash` alone as the primary key, but the revoke route needs a non-secret handle to
key on, and `GET` must never hand out `key_hash` (there is no reason to distribute the lookup value
even though it is not reversible) — a UUID PK plus a unique index on `key_hash` gives both without
making the secret-derived value the row identity. `name` also carries no uniqueness constraint,
deliberately: rotating a key under a stable name is an expected state while an old key is phased out,
so every revoke path keys on `key_id`, never `name`.

Do **not** store the key in cleartext — a plaintext column is strictly worse than the env var
(backups, replicas, read access, query logs). A **unique** index on `key_hash` gives the O(1) lookup
(not merely an index, so a hand-written legacy-key import can use
`INSERT ... ON CONFLICT (key_hash) DO NOTHING` and be safely re-run). The import requirement
(existing key strings keep working, 0d) rules out imposing a `key_id.secret` shape, so lookup is by
hash of the whole string. SHA-256 without a KDF is safe **only** because these are high-entropy random
keys, not passwords — Argon2 would be both unindexable and too slow per request. Pair this with
rotating any legacy key that is not actually random.

0b. `DbApiKeyAuthProvider::new(pool, table)` composed via the existing `MultiAuthProvider`; env
    keyring and DB keyring compose during transition, so nothing breaks mid-migration. Requires
    threading a connection pool into `default_provider::provider_with_prefix`, which is a pure env
    factory today (`default_provider.rs:51`) — the first real API change in the auth crate; three
    call sites, and the two that pass an empty prefix (`telemetry-ingestion-srv/src/main.rs:51`,
    `flight_sql_server.rs:226`) must now name their table explicitly, since `""` cannot be inferred.
    Cache lookups in a bounded `moka` with a short TTL (same pattern as the §4 caches): a per-request
    DB hit on the ingestion hot path is not acceptable. **State the consequence as a property, not
    an accident: revocation takes effect within the cache TTL** — the endpoint below writes
    `revoked_at` and cannot invalidate remote caches.

0c. **Key-management API**, OIDC-authenticated and admin-gated. **Shipped end state
    (2026-08-12), after #1411/#1458 relocated it** — this is what Stages 4 and 6 must target:
    routes live on **`analytics-web-srv`**, gated by the `AdminUser` extractor over the browser
    cookie session (`ValidatedUser { is_admin }`), writing the telemetry DB directly:
    - `POST {base_path}/api/ingestion-api-keys` — generate a fresh random key, store its hash,
      return the cleartext **once**. Writes `ingestion_api_keys`; no audience until Stage 4.
      (`ingestion_keys.rs::mint_key` — the Stage 6 `resolve_audience` call site.)
    - `DELETE {base_path}/api/ingestion-api-keys/{key_id}` — set `revoked_at`. This is the operation
      that carries the stage (the 2am revoke with no redeploy).
    - `GET {base_path}/api/ingestion-api-keys` — name, created_at, last_used_at, revoked_at.
      **Never the hash**; there is no reason to hand out the lookup value even though it is not
      reversible.
    - `POST {base_path}/api/ingestion-api-keys/import` — 0d's legacy-key import, landed with #1411.
    - `/api/analytics-api-keys[...]` — the same four operations against `analytics_api_keys`
      (`analytics_keys.rs`, the module `ingestion_keys.rs` was modeled on).
    - Under `--disable-auth` the real routers are **not merged at all**; a static router answers both
      prefixes with 503, because that mode layers a hardcoded `is_admin: true` user on every request.

    **Superseded: "analytics keys are not mintable through this API."** The original text kept read
    credentials out of every HTTP write path because issuing them *from the fleet-facing ingestion
    service* is the wrong direction for the asymmetry. Once key management left ingestion, that
    reason stopped applying to the surface that now hosts it: `analytics-web-srv` is the read-side,
    admin-facing service, and minting a read credential there is no longer a cross-direction move.
    Analytics keys are mintable through `/api/analytics-api-keys` today. The property that actually
    mattered — **ingestion never issues keys, and `analytics_api_keys` never gains Stage 4's write
    `audience` column** — is preserved and is what Stage 4 relies on. (It does gain a *read* grant in
    Stage 4b, which is a different column with the opposite direction; minting one is consequently a
    confidentiality grant, see 9e.)

    The route no longer expands the surface of the most exposed process at all: ingestion holds no
    key-management route and no `INSERT` grant. API keys still cannot be admin (`api_key.rs:124`),
    so no key can mint another; the gate is a cookie session, not a bearer key.

0d. **Import — landed with #1411** as `POST {base_path}/api/ingestion-api-keys/import` plus a python
    CLI (`python/micromegas/micromegas/cli/import_keys.py`), superseding the python/`psql` tool
    originally planned here. It is the one thing the mint route cannot do, since minting generates
    fresh key strings. Destination table is explicit per key with no default — the prefixed vars map
    cleanly: `MICROMEGAS_INGESTION_API_KEYS` → ingestion, `MICROMEGAS_ANALYTICS_API_KEYS` →
    analytics. (The hand-written `INSERT ... ON CONFLICT (key_hash) DO NOTHING` path from #1383 §4's
    runbook still works and stays the fallback for operators without web-app access.)

    **The unprefixed fallback is the one behavior change in this stage**, independent of which tool
    (or hand) does the importing. In every split deployment
    both `telemetry-ingestion-srv/src/main.rs:51` and `flight_sql_server.rs:226` read the unprefixed
    `MICROMEGAS_API_KEYS`, so *every existing key is currently valid on both surfaces*. "Never both"
    cannot preserve that: a genuinely dual-use key must become two keys, and any client that used one
    key for both ingestion and queries must be updated. This is the single place the
    zero-client-change claim does not hold, and the Stage 7 migration guide must say so explicitly
    rather than leaving operators to discover it.

### Stage 1 — Policy seam + identity threading (no enforcement yet)

Detailed implementation plan: `tasks/1369_policy_seam_plan.md` (issue #1369). Where the two disagree,
that plan wins on placement/mechanism detail (notably: `ReadScope` lives in `micromegas-analytics`, not
`micromegas-auth`, and identity crosses the tower boundary via the existing `AuthContext` request
extension rather than a new header).

1. **Policy traits + AbAC impls.** Add `MintPolicy`, `ReadPolicy`, `ReadScope` in `rust/auth/src/`
   (e.g. `policy.rs`); add `AudienceMintPolicy` / `AudienceReadPolicy` (§1–2). No `Self*` impls — per-user
   is the AbAC engine with empty grants. **Both traits are `#[async_trait]` and fallible** — required by
   the long-term store-backed grant source, and free today since both call sites are already async.
   `AudienceMintPolicy` needs an explicit **admin arm**: `caller.is_admin` ⇒ any well-formed
   (`user:`/`group:`-prefixed) audience; otherwise the mintable-set formula of §1. Without it the only
   shipped impl cannot express the mint flow that exists today — the route is admin-gated
   (`ingestion_keys.rs::mint_key`), so `requested = user:bob@…` is unrepresentable and every minted key
   would be stamped with the minting admin's own audience. The arm grants no power the route's gate does
   not already grant, and it is deliberately asymmetric to the read path (§5: `is_admin` is never a read
   bypass) — mint is integrity, reads are confidentiality. Say so in the doc comment.
2. **AuthContext fields.** Add `bound_audience: Option<String>`, `read_audiences: Vec<String>` and
   `groups: Vec<String>` to `AuthContext` (`rust/auth/src/types.rs`); populate `None`/`[]` everywhere
   except the key paths (Stage 4 for `bound_audience`, Stage 4b for `read_audiences`) and OIDC.
   Document `groups` as **IdP-asserted leaf membership — an input to resolution, possibly incomplete**,
   never "the caller's groups" (long-term model, property 3).
   **Groups claim (low effort, confirmed):** add `groups: Option<Vec<String>>`
   to the `Claims` struct (`oidc.rs:193-227`) — no `#[serde(deny_unknown_fields)]`, so it is
   backward-compatible and absent-claim-safe; populate at the OIDC construction site
   (`oidc.rs:536-545`). Flat top-level array covers Auth0/Azure AD/Google (the confirmed targets);
   Keycloak's nested `realm_access.roles` is not a current target and would need a nested helper.
3. **Thread identity.** Add `read_scope` param to `make_session_context` (`query.rs:194`) and feed
   `register_lakehouse_functions` (`query.rs:96`). Resolve scope via `ReadPolicy` in
   `flight_sql_service_impl` and pass through both call sites (`:661`, `:1149` at 2026-08-12 HEAD;
   were `:372`/`:842`). **Close the two identity holes** (§6): resolve identity on the
   prepared-statement path, and never derive `ReadScope` from client-claimed attribution; carry
   `groups` across the `AuthService` boundary.
   **The resolver denies on `Err`** (long-term model, property 2): a `ReadPolicy` failure becomes
   `Status::unavailable`/`permission_denied`, never a default or empty scope. Write that branch now,
   while the call site is being authored, and test it with a stub policy that returns `Err` — a
   claim-only policy cannot fail, so nothing else would catch a permissive fallback until the day the
   policy starts doing I/O.

   **A third boundary drops `groups`, found 2026-08-12.** `analytics-web-srv` converts `AuthContext`
   into its own session type via `impl From<&AuthContext> for ValidatedUser`
   (`rust/analytics-web-srv/src/auth/claims.rs:40-48`), which keeps only
   `subject`/`email`/`issuer`/`is_admin`. Every new `AuthContext` field is silently dropped there.
   That does not matter for read enforcement (flight-sql resolves its own scope), but it is the
   **mint** path's identity source now that `MintPolicy` runs in `ingestion_keys.rs` — a policy that
   consults `groups` against a `ValidatedUser` would see an empty set and refuse every group mint.
   **Decided 2026-08-12: `MintPolicy::resolve_audience` takes `&AuthContext`, and the web service
   inserts the `AuthContext` into request extensions** beside the `AuthToken` and `ValidatedUser` it
   already inserts (`auth/handlers.rs`, one line). `ValidatedUser` stays as-is — it is the browser-session
   view and needs no groups. This mirrors the gRPC side, where `AuthService` already inserts the whole
   `AuthContext` (`tower.rs:141`), and it means every future `AuthContext` field reaches the mint path for
   free. Stage 6 reaches for `Extension<AuthContext>`; nothing to discover late.
4. **Config factory.** Parse the grant knobs (Config surface) next to
   `default_provider::provider_with_prefix`; `from_env` precedent
   `static_tables_configurator.rs:44-54`. Unset ⇒ enforcement inactive (transitional).
5. **Policy source (decided at Stage 1; superseded by Stage 4, #1372 — see the status update near
   the top of this document): IdP `groups` claim + `MICROMEGAS_IMPLICIT_GROUPS` only.** No local
   grants table in v1 — confidentiality rests solely on OIDC plus operator config; no TCB
   additions. Precedent: the `MICROMEGAS_ADMINS` allowlist (`oidc.rs:264-394`).
   **Consequence — write/read collapse to membership, closed by Stage 4, not deferred to the
   long-term store:** this step's original premise was that membership in `G` grants *both*
   `read:G` and `write:G`, with separate read/mint grants deferred to the eventual
   `group_read_grants`/`group_mint_grants` store. Stage 4 (#1372) lands that split now, one stage
   early, via the env-map `AudienceGrants`'s separate `"read"`/`"mint"` lists per audience — a
   bare-array (read-only) grant confers no mint authority, tested explicitly. This is *not* the
   Postgres grants table the paragraph below anticipated (no new TCB member — the map is still a
   flat env var, not an admin-editable store), but it is the read/write split arriving via the
   *env-map* mechanism rather than the store. The env map is a 1:1 stand-in for the two tables below,
   kept in one map only because no store exists yet to split it across — see
   [Long-term model](#long-term-model--groups-nested-membership-and-grants), including nested
   groups, two grant tables, and the TCB consequence a *store* (not the env map) still defers.

### Stage 2 — Enforcement Prong A (inactive until configured)
6. Add `OwnershipRewrite` in `rust/analytics/src/lakehouse/ownership_rewrite.rs`, constructed from
   `ReadScope` + the unstamped-audience + public-view-set config. Inject the audience predicate
   (with `coalesce` semantics, §4) on the `processes` view, the semi-join on `process_id`-keyed
   views, and `view_instance` (caught as a `TableScan<MaterializedView>`). Branch per view set via
   `MaterializedView::get_view_set_name()` (§5b). **Register unconditionally** — unlike
   `TableScanRewrite`, which is added only when `query_range.is_some()` (`query.rs:206`).
7. **Test with audience stamped manually** (before ingestion stamping exists): seed processes with
   a `micromegas.audience` property; assert cross-audience queries return nothing, same-audience
   returns its own rows, unstamped rows follow the `MICROMEGAS_UNSTAMPED_AUDIENCE` rule, and the
   daemon (`ReadScope::All`) returns everything.

### Stage 3 — Enforcement Prong B (inactive until configured)
8. Thread `ReadScope` into each affected function. Arg-addressed (`process_spans`,
   `perfetto_trace_chunks`, `parse_block`, **`get_payload`**) verify the named process's audience
   at async scan time, failing closed; `list_partitions` row-filters by readable audience incl.
   the `'global'`-row rule (§4); the five mutating functions are registered only for maintenance
   contexts, admin sessions, or when no admin principal can exist for the deployment (the admin arm
   is issue #1377 and may land ahead of this stage). Build the `moka` caches
   (`process_id → audience`, `stream_id → process_id`, `block_id → process_id`). Internal
   maintenance contexts get `ReadScope::All`; the three user-reachable recursive context sites
   inherit the caller's scope (§5).

### Stage 4 — Audience on keys (the open-deployment migration vehicle) — **landed, #1372**

Reduced to the part that genuinely needs the AbAC seam (revised 2026-07-30); the key store itself
is Stage 0. Depends on Stage 0 (the tables) and Stage 1 (the audience shape). **Landed as #1372**,
which also amended Stage 1's audience model itself (opaque labels, grant map — see the status
update near the top of this document); the two bullets below are corrected to what actually shipped
rather than left as the pre-implementation plan.

9. Added `audience VARCHAR(255) NOT NULL` to `ingestion_api_keys` (migration v6) — **a column, not a
   mapping table.** The
   binding is 1:1 and immutable (§3: `resolve_audience` runs once at mint and the result is recorded
   on the key; `bound_audience` is single-valued), so `NOT NULL` is fail-closed by construction,
   whereas a 1:1 side table would add a join to the hot auth path and admit a key-with-no-audience
   state that ingestion must remember to reject. A separate table only becomes the right shape if
   audiences gain their own metadata/grants or a key can bind more than one. `analytics_api_keys`
   gets **no** `audience` column: nothing stamps data on the read side. It gets a set-valued *read
   grant* instead — Stage 4b, a different column in the opposite direction.
   The ingestion `DbApiKeyAuthProvider` now produces `AuthContext { bound_audience: Some(audience),
   email: None, allow_delegation: false, is_admin: false }` — **`email` stays `None`, not
   `Some(...)` as originally sketched**: under the opaque-label model, `email` is what `user:`
   selectors match on, and populating it from `created_by` would hand an ingestion key every
   audience granted to the minting admin (a deliberate, settled deviation from this bullet's
   original sketch, not an oversight).
10. Legacy-key imports assign an audience: existing keys land as **`public`** (the built-in read
    grant, not `group:everyone` — that value is no longer representable under the opaque-label
    charset) — still zero client changes; this is how open deployments migrate. Keys imported before
    this stage get `public` on the v6 backfill (an accurate description of their current,
    unstamped-and-visible-to-everyone state, not a new grant). The import path is a real route —
    `POST {base_path}/api/ingestion-api-keys/import` in `ingestion_keys.rs` — this step edits
    that handler (and the `import_keys.py` CLI's request shape, plus a per-entry keyring
    `"audience"` field), not a runbook. The `NOT NULL`
    column added in step 9 means **every** insert site must supply an audience: `mint_key`,
    `import_key`, and any hand-written `INSERT` in the 0d fallback. Adding the column without
    updating all three breaks key creation outright — that is the fail-closed behavior working,
    but it should be a planned edit, not a surprise. **`mint` requires an explicit audience or a
    configured `MICROMEGAS_DEFAULT_KEY_AUDIENCE`** (400 otherwise — never a silent `public`, since
    that would publish a new credential's entire future ingestion history); `import` alone falls
    back to `public`, matching the backfill's continuity assumption.

### Stage 4b — Read grants on analytics keys (service accounts)

New 2026-08-12, the read-side mirror of Stage 4. Analytics API keys are **service-account credentials**
and their readable audiences are configurable per key. Depends on Stage 0 (the table) and Stage 1
(`AuthContext.read_audiences` plus §2's union). It does **not** block Stage 2: pre-existing rows default
to no grant, which is fail-closed, so enforcement can land first and grants can be filled in before
Stage 7 activation. Needs its own epic issue.

9b. **The column.** `read_audiences TEXT[] NOT NULL DEFAULT '{}'` on `analytics_api_keys` (array
    precedent: `processes.tags`, `lakehouse_partitions.sort_order`; read back via
    `sql_arrow_bridge.rs:350`). The analytics `DbApiKeyAuthProvider` selects it into its cached `KeyRow`
    (`db_api_key.rs:160-164`) and produces `AuthContext { read_audiences: <grant>, email: None,
    is_admin: false, allow_delegation: true }`. `DEFAULT '{}'` means an omitted grant is a key that reads
    **nothing**, never one that reads everything.

9c. **Immutable at mint; rotate to change (decided).** This mirrors the ingestion `audience` model and
    avoids a mutable-grant cache story: `KeyRow` is cached in a `moka` with a TTL, so an editable grant
    would take effect only within that TTL — the same latency property 0b documents for revocation, but
    far more surprising when the change *widens* access. If a PATCH route is ever added, state the TTL
    bound as loudly as the revoke docs do.

9d. **Route/UI/client plumbing.** `POST {base_path}/api/analytics-api-keys` accepts `read_audiences`;
    `/import` accepts it too; `GET` returns it (a grant is not secret — unlike `key_hash`, listing it is
    the point); the Admin → Analytics API Keys page gains the input; `import_analytics_api_key` in
    `python/micromegas/micromegas/web_client.py` and `cli/import_keys.py` pass it through. Audiences are
    shape-checked (`user:`/`group:` prefix) at the route.

9e. **Grant vetting — minting an analytics key *is* a read grant.** So this route's authorization is a
    confidentiality control, not just an admin convenience. Admin-only (today's `AdminUser` gate) is
    sufficient and needs no policy call. **If a non-admin mint path is ever added** (cf. Stage 6's open
    question for ingestion keys), the requested `read_audiences` must be **⊆ the minter's own resolved
    readable set**, i.e. vetted by `ReadPolicy` on the minter — otherwise self-service minting is
    arbitrary read escalation. That gives `ReadPolicy` a second consumer at mint time, which is the one
    structural fact this stage adds to the seam.

9f. **Residual for Stage 7 docs, not code:** OIDC **client-credentials** tokens still resolve to the
    empty set — they have no key row, no `email`, and usually no `groups` claim. Grafana's OAuth mode is
    exactly this (`grafana/pkg/flightsql/oauth.go`), so a privacy deployment either decorates M2M tokens
    with a `groups` claim at the IdP or points service dashboards at analytics keys. Also document the
    limitation that **Grafana cannot be per-user** in a privacy deployment: the plugin forwards no
    end-user token, so its dashboards read the service account's audiences and its `x-user-*` headers
    remain attribution-only.

### Stage 5 — Ingestion stamping — **landed, #1373**

Full design and rationale live in `tasks/1373_ingestion_stamping_plan.md`; this section is
corrected to what actually shipped rather than left as the pre-implementation plan.

11. Read `AuthContext.bound_audience` in the two process-insert sites (native `insert_process`,
    OTLP's `register_otel_process`) and write `micromegas.audience` onto the process; strip any
    client-supplied property under the reserved `micromegas.*` namespace so the property can
    never be asserted from the payload. **Corrects this step's original, stale auth premise**:
    the issue text this step was originally drafted against claimed "OTLP handlers currently have
    no auth wiring at all, and Firehose routes are merged outside the protected router" — verified
    false on the first half (OTLP and webhook already sat inside `serve_ingestion`'s
    `auth_middleware`-covered router tree; `mkdocs/docs/otlp/index.md:17,36` already documented
    "the OTLP routes share the same auth chain as the rest of the ingestion service") and
    misleading on the second (Firehose *is* merged outside the protected router, because it can
    only authenticate via the non-standard `X-Amz-Firehose-Access-Key` header, but it already ran
    the same `AuthProvider` through its own `firehose_auth_middleware` — the gap was one missing
    `req.extensions_mut().insert(ctx)` on that middleware's success arm, not missing auth
    entirely). So this stage's actual work was resolution + a fail-closed knob
    (`{prefix}_REQUIRE_WRITE_AUDIENCE`, off by default), not adding auth wiring that already
    existed.
11a. **Landed alongside stamping, not deferred**: OTLP-derived `process_id`/`block_id` are now
    audience-scoped (`IdentityContext { audience, extra_hash_input }` folded into both formulas).
    Stamping without this would let two audiences sending identical resource attributes collapse
    onto one `process_id` — silently mislabeling the second audience's data and, since `blocks`
    also dedups on `block_id` alone, silently dropping its writes — which the design plan judged
    "not only an attack — it is the ordinary multi-tenant case," not a follow-up-worthy edge case.
11b. **Residual, deliberately deferred to a follow-up issue (Stage 5b), not this stage**:
    `insert_stream`/`insert_block` still accept any `process_id`/`stream_id` unconditionally, with
    no check that the authenticated caller is authorized to write to that specific process. A
    credential bound to audience A that discovers a `process_id`/`stream_id` belonging to
    audience B can still append events to B's process, which then inherit B's stamped audience —
    an integrity gap (no read escalation: reading B still requires a read grant on B). Deferred
    because the fix is a write-side authorization gate that shares Stage 3's (#1371) still-
    unimplemented `moka` cache layer (`process_id → audience`, `stream_id → process_id`) rather
    than duplicating that design inside #1373 — see `web_ingestion_service.rs`'s
    `insert_stream`/`insert_block_typed` doc comments for the in-tree tracking.

### Stage 6 — Audience resolution on mint + setup script (enables real per-user keys)
12. Extend the mint route with `MintPolicy::resolve_audience` (`AudienceMintPolicy`, §1): the
    request may name a `requested` audience, the policy vets it, and the resolved value is written
    to the key's `audience` column. The route, its OIDC auth and its DB access already exist from
    Stage 0 — this stage adds only the policy call. **The call site is
    `rust/analytics-web-srv/src/ingestion_keys.rs::mint_key`, not a handler on ingestion** (see the
    2026-08-12 note and 0c); the caller identity available there is `AdminUser(ValidatedUser)`, so
    the caller identity is available as `Extension<AuthContext>` per Stage 1's resolution of the
    `groups`-across-`ValidatedUser` question (step 3).
    **Narrowed 2026-08-12.** What remains open here is only **who may call the route**, not the policy's
    shape: Stage 1's admin arm (step 1) makes both stories expressible — an operator minting on behalf of
    a user (`requested = user:bob@…`, permitted because the caller is admin) and a user self-minting
    (`requested = None` ⇒ `user:<own email>`, permitted by the mintable-set formula). So the decision is
    a route-authorization one: keep `AdminUser` and have the setup script (step 13) drive an operator-run
    mint, or add a non-admin path where `MintPolicy` *is* the authorization (an admin gate on top of it
    makes self-service minting impossible). Either can land without touching `AudienceMintPolicy`.
    If the non-admin path is chosen, apply Stage 4b/9e's rule on the read side too: a non-admin may never
    mint a credential granting more than they themselves hold.
13. Setup script: OIDC device-code/loopback flow → mint → write OTLP exporter env
    (`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS=authorization=Bearer <key>`).

### Stage 7 — Activation, docs, integration tests
14. Make the isolation config **required at startup** (no default — startup error if unset);
    mkdocs isolation page + deployment/migration guide for the two profiles; two-audience
    integration tests per Testing Strategy, including **open-profile equivalence** (nothing
    hidden, maintenance functions present, `'global'` rows visible).

**Deployment stories** (revised for the Stage 4/#1372 opaque-label + grant-map model — see the
status update near the top of this document):
- *Team/open*: upgrade → import keys into the two tables, choosing a destination per key (Stage 0;
  any key that was used for **both** ingestion and queries splits into two keys — the one
  client-visible change) → stamp the ingestion keys `public` (Stage 4 — the v6 backfill's default,
  and `import`'s fallback for keys imported later) → set `MICROMEGAS_UNSTAMPED_AUDIENCE=public`
  (no `{prefix}_AUDIENCE_GRANTS` needed at all — `public` is a built-in read grant)
  → identical behavior forever; no flip, no backfill, nothing disappears.
- *Privacy*: key store + management API (Stage 0) → audience on ingestion keys (Stage 4) → audience
  resolution on mint (Stage 6) → users mint personal ingestion keys, each stamping data under its own
  audience (e.g. `alice-laptop` — an opaque name, not `user:<email>`) → grant read audiences to
  service-account analytics keys (Stage 4b — Grafana and any other
  non-human reader, or they see nothing) → a per-user audience needs an explicit grant entry (there
  is no self-audience rule under the opaque-label model — provisioning one per user is Stage 6
  territory, since minting a personal key and creating its matching grant happen in the same flow)
  → set restrictive config (`{prefix}_AUDIENCE_GRANTS` scoped to real teams, no unstamped
  audience) → per-user isolation; team sharing via an explicit grant naming the IdP groups claim,
  and later via nested groups and grants
  ([Long-term model](#long-term-model--groups-nested-membership-and-grants)).

### Later — (optional) physical boundary
15. Promote `micromegas.audience` to a first-class `audience` column; propagate through views;
    enable partition pruning and per-audience object-storage prefixing.

## Files to Modify

- Auth: `rust/auth/src/types.rs` (`bound_audience`, `groups`), `rust/auth/src/policy.rs` (new —
  traits + `Audience*` impls), `rust/auth/src/default_provider.rs` (policy factory / grant knobs),
  `rust/auth/src/oidc.rs` (groups claim, Stage 1), `rust/auth/src/user_attribution.rs` (never feed
  client-claimed identity into scope resolution, Stage 1), `rust/auth/src/api_key.rs` + new
  `db_api_key.rs` (Stage 0 — one provider parameterized by table; `audience` column on
  `ingestion_api_keys` only, Stage 4).
- Analytics (Prong A): `rust/analytics/src/lakehouse/ownership_rewrite.rs` (new),
  `rust/analytics/src/lakehouse/query.rs` (`make_session_context` + `register_lakehouse_functions`
  signatures), `rust/analytics/src/lakehouse/processes_view.rs` (audience exposure if promoted).
- Analytics (Prong B — UDTF/UDF guards): `rust/analytics/src/lakehouse/process_spans_table_function.rs`,
  `perfetto_trace_table_function.rs`, `parse_block_table_function.rs`,
  `list_partitions_table_function.rs`, and their execution plans (scan-time audience check);
  the `get_payload` UDF (same guard, scalar form); `retire_partitions_table_function.rs`,
  `materialize_partitions_table_function.rs`, `regenerate_partitions` and the
  `retire_partition_by_file` / `retire_partition_by_metadata` UDFs (registration gate instead of an
  audience check). Internal-context sites in `view.rs`, `merge.rs`, `sql_batch_view.rs`,
  `export_log_view.rs`, `batch_partition_merger.rs`, `metadata.rs` (`ReadScope::All`); recursive
  context sites in the three UDTF execution plans (inherit caller scope).
- Query service: `rust/public/src/servers/flight_sql_service_impl.rs` (resolve scope, pass through
  both call sites incl. prepared statements).
- Ingestion: `rust/public/src/servers/ingestion.rs`, `rust/public/src/servers/otlp.rs` (incl.
  auth wiring; Firehose route placement), `rust/ingestion/src/sql_telemetry_db.rs` (audience
  storage).
- Key store (Stage 0, landed): `rust/ingestion/src/sql_telemetry_db.rs` (the two tables),
  `rust/auth/src/db_api_key.rs`, `rust/monolith/src/main.rs`.
  **Not** `rust/object-cache-srv/` — it has no DB access and keeps the env keyring (Stage 0).
- Analytics-key read grants (Stage 4b): `rust/ingestion/src/sql_telemetry_db.rs` (the
  `read_audiences TEXT[]` migration), `rust/auth/src/db_api_key.rs` (`KeyRow` + `AuthContext`
  population), `rust/analytics-web-srv/src/analytics_keys.rs` (mint/import/list),
  `analytics-web-app`'s Analytics API Keys page, `python/micromegas/micromegas/web_client.py` and
  `cli/import_keys.py`.
- Key-management routes — **`rust/analytics-web-srv/src/ingestion_keys.rs`** (`audience` on
  `mint_key` and `import_key`, Stages 4/6) and its identity source
  `rust/analytics-web-srv/src/auth/claims.rs` (`ValidatedUser`, Stage 1 — see the third identity
  boundary), plus `python/micromegas/micromegas/cli/import_keys.py` and the
  `analytics-web-app` ingestion-keys page if audience becomes a mint-time input.
  **Not** `rust/public/src/servers/api_keys.rs` — deleted by #1458.

## Trade-offs

- **Set-valued rule from day one** vs. a per-user equality now, generalize later. Chosen: set-valued.
  The singleton `IN` costs nothing at runtime and is the one decision that prevents a rewrite; a
  boolean `owner = caller` special-case is exactly the corner to avoid.
- **`ReadScope::All` variant** vs. a wildcard principal string. Chosen: explicit enum — no sentinel
  that could collide with a real audience or be forged into a filter.
- **Everyone-group over a wildcard read grant** (decided 2026-07-30, revised by Stage 4/#1372 — see
  the status update near the top of this document) for open deployments. A
  user-grantable `ReadScope::All` would be exactly today's behavior, but it forks the model into
  "filtered" and "unfiltered" deployments. Chosen instead: open = the built-in `public` read grant
  (originally an implicit `group:everyone` membership) + `MICROMEGAS_UNSTAMPED_AUDIENCE=public`
  (originally `=group:everyone`) — one uniform data model where every
  deployment runs the same filtered path. The behavioral deltas that choice creates (unstamped
  legacy data, `'global'` partition rows, mutating functions) are each closed by a dedicated knob
  (Config surface) so open deployments still see byte-for-byte today's behavior.
- **No default policy / required config at GA** (decided 2026-07-30) vs. defaulting to `self` or to
  open. A `self` default breaks every existing deployment on upgrade (all data invisible); an open
  default makes the privacy posture opt-in forever. Chosen: no default — transitional
  "unset = inactive" while stages ship, then a startup error forcing a conscious operator choice.
- **Query-time coalesce for unstamped data** vs. a backfill script vs. waiting out retention.
  Chosen: `MICROMEGAS_UNSTAMPED_AUDIENCE` (query-time attribution). No data mutation, no
  re-materialization of `processes` partitions (which a property backfill would require), works the
  instant enforcement turns on. Privacy deployments leave it unset (fail-closed).
- **Eternal write keys / no `minted_by` in v1.** Accepts that revoking a subject's `write(→G)` does
  not retroactively invalidate keys already minted for G — the key *is* the frozen grant; to undo it
  you revoke the key. This matches the stated use case. If retroactive write-revocation is ever
  needed, add `minted_by` to `ingestion_api_keys` and revoke by `(minted_by, audience)` — an additive
  change.
- **Separate key tables per risk class** vs. one table with a scope column (decided 2026-07-30).
  Chosen: separate. Costs a second migration and forces dual-use keys to split at import (the one
  client-visible change in Stage 0); buys a boundary enforced by Postgres grants rather than by
  application logic on the mint path, and avoids an `audience` column that is meaningful for only
  half the rows. Rationale in Stage 0.
- **Key management over HTTP** vs. admin-gated lakehouse UDFs (decided 2026-07-30). Chosen: HTTP.
  The UDF route would put key material in `sql={sql:?}` logs that micromegas itself ingests
  (`flight_sql_service_impl.rs:330`) and would require granting the read service write access to the
  key tables; the mint API is needed for Stage 6 anyway.
- **Policy source (decided): IdP `groups` claim + implicit-groups config only.** Keeps
  confidentiality resting on OIDC plus operator config; no TCB additions. Trade-off accepted:
  membership grants both read and write for a group (no independent write-only/read-only). A local
  grants table (more expressive, but its editors join the TCB) is a deferred pure addition, not
  part of v1 — and is the recorded target state, with nested groups and separate read/mint grant
  tables, in [Long-term model](#long-term-model--groups-nested-membership-and-grants). The v1 rule is
  that model's degenerate identity-grant case, so adopting it later changes no caller's readable set.
- **Analytics keys as service accounts with per-key read grants** (decided 2026-08-12) vs. an env
  subject→groups map vs. deprecating them. Chosen: per-key `read_audiences` (Stage 4b). The env map
  would require a redeploy per new service account, which is the operational problem Stage 0 exists to
  eliminate; deprecation is unavailable because key-only flight-sql is a documented supported mode. Cost:
  minting an analytics key becomes a confidentiality grant, so that route's authorization is now a
  security control (9e), and "confidentiality rests solely on OIDC" weakens to "on the caller's
  authenticated principal".
- **Reserved property vs. first-class column** for the audience (v1 vs the later physical
  boundary): row-level filter now with zero migration, physical pruning later.
- **Public views opt-in (§5b)** vs. keeping every aggregate private. Chosen: opt-in allowlist,
  default empty. Reuses Prong A's existing per-view-set branch (`get_view_set_name`), so it adds a
  config knob rather than a new enforcement seam, and stays fail-closed until an operator names a
  view set. Deferrable past v1 with no rework.

## Security

- Confidentiality = **the caller's authenticated principal** + `ReadPolicy` per query; write-key theft is
  integrity-only. For human callers the principal is OIDC identity; for **service accounts** it is an
  analytics key whose `read_audiences` grant is the credential's own scope (Stage 4b). So a stolen
  *analytics* key does grant reads — of exactly its granted audiences, which is why minting one is a
  confidentiality grant (9e) and why the two key tables stay separate. A stolen *ingestion* key still
  grants zero reads.
- No write→read escalation (audience label ≠ read grant).
- Metadata tables/functions **must** be covered by **both** prongs or they leak process names,
  machine names, and `otel.resource.*` properties even while log bodies are hidden. Prong A covers the
  views; Prong B covers the span/metadata UDTFs the analyzer physically cannot filter. This is the
  primary correctness risk and the focus of testing.
- The five mutating functions (`retire_partitions`, `materialize_partitions`,
  `regenerate_partitions`, `retire_partition_by_file`, `retire_partition_by_metadata`) are not read
  paths; they are excluded from user sessions (registered only for maintenance contexts, admin
  sessions — issue #1377, which also closes the pre-isolation hole where every authenticated
  caller can invoke them — or when no admin principal can exist for the deployment, which is
  derived from the resolved auth provider rather than an operator opt-in) rather than
  audience-filtered — an integrity/availability control, not a confidentiality one. Without it, a
  non-admin could name
  another principal's `process_id` via `retire_partitions`' `view_instance_id` argument to destroy
  their partitions.
- **Identity holes closed in Stage 1** (would otherwise be full enforcement bypasses): the
  prepared-statement path resolves no identity (`flight_sql_service_impl.rs:842`), and
  `validate_and_resolve_user_attribution_grpc` falls back to client-claimed identity when the
  `x-auth-subject` header is absent (`user_attribution.rs:125-133`) — `ReadScope` is derived from
  the authenticated `AuthContext` only, never from client-claimed attribution.
- `{prefix}_AUDIENCE_GRANTS` and `MICROMEGAS_UNSTAMPED_AUDIENCE` are deliberate, operator-owned
  confidentiality relaxations (like §5b): setting them widens what every authenticated caller can
  read. Both are unset (empty grant map; unstamped data invisible) in a privacy deployment; the
  engine is fail-closed without them — every authenticated caller's readable set is still `{public}`
  even with an empty grant map, since `public` is a built-in, not a relaxation the operator opts
  into. (Originally `MICROMEGAS_IMPLICIT_GROUPS`, removed by Stage 4/#1372 — see the status update
  near the top of this document.)
- No admin query-path read bypass — admin FlightSQL sessions are filtered like any other. Cross-
  principal reads for operators are an out-of-band capability (direct object-store/parquet access),
  intentionally outside the query path. API keys can never be admin.
- Group grants add a single trust dependency: the IdP's `groups` claim (plus operator-set implicit
  groups). No local policy store **in v1**, so the TCB gains no new members; per Stage 4b, whoever may
  mint analytics keys can grant read access, and per the long-term model, a group/grant store adds its
  editors to the TCB when it lands.
- **Delegation never affects `ReadScope`.** API keys carry `allow_delegation: true` and clients (Grafana,
  python services) send `x-user-id`/`x-user-email`; those headers are audit attribution only. A service
  account's readable set is its own grant, and a delegated user's claimed identity neither widens nor
  narrows it — the same rule as hole #2, applied to the credential kind that makes it tempting.
- Public views (§5b) are an explicit, opt-in confidentiality relaxation: a listed view set is
  readable by every authenticated caller, so only genuinely aggregated / non-PII view sets may be
  listed. The default allowlist is empty (fail-closed); the raw global `log_entries` / `measures`
  instances must never be listed, and the arg-addressed process-scoped UDTFs are never exempted.

## Testing Strategy

- **Key store + management API (Stage 0, independent of everything below):** a DB key authenticates
  and an unknown key is rejected; a revoked key stops authenticating within the cache TTL (assert the
  stated revocation-latency property, don't leave it implicit); env keyring and DB keyring compose — a
  key in either authenticates during the transition; an imported row (via the `/import` route or the
  hand-written SQL fallback) round-trips an existing key string so the *same key string* still
  authenticates on its own surface afterwards (the zero-client-change claim, so it deserves a real
  test); no cleartext key is stored — assert the column holds the hash.
  **Surface separation (the load-bearing property of the split):** a key in `ingestion_api_keys` is
  rejected by flight-sql and a key in `analytics_api_keys` is rejected by ingestion — assert both
  directions, since a provider constructed against the wrong table is the failure mode the two-table
  design exists to prevent. Assert each key-management router writes only its own table — the
  ingestion-keys routes never touch `analytics_api_keys` and vice versa (the code-level boundary
  that replaced "the ingestion role has no INSERT grant" once both routers moved into one process,
  and therefore the assertion that now carries the two-table split).
  **Management routes:** create returns a key that then authenticates; the cleartext is returned once
  and never retrievable afterwards; list omits the hash column; revoke is idempotent; every route
  rejects an API-key-authenticated caller (admin requires OIDC, `api_key.rs:124`); under
  `--disable-auth` both prefixes answer 503 rather than falling through to the SPA.
- **Unit:** `AudienceMintPolicy` rejects `requested` outside the mintable set, defaults to
  `user:<email>`, and — the admin arm — accepts an arbitrary well-formed audience for an `is_admin`
  caller while rejecting that same value for a non-admin, and rejects a malformed (unprefixed) audience
  for both. `AudienceReadPolicy` returns `{user:} ∪ claim groups ∪ implicit groups` and the singleton
  when both group sources are empty.
- **Unit — service-account scope (Stage 1 shape, Stage 4b data):** a caller with
  `read_audiences = [a, b]` and no email/groups resolves exactly `{a, b} ∪ implicit` with **no** `user:`
  element; the same caller with an empty grant resolves implicit-only (∅ in a privacy deployment). Assert
  a request carrying `x-user-id`/`x-user-email` for a *different* principal resolves the identical scope —
  delegation must not move it either way.
- **Fail-closed resolution:** a `ReadPolicy` stub returning `Err` makes the request fail
  (`unavailable`/`permission_denied`); assert it does **not** produce an empty, default, or `All` scope.
  This is the test that keeps property 2 of the long-term model true once the policy starts doing I/O.
- **Stage 4b:** a key minted with a grant authenticates and carries it into `AuthContext`; a key minted
  without one carries `[]`; `GET` returns `read_audiences` and still omits `key_hash`; a grant change is
  visible within the cache TTL (or, per 9c, is not possible at all — assert whichever shipped). Prong A: `OwnershipRewrite` injects the expected
  predicate per table kind (snapshot the rewritten logical plan), including `view_instance` and the
  `coalesce` form when `MICROMEGAS_UNSTAMPED_AUDIENCE` is set. Prong B: each guarded UDTF/UDF
  (incl. `get_payload`) rejects an unowned `process_id`/`block_id` and `list_partitions`
  row-filters — assert both fail closed; assert all five mutating functions are absent
  ("function not found") from a non-admin session's registration when an admin principal can
  exist for the deployment, present when one cannot (`admin_principal_possible`), and present for
  an admin session regardless (#1377). Public views (§5b): with a view
  set on the allowlist, `OwnershipRewrite` injects no predicate for it and `list_partitions` shows
  its `'global'` rows; with an empty allowlist behavior is unchanged (every set filtered).
- **Integration (privacy profile):** two audiences seeded; assert each sees only its own rows
  across `processes`, `log_entries`, `measures`, spans, `view_instance`, `list_partitions`; assert
  the `process_id` semi-join blocks naming another audience's process directly; assert unstamped
  rows are hidden; assert the daemon (`ReadScope::All`) sees everything and that an **admin user
  session is still filtered** (no bypass); assert the prepared-statement path is filtered
  identically to `do_get`.
- **Integration (open profile — equivalence with today):** with `MICROMEGAS_UNSTAMPED_AUDIENCE=public`,
  no admin principal possible for the deployment (no `MICROMEGAS_IMPLICIT_GROUPS` — removed by
  Stage 4/#1372, since `public` is a built-in read grant needing no companion knob), and
  a mix of stamped (`public`) and unstamped data: every caller sees every row, `'global'`
  partition rows are listed, and the mutating functions are registered — byte-for-byte the
  pre-isolation behavior.
- **Equivalence (per-user plan):** confirm the executed plan in the privacy profile matches the
  intended per-user filter (a singleton `IN`), i.e. no behavioral difference from a hand-written
  per-user design.
- **Group grants:** group member reads group data; write-only producer (`write(→G)`, no
  `read(→G)`) cannot read G including its own writes; membership change reflected on next query.
- Rust: `cargo test`, `cargo clippy --workspace -- -D warnings`, `cargo fmt`; CI via
  `python3 build/rust_ci.py`.

## Documentation

- New page under `mkdocs/docs/` for the isolation model: the three-relation model, the grant knobs
  and the two deployment profiles (open / privacy), the required-at-GA activation story, the
  `MICROMEGAS_PUBLIC_VIEW_SETS` allowlist (§5b, with its non-PII caveat), and the
  confidentiality/integrity properties.
- Update any auth/deployment docs to mention the two key tables and why they are separate, the
  key-management routes (create/revoke/list/import, on `analytics-web-srv`) and the
  revocation-latency property, the legacy-key import procedure **including the dual-use-key split**,
  the setup script, and the groups-claim configuration. Stage 0's own docs
  (`mkdocs/docs/admin/api-keys.md`, `admin/web-app.md`, `admin/ingestion.md`) already describe the
  single relocated surface as of #1458 — the isolation stages add `audience` to that story rather
  than re-describing it.
- **Service accounts (Stage 4b):** `admin/api-keys.md` gains the `read_audiences` grant on analytics keys
  and the statement that minting one grants read access; `grafana/authentication.md` gains the two
  consequences from 9f — an M2M OAuth token resolves to nothing unless the IdP decorates it with a
  `groups` claim, and Grafana dashboards in a privacy deployment are scoped to the service account, not to
  the viewing user.
- **Groups (long-term, not v1):** when the group/grant store lands it needs its own page — the membership
  vs. grant distinction, edge direction, grant-latency (cache TTL) property, and the two-sided
  authorization rule for the admin surface.

## Resolved Decisions

Resolved by research (kept here for the record; details in Appendix A):
- ~~**Table-function coverage.**~~ **Resolved:** two-pronged — analyzer rule for `MaterializedView`
  scans incl. `view_instance` (Prong A); construction-time guard threaded with `ReadScope` for the
  span/metadata UDTFs (Prong B), with the audience check at async scan time. A single analyzer rule is
  provably insufficient (owner id absent from schema, opaque in provider, filters ignored).
- ~~**Audience identifier shape.**~~ **Resolved:** value-prefix `user:<email>` / `group:<id>` in a
  single dotted-namespace property `micromegas.audience` (matches `otel.resource.*` convention; no
  collision possible).
- ~~**Audience storage for v1.**~~ **Resolved:** reserved property `micromegas.audience`; in-tree
  usage of `property_get` in WHERE predicates is equality only; the `IN (...)` form relies on
  DataFusion's dictionary-type coercion (`property_get` returns `Dictionary(Int32, Utf8)`). Promote to
  a column in the later physical-boundary stage.
- ~~**Groups-claim feasibility.**~~ **Resolved:** one-line additive `Claims`/`AuthContext` change,
  backward-compatible; Auth0/Azure AD/Google flat arrays; `MICROMEGAS_ADMINS` is the config precedent.
- ~~**Grant source.**~~ **Decided: IdP `groups` claim (+ implicit-groups config) only** for v1 (no
  local grants table). Keeps confidentiality on OIDC and the TCB unchanged; accepted
  trade-off is that membership grants both read and write for a group. A grants table (or a second
  write-role claim) is a deferred pure addition — and, as of 2026-08-12, the **intended end state**
  rather than a mere option: see Stage 1 step 5 and
  [Long-term model](#long-term-model--groups-nested-membership-and-grants).
- ~~**Admin read bypass.**~~ **Decided: no query-path bypass.** `is_admin` does not map to
  `ReadScope::All`; admin sessions are filtered like any other. Operators needing cross-principal
  reads use direct object-store/parquet access, which they already have — a query bypass would add
  attack surface and audit burden for no confidentiality gain. Only the maintenance daemon is
  unfiltered. See §5.
- ~~**`list_view_sets` exposure.**~~ **Decided: stays unfiltered** — view-set schema/definitions only,
  no PII or per-principal data. Only `list_partitions` is row-filtered. See §4 Prong B.
- ~~**`retire_partitions` / `materialize_partitions` exposure.**~~ **Decided (revised 2026-07-30):
  registered for maintenance contexts, admin sessions (issue #1377), or under
  `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS=true`.** (Superseded by Stage 3, #1371: no such knob
  shipped — the third arm derives from whether an admin principal can exist for the deployment;
  see the correction in §4.) Both were missing from
  the original Prong B audit despite being registered unconditionally alongside the other UDTFs;
  the 2026-07-30 audit added `regenerate_partitions` and the `retire_partition_by_file` /
  `retire_partition_by_metadata` UDFs to the set. All mutate lakehouse state, so none gets an
  audience read-filter — instead `register_lakehouse_functions` skips registering them for
  non-admin user sessions unless no admin principal can exist for the deployment (derived
  automatically, not an operator opt-in). The admin arm also closes a pre-isolation hole (today
  every authenticated caller can invoke them) and may land first. See §4 Prong B and Appendices A–B.
- ~~**Scan-time check cost.**~~ **Resolved:** `process_id → audience` is immutable, so an
  invalidation-free size-bounded `moka` cache (backed by `find_process`) makes the check an O(1)
  in-memory lookup on warm hits, one indexed PG query per process ever on cold miss. `ReadScope` is
  free (from the JWT `groups` claim). See §4 "Prong B performance".

Decided 2026-07-30 (deployment-staging revision, from issue #1334 follow-up discussion):
- **No mode enum, no default policy.** The former `MICROMEGAS_ISOLATION_POLICY=self|audience` knob and
  its `self` default are gone. One AbAC engine configured by grants; per-user isolation is the
  empty-grants configuration, not a separate mode. Transitional unset-config = enforcement
  inactive; at GA the config is required at startup.
- **Open deployments = everyone-group configuration** (chosen over a user-grantable
  `ReadScope::All` wildcard): implicit `everyone` membership, imported keys stamp
  `group:everyone`, unstamped data coalesces to `group:everyone`. One uniform filtered path for
  every deployment.
- **Existing API keys are imported into the DB key store** (same key strings, audience
  `group:everyone`) — the migration vehicle for team deployments; zero client changes except for
  dual-use keys (see below). The key store therefore moves early in the ordering — **Stage 0**,
  ahead of the policy seam and depending on nothing (revised 2026-07-30); only the `audience` column
  stays behind the seam, as Stage 4.
- **Write and read keys live in separate tables** (`ingestion_api_keys` / `analytics_api_keys`,
  decided 2026-07-30, issue #1383): one key is never valid on both surfaces. The risk is asymmetric
  (write = integrity, read = confidentiality) and Stage 6 distributes ingestion keys to thousands of
  machines. The boundary is enforced **in code** — each router is hardcoded to one table, each
  provider constructed bound to one table — with per-service Postgres grants documented as an
  operator option rather than shipped (every service shares one DB role by default, and after
  #1458 both routers live in the same process, so a grant split cannot separate them anyway).
  Only ingestion keys carry an `audience`. Consequence: a key currently used
  for both ingestion and queries (the unprefixed `MICROMEGAS_API_KEYS` fallback) must split into two
  at import — the one place zero-client-change does not hold.
- **`object-cache-srv` keeps the env keyring** (decided 2026-07-30): it has no DB access, and a cache
  service does not earn a Postgres pool just to read a key table. Therefore `ApiKeyAuthProvider` /
  `parse_key_ring` are **permanent**, not transitional, and that service's keys stay
  redeploy-to-revoke — acceptable because they are service-held and never distributed.
- **Key management is an OIDC-authenticated HTTP API**, not admin-gated
  lakehouse UDFs (decided 2026-07-30). Create/revoke/list move into **Stage 0**, since the table
  alone does not deliver the revoke-without-redeploy value; Stage 6 only adds audience resolution to
  the existing create route. UDFs were rejected because query text is logged into micromegas's own
  `log_entries` (`flight_sql_service_impl.rs:330`) and because a write UDF would grant the read
  service write access to the key tables. **Host service revised 2026-08-12 (#1411/#1458):** the API
  moved from ingestion to `analytics-web-srv` and ingestion's `/auth/api_keys*` routes were deleted;
  Stage 6's `resolve_audience` call site moves accordingly. The UDF rejection is unaffected — the
  logging argument was never about which service hosted the route.
- **Unstamped data: query-time coalesce knob** (`MICROMEGAS_UNSTAMPED_AUDIENCE`), not a backfill
  and not a retention wait. Unset = hidden (fail-closed, privacy profile).
- **Mutating functions: registration gate — maintenance ∨ admin ∨ deployment opt-in**
  (`MICROMEGAS_USER_MAINTENANCE_FUNCTIONS`) rather than unconditionally maintenance-only.
  (Superseded by Stage 3, #1371: the opt-in arm derives from whether an admin principal can exist
  for the deployment rather than from an operator-set knob; see the correction in §4.) Admin
  sessions always get them (issue #1377 — standalone, closes today's
  any-authenticated-caller hole, may land before the isolation stages); the derived arm keeps them
  available to non-admins in deployments where no admin principal can exist.
- **Prong B coverage extended** after the 2026-07-30 drift audit: `regenerate_partitions`,
  `retire_partition_by_file`, `retire_partition_by_metadata` join the mutating set; `get_payload`
  gets the arg-addressed read guard. See Appendix B.
- **Identity holes are in scope for Stage 1**: prepared-statement path identity resolution and the
  client-claimed-attribution fallback (never feeds `ReadScope`).

Decided 2026-08-12 (from the #1369 planning review):
- **Analytics keys are service accounts with configurable read grants** (`read_audiences TEXT[]`, new
  Stage 4b), superseding "read scope never comes from a key" and "`analytics_api_keys` may be
  transitional". Reasons: key-only flight-sql is a documented supported deployment, and the empty-scope
  problem was never key-specific (OIDC client-credentials tokens have no email either), so a
  key-specific `ReadPolicy` branch would have fixed half of it. **Rejected:** an env
  subject→groups map (every new service account would need a redeploy — the operational problem Stage 0
  exists to kill); delegation-derived scope (client-claimed identity, hole #2); deprecating analytics keys
  (a documented mode needs a deprecation cycle, not a stage decision).
- **`AudienceReadPolicy`'s union is branch-free** — `caller.read_audiences ∪ {user:email} ∪ claim groups
  ∪ implicit` — because the grant rides on `AuthContext` rather than being resolved by credential kind. No
  `auth_type` check exists anywhere in the policy.
- **`AudienceMintPolicy` has an admin arm** (`is_admin` ⇒ any well-formed audience). Without it the
  admin-gated mint route can only ever produce `user:<admin email>`, so Stage 6 would stamp every fleet
  key with the minting admin's audience. This narrows Stage 6's open question to *who may call the route*,
  which no longer changes the policy's shape.
- **Both policy traits are `async` and fallible, and resolution failures deny.** Required by the
  long-term store-backed grant source; free today. The deny-on-`Err` branch is written in Stage 1, with a
  test, because a claim-only policy cannot fail and a permissive fallback would stay invisible until it
  matters.
- **`MintPolicy` takes `&AuthContext`**, and `analytics-web-srv` inserts the `AuthContext` into request
  extensions (one line) rather than growing `ValidatedUser` — resolving the third identity boundary in
  Stage 1 as the plan required.
- **Target state recorded:** users belong to groups, groups nest, and a group is granted a set of
  audiences it may read — with today's rule as the degenerate identity-grant case. See
  [Long-term model](#long-term-model--groups-nested-membership-and-grants). This promotes the deferred
  grants table from "possible addition" to "the intended end state", and accepts its TCB cost.

All design decisions are closed **except one, narrowed** (2026-08-12): the mint route is admin-gated
today, so per-user key issuance in the privacy profile is either an operator-run mint driven by the setup
script or a non-admin route where `MintPolicy` is the authorization — a route-authorization choice made in
Stage 6 (step 12), with both options already expressible by the shipped policy. Everything Stage 1 needs
is settled. Remaining work is implementation, staged per the Implementation Steps; Stage 0 has landed and
Stage 1 is unblocked.

## Appendix A — Research findings (2026-07-21)

Grounded against the current tree; file:line refs verified.

**Table functions (DataFusion 54.0).** UDTFs are registered via `ctx.register_udtf` in
`register_lakehouse_functions` (`query.rs:102-155`) and resolve at SQL-planning time into
`LogicalPlan::TableScan` nodes wrapping a `DefaultTableSource(provider)` — so an `AnalyzerRule` *can*
see them. But:
- `view_instance` returns a `MaterializedView` (`view_instance_table_function.rs:76`) → already
  rewritten by `TableScanRewrite`; Prong A handles it.
- `process_spans` (`ProcessSpansTableProvider`, `process_spans_table_function.rs:366`),
  `perfetto_trace_chunks`, `parse_block`, `list_partitions`, `list_view_sets` do **not** expose their
  owner id in the output schema, and bake `process_id`/`stream_id` opaquely into the provider at plan
  time via `exp_to_string` (`process_spans_table_function.rs:110`, `perfetto:71`). `scan()` even
  ignores `_filters` (`process_spans_table_function.rs:384`). ⇒ predicate injection is impossible for
  these; **guard-at-construction (Prong B)** is the only uniform enforcement point. `call_with_args`
  is synchronous → pass a pre-resolved `ReadScope`, defer the metadata-dependent check to async scan.
- `process_thread_spans_table_function.rs~` is a dead backup (not compiled/registered) — ignore.
- `retire_partitions` (`query.rs:119-122`) and `materialize_partitions` (`query.rs:131-137`) are the
  remaining two of the eight registered UDTFs and were absent from the original audit above. Neither
  is a read. `retire_partitions` deletes `lakehouse_partitions` rows for a
  `(view_set_name, view_instance_id)` pair (`write_partition.rs:116`) — destructive — and
  `view_instance_id` is a `process_id` for process-scoped view sets, so it has the same opaque,
  unchecked argument shape as `process_spans`. `materialize_partitions` takes no per-process id — it
  materializes a *global* view (`view_factory.get_global_view`) over an insert-time range — so it
  can't target another principal's data but is still an unbounded write with no read-session use
  case. **Decided:** gate registration of both on `ReadScope::All` (maintenance-only) rather than
  extending the audience check to them — they're integrity/availability concerns, not confidentiality
  ones.
- Identity is already resolved at `flight_sql_service_impl.rs:317` but currently used only for audit;
  it is not passed to `make_session_context`/`register_lakehouse_functions`.

**OIDC claims (`oidc.rs`).** `Claims` struct at `:193-227`; `get_email()` priority chain at
`:232-240`; no `#[serde(deny_unknown_fields)]` ⇒ adding `groups`/`roles` is additive and
absent-safe. `is_admin` = allowlist match on `sub`/email from `MICROMEGAS_ADMINS`
(`load_admin_users` `:264-269`, check `:390-394`) — the precedent for group→capability config.
Targets: Auth0, Azure AD, Google (Keycloak nested claims not currently targeted). `AuthContext`
(`types.rs:14-37`) has no groups field yet — must be added for AbAC grants.

**Properties.** `micromegas_property = (key TEXT, value TEXT)` (`sql_telemetry_db.rs:17`),
`processes.properties micromegas_property[]` (`:39`) — arbitrary strings, no per-key typing.
`property_get` returns `Dictionary(Int32, Utf8)`, case-insensitive key match, `NULL` when absent
(`rust/datafusion-extensions/src/properties/property_get.rs:48,87-92`); used in WHERE across the
codebase (e.g. `rust/public/src/client/query_processes.rs:73`). No
`micromegas.` reserved-key convention exists yet, but `otel.resource.*` is the established dotted
namespace (`otel-ingestion/src/block.rs:467-475`); OTel `process.owner`/`host.name` already land as
`otel.resource.*` properties (demote to display-only). No user/group value discriminator exists ⇒
adopt `user:`/`group:` prefixes.

## Appendix B — Drift audit (2026-07-30)

Re-verification of Appendix A against HEAD `2e95770` (branch `privacy`). Confirmed unchanged unless
listed. Nothing from the design vocabulary (`ReadScope`, `MintPolicy`, `micromegas.audience`,
`bound_audience`, …) is implemented yet — zero hits in `rust/`.

- **Function registry grew.** `register_lakehouse_functions` is now `query.rs:96-180` and registers
  **nine** UDTFs, not eight: `view_instance` (:103), `list_partitions` (:112), `list_view_sets`
  (:116), `retire_partitions` (:120), `perfetto_trace_chunks` (:124), `materialize_partitions`
  (:132), **`regenerate_partitions` (:139 — new, mutating)**, `parse_block` (:146), `process_spans`
  (:155). It also registers three UDFs the original audit didn't cover: **`get_payload` (:165)** —
  an async raw-payload **read** by block id, needing the same audience guard as `parse_block` — and
  the destructive **`retire_partition_by_file` / `retire_partition_by_metadata` (:168, :170)**,
  which join the mutating set.
- **Identity holes.** The prepared-statement path (`flight_sql_service_impl.rs:842`,
  `do_action_create_prepared_statement`) builds its session context with **no identity resolution**
  and passes `query_range = None`. `validate_and_resolve_user_attribution_grpc`
  (`rust/auth/src/user_attribution.rs:108`) **falls back to client-claimed identity** when
  `x-auth-subject` is absent (`:125-133`). Identity crosses the tower `AuthService` boundary as
  stringified gRPC metadata (`x-auth-subject`, `x-auth-email`, `x-allow-delegation`) — no
  groups/audience carried today.
- **No maintenance chokepoint.** The daemon (`telemetry-maintenance-srv/src/main.rs:40` →
  `servers/maintenance.rs:296`) never calls `make_session_context`; internal contexts are built
  per-view at ~10 sites hardcoding `NoOpSessionConfigurator` (`view.rs:109` — with an empty
  `ViewFactory` at :107, `merge.rs:101`, `sql_batch_view.rs:87,154`, `export_log_view.rs:118,171`,
  `batch_partition_merger.rs:133`, `metadata.rs:182,287`). Three context-building sites are
  reachable from user queries and must inherit the caller's scope:
  `parse_block_table_function.rs:81`, `process_spans_table_function.rs:254`,
  `perfetto_trace_execution_plan.rs:232`.
- **`TableScanRewrite` registration is conditional** on `query_range.is_some()` (`query.rs:206`);
  `OwnershipRewrite` must be unconditional.
- **Ingestion auth gaps.** No ingestion handler extracts `Extension<AuthContext>` (available since
  `rust/auth/src/axum.rs:75` inserts it); `otlp.rs` has no auth wiring at all; Firehose routes are
  merged outside the protected router (`ingestion.rs:151-156`). Stamping (Stage 5) must define
  their auth story.
- **Line-ref updates.** `validate_and_resolve_user_attribution_grpc` at
  `flight_sql_service_impl.rs:318` (was :317); `make_session_context` call sites at :372 and :842
  (were :371/:841); `make_session_context` itself at `query.rs:194`; `retire_partitions` at
  `query.rs:120`, `materialize_partitions` at `query.rs:132`.
- **Config factory precedent** for the grant knobs: `static_tables_configurator.rs:44-54`
  (`from_env` returning a no-op when unset).

## Appendix C — Drift audit (2026-08-12, before Stage 1)

Re-verification against HEAD `213ed3b`. Appendices A and B hold except as listed. The design
vocabulary (`ReadScope`, `MintPolicy`, `micromegas.audience`, `bound_audience`, `AudienceReadPolicy`)
is still **entirely unimplemented** — zero hits in `rust/`.

**Stage 0 landed, and the key-management surface then moved.** #1383 shipped the two tables and
`DbApiKeyAuthProvider`; #1411 added the web admin UI and the import route; **#1458 deleted
ingestion's `/auth/api_keys*` routes and `rust/public/src/servers/api_keys.rs` entirely.** Key
management now lives in `rust/analytics-web-srv/src/ingestion_keys.rs` and `analytics_keys.rs` under
`{base_path}/api/{ingestion,analytics}-api-keys`, gated by the `AdminUser` extractor over the cookie
session, writing the telemetry DB directly through the pool the service already opens. Consequences
folded into the body above: 0c/0d rewritten, Stage 4 step 10 and Stage 6 step 12 re-pointed, Files
to Modify and Testing updated, and the "not mintable through this API" property for analytics keys
recorded as superseded.

**Third identity boundary (new, Stage 1 scope).**
`impl From<&AuthContext> for ValidatedUser` (`rust/analytics-web-srv/src/auth/claims.rs:40-48`)
keeps only `subject`/`email`/`issuer`/`is_admin` — a new `AuthContext.groups` field is dropped
there silently. This is now the mint path's identity source, so it joins the prepared-statement and
client-claimed-attribution holes as something Stage 1 must decide rather than let Stage 6 discover.

**The two original identity holes are unchanged.**
- `do_action_create_prepared_statement` (`flight_sql_service_impl.rs:1142-1158`) still builds its
  session context with no user identity and `query_range = None`. It does now pass
  `is_admin(request.metadata())`, so the RPC reads *some* identity from metadata — but no
  subject/email resolution, which is what `ReadScope` would need.
- `validate_and_resolve_user_attribution_grpc` (`rust/auth/src/user_attribution.rs:127`) still falls
  back to client-claimed identity when `x-auth-subject` is absent.

**Line-ref updates** (the previous audit's numbers are stale enough to misdirect):
- `make_session_context` is `query.rs:207` (was `:194`); `register_lakehouse_functions` is still
  `query.rs:96`.
- The two flight-sql call sites are `flight_sql_service_impl.rs:661` (`do_get`/execute path) and
  `:1149` (prepared statement) — were `:372`/`:842`. #1369's issue text still cites the old pair.
- `validate_and_resolve_user_attribution_grpc` is called at `flight_sql_service_impl.rs:573`
  (was `:318`); its definition is `user_attribution.rs:127` (was `:108`).
- The registry shape is unchanged from Appendix B: nine `register_udtf` calls and the
  `get_payload` / `retire_partition_by_file` / `retire_partition_by_metadata` UDFs, several behind
  the existing registration conditionals.
- `make_session_context` has more callers than Appendix B's list implies — adding a `read_scope`
  parameter touches `analytics/src/metadata.rs:182,283`,
  `lakehouse/perfetto_trace_execution_plan.rs:254` and `lakehouse/export_log_view.rs` as well as the
  two flight-sql sites. All are internal/maintenance contexts (`ReadScope::All`) except the
  perfetto plan, which must inherit the caller's scope.
