# Admin-Managed Query Deny List Plan

Issue: [#1488](https://github.com/madesroches/micromegas/issues/1488)

## Overview

A single misbehaving client — a dashboard on a short refresh interval, an alert rule that re-fires
on failure, a notebook cell in a retry loop — can keep a heavy query in flight continuously and
saturate `flight-sql-srv`. Today the only remedies are restarting the service (the offender comes
back on its next tick) or revoking the caller's credentials (too blunt for a shared service
account). This plan adds a deny list of admin-managed rules, stored in Postgres, cached in every
`flight-sql-srv` replica, and evaluated at the front of `execute_query` — before the session
context is built and before planning — so a matching query is rejected for a few microseconds of
work instead of a memory-pool reservation and a wave of object-store reads.

Rules carry a reason and their creator, and stay in force until an admin removes them explicitly.
They are administered from SQL (`list_query_denials()` / `deny_queries(...)` / `remove_query_denial(...)`,
admin-gated exactly like `retire_partitions`) and from a new **Admin → Query Deny List** screen in the
analytics web app that drives those same SQL functions.

This is the manual valve an on-call admin can pull without a deploy. Rate limiting, per-user
concurrency caps, and cost-based admission control remain out of scope (separate issues).

## Current State

### Query path

`FlightSqlServiceImpl::execute_query` (`rust/public/src/servers/flight_sql_service_impl.rs:591`)
runs, in order:

1. mint `query_id`, parse SQL from the ticket, parse `query_range_begin`/`end` headers (lines 601-640)
2. `validate_and_resolve_user_attribution_grpc(metadata)` → `attr` (line 643)
3. read `x-client-type` / `-agent` / `-entrypoint` / `-session` / `-notebook` / `-cell` (lines 645-659)
4. build `ScopedMemoryPool` and `QueryAuditState` (lines 686-718) — **the first point where all
   attribution is assembled**
5. `scoped_runtime` → `caller_context` → `make_session_context` (lines 724-744) — session context,
   view registration, `OwnershipRewrite`
6. `ctx.sql(sql)` (planning), optional `limit`, physical plan, `execute_stream`, Flight encoding

Steps 5-6 are where the money goes: JIT partition materialization, object-store reads, memory-pool
reservations. Step 4 is the natural insertion point for the check.

`do_get_statement` (line 1036) and `do_get_fallback` (line 866) are the only callers of
`execute_query`. `do_action_create_prepared_statement` (line 1235) is a second, cheaper planning
path that builds a session context and plans without executing.

### Audit record

`QueryAuditRecord` (`rust/public/src/servers/query_audit.rs:79`) already carries every field a rule
would want to match on: `client_ip`, `client`, `agent`, `entrypoint`, `session`, `notebook`, `cell`,
`user`, `email`, `service_account_name`, and the raw `sql`. It is emitted as one JSON line under the
`flightsql_query_audit` target on every terminal path — success, failure, and abandoned-mid-drain
(`QueryAuditState::emit`, line 314; `fail`, line 369; `Drop`, line 406). It does **not** currently
carry a normalized-SQL fingerprint.

`error_class` is derived from the gRPC code by `error_class()` (line 116): `InvalidArgument` /
`Unimplemented` → `"user"`, `ResourceExhausted` → `"resource"`, everything else → `"internal"`.

### Admin gating

`register_lakehouse_functions` (`rust/analytics/src/lakehouse/query.rs:176-201`) registers the
mutating functions (`retire_partitions`, `materialize_partitions`, `regenerate_partitions`,
`retire_partition_by_file`, `retire_partition_by_metadata`) only when
`caller.is_admin || !caller.admin_principal_possible`. `CallerContext`
(`rust/analytics/src/lakehouse/read_scope.rs`) carries `read_scope`, `is_admin`,
`isolation_config`, `admin_principal_possible` — **no caller identity** (no user id / email).

`is_admin` comes from the `is_admin` gRPC header, stamped by `AuthService`
(`rust/auth/src/tower.rs:102-139`) from `AuthContext.is_admin` with client-supplied copies
stripped; absent header means trusted admin (`--disable-auth`). API keys hardcode
`is_admin: false` (`rust/auth/src/db_api_key.rs:350`), so a dashboard or alert service account can
never be an admin principal.

### Postgres and caching precedents

- Lakehouse schema migrations: `rust/analytics/src/lakehouse/migration.rs`,
  `LATEST_LAKEHOUSE_SCHEMA_VERSION = 8`. `LakehouseContext::from_env`/`new` run `migrate_lakehouse`
  at startup (`lakehouse_context.rs:45,60`), so **`flight-sql-srv` itself migrates on boot**.
- `AudienceIndex` (`rust/analytics/src/lakehouse/audience_guard.rs:202`) is the precedent for a
  Postgres-backed, TTL-cached lookup owned by `LakehouseContext` and reached from `query.rs` via
  `lakehouse.audience_index()`.
- Row-returning UDTF over Postgres: `list_partitions_table_function.rs`. Mutating UDTF:
  `retire_partitions_table_function.rs` (streams a log via `LogStreamTableProvider`).
- `sqlparser` 0.62 (with the `visitor` feature) is reachable as `datafusion::sql::sqlparser`;
  `sha2` is a workspace dependency; `regex` is **not** a direct dependency anywhere.

### Web app

- `AdminPage.tsx` is a card grid of admin destinations, each `AuthGuard requireAdmin`.
- `ApiKeysAdminPage.tsx` is the closest full-page precedent (table + create dialog + `ConfirmDialog`
  + `ErrorBanner`), but it talks to `analytics-web-srv` REST routes backed by the telemetry DB.
- SQL-driven pages (`ProcessesPage.tsx`, `ProcessLogPage.tsx`, …) use `useStreamQuery` →
  `POST /api/stream-query` (`rust/analytics-web-srv/src/stream_query.rs`), which forwards the
  browser user's own bearer token to FlightSQL via `BearerFlightSQLClientFactory`. So an admin's
  web query resolves `is_admin: true` at `flight-sql-srv` and sees the admin functions.
- `stream_query.rs:89` holds `BLOCKED_FUNCTIONS`, a substring blocklist that refuses
  `retire_partitions*` in web queries.

## Design

### 1. Storage — `query_deny_list` table (lakehouse migration v9)

```sql
CREATE TABLE query_deny_list (
  rule_id      UUID PRIMARY KEY,
  created_at   TIMESTAMPTZ  NOT NULL,
  created_by   VARCHAR(255) NOT NULL,
  reason       TEXT         NOT NULL,
  -- a boolean SQL expression over the match context (§3)
  match_expr   TEXT         NOT NULL,
  hit_count    BIGINT       NOT NULL DEFAULT 0,
  -- NULL until the rule first fires; the "is this rule still doing anything?" signal
  last_hit_at  TIMESTAMPTZ
);
```

One `match_expr` column, not a column per matcher. A fixed column set fossilizes the matching
language into the schema: every new attribute is a migration, and the only combinator it can ever
express is AND. A single expression column carries an arbitrary predicate today and can grow
richer semantics later without touching the schema at all — the evolution path §3 describes.

`hit_count` and `last_hit_at` are both flushed on the refresh tick (§4). `last_hit_at` matters more
here than it would with expiring rules: with rules standing until removed, "last fired three weeks
ago" is what tells an operator a rule is stale and safe to remove, and "last fired four seconds
ago" is what tells them the offender has not been fixed.

No expiry column: a rule is in force from insertion until `remove_query_denial` deletes it. The
table holds at most `MICROMEGAS_QUERY_DENY_MAX_RULES` rows (§10) and needs no index — every replica
reads all of it on each refresh tick.

`LATEST_LAKEHOUSE_SCHEMA_VERSION` 8 → 9, `upgrade_v8_to_v9`. No `SCHEMA_VERSION` (partition
file-schema) change: no partition content is affected.

### 2. Normalized SQL fingerprint

New in `rust/analytics/src/lakehouse/query_deny_list.rs`:

```rust
/// Literal-stripped fingerprint of a statement: the first 16 hex chars of the
/// SHA-256 of the normalized token stream.
pub fn sql_fingerprint(sql: &str) -> String;
```

Implementation: `sqlparser::tokenizer::Tokenizer` over `GenericDialect`, then

- every `Token::Number`, `SingleQuotedString`, `DoubleQuotedString`, `NationalStringLiteral`,
  `HexStringLiteral`, `EscapedStringLiteral` → `?`
- whitespace runs collapse to one space; comments dropped
- keywords/identifiers lowercased (identifiers are already case-insensitive in DataFusion unless
  quoted; quoted identifiers keep their case since they arrive as `DoubleQuotedString`… which is
  also a literal token — see Open Questions)
- tokens joined with a single space, then SHA-256, hex, truncated to 16 chars (64 bits)

Tokenization failure falls back to hashing the whitespace-collapsed raw text, so a fingerprint
always exists.

This is what makes the dashboard case work: consecutive refreshes differ only in their time-range
literals, so they collapse to one fingerprint. A 64-bit fingerprint's collision odds across a
realistic query population are negligible, and the consequence of a collision is bounded by the
rule's other match predicates.

The fingerprint is computed once per query in `execute_query` and stored on `QueryAuditState`, so
it costs nothing extra to also emit it in the audit record (§7) — which is how an operator gets the
value to paste into `deny_queries`.

### 3. Match expression

#### Prior art

"Express a predicate over request attributes, store it, evaluate it safely" is a well-trodden
problem, with a few established answers:

| Approach | Where it's used | Fit here |
|---|---|---|
| **CEL** (Common Expression Language) | Kubernetes admission policies & CRD validation, Envoy RBAC and rate-limit matching, Istio | The closest thing to a standard for exactly this job — typed, non-Turing-complete, bounded evaluation. Rust support exists (`cel-interpreter`), but it means a new dependency and a second expression language in a product whose interface is already SQL. |
| **OPA / Rego** | Cluster-wide authorization policy | A whole policy language and runtime (`regorus` in Rust). Far past what an incident valve needs. |
| **Envoy Unified Matcher API** | Envoy xDS | A protobuf matcher *tree*, not a text language. Useful as a shape reference (predicate tree over typed inputs); nothing to adopt directly. |
| **JsonLogic** | Assorted rules engines | JSON-encoded predicate trees; trivial to build a form UI over and to serialize. Not really a standard, and painful to write by hand. |
| **A boolean SQL expression** | — | Not a "matcher standard", but a standard *language* — already this product's stable interface, already parsed *and evaluated* by an engine in the process, and already what an admin reading the audit log thinks in. |

**This plan uses a boolean SQL expression**, parsed *and evaluated* by DataFusion, with a
pre-filter in front so that the common case never reaches it (next subsection). No new language for
the admin to learn, no grammar to specify, and no evaluation semantics to reimplement. CEL becomes
the better answer the day the matcher has to be authored by non-admins, or evaluated somewhere with
no DataFusion — and that is the trigger to revisit this.

#### The match context

Every rule is a predicate over one fixed, documented schema (`match_schema()`, a `DFSchema` of
nullable `Utf8` fields) — the attributes `execute_query` has already resolved by the time the check
runs:

| Column | NULL when |
|---|---|
| `user`, `email` | never |
| `service_account` | the caller is not a service account |
| `client`, `agent`, `entrypoint` | never (`'unknown'` when the header is absent) |
| `session`, `notebook`, `cell` | the caller sent no such header |
| `client_ip` | never |
| `sql` | never — the raw statement text |
| `sql_hash` | never — the normalized fingerprint (§2) |

Every attribute is a string, so there are no coercion surprises. Adding an attribute later means
appending one field to this schema: existing expressions keep compiling, and no migration is
involved. That is the point of the single-column design.

NULL semantics come from SQL and are the ones we want: `notebook = 'fleet-overview'` evaluates to
NULL — not true — for a query that carried no notebook header, so the rule does not fire.

#### Measured: what evaluation actually costs

DataFusion is the evaluator. It owns SQL semantics — three-valued logic, `LIKE`, coercion, regex —
and none of that gets reimplemented here. The open question was whether it is fast enough on the
front door, so it was measured rather than argued about: release build, one rule set evaluated
against one query's attribution (12 Utf8 columns), on the development machine.

| | ns/query |
|---|---|
| Build the one-row `RecordBatch` (12 string columns), nothing else | **2 804** |
| Evaluate 1 rule against a pre-built batch | 309 |
| Build batch + evaluate 1 rule | **3 384** |
| Build batch + evaluate 10 rules | 6 230 |
| Build batch + evaluate 10 rules OR-folded into a single expression | 7 338 |
| Build batch + evaluate 100 rules | 44 983 |
| Build batch + evaluate 100 rules OR-folded into a single expression | 45 373 |
| Floor: the same 10 predicates as direct `&str` comparisons | 29 |
| Floor: the same 100 predicates as direct `&str` comparisons | 287 |

Three things fall out of this, two of them counter to what the plan previously assumed:

1. **Entering Arrow at all costs ~2.8 µs** — building the one-row batch dominates everything else at
   realistic rule counts. The marginal cost of a rule is only ~400 ns.
2. **OR-folding every rule into one expression does not help** (7.3 µs vs 6.2 µs at ten rules; a
   wash at a hundred). DataFusion evaluates each disjunct as its own array operation with its own
   allocation, so collapsing N expressions into one changes nothing. This plan previously proposed
   that fold as the key optimization; the measurement retired it.
3. Straight-line string comparison is ~200× faster, which is what makes the pre-filter below
   worth having — not because 3.4 µs is unaffordable on a millisecond-scale query, but because it
   is pure waste on the overwhelming majority of queries, which match nothing.

#### Pre-processing: an equality pre-filter, so the common case never reaches Arrow

Rules are few and change rarely, so everything expensive happens at refresh:

1. `ctx.parse_sql_expr(match_expr, &match_schema())` — parse and resolve against the match context.
2. **Validate** by walking the resulting `Expr` (a check, not an evaluator): reject subqueries,
   aggregates, and window functions (`Expr::Exists` / `InSubquery` / `ScalarSubquery` /
   `AggregateFunction` / `WindowFunction`); reject any scalar function whose `Volatility` is not
   `Immutable`, which is what keeps `now()` and `random()` out so a rule means the same thing on
   every replica; require a non-`Boolean`-free result type and at least one column reference.
3. Simplify via `ExprSimplifier` (constant folding, boolean simplification) — free, once.
4. `ctx.create_physical_expr(...)` → the `Arc<dyn PhysicalExpr>` kept on the rule.
5. **Extract a required equality.** Walk the top-level `AND` chain for a
   `Column = Literal(Utf8)` conjunct. A rule containing one *cannot* match a query whose attribution
   disagrees on that field, so the rule can be indexed by it. Rules with no such conjunct — a
   top-level `OR`, or nothing but `LIKE`/regex predicates — go on a small always-evaluate list.

The refresh then builds, alongside the rule vector:

```rust
/// (field, value) -> rules whose required equality that satisfies. Only fields some rule
/// actually constrains are probed, so a single rule keyed on `sql_hash` costs one lookup.
index: HashMap<(FieldIdx, Box<str>), Vec<RuleIdx>>,
indexed_fields: Vec<FieldIdx>,   // usually one or two
always_evaluate: Vec<RuleIdx>,   // rules with no indexable equality
```

Per query, `check` then does:

- empty snapshot → return (the steady state of a healthy deployment: zero cost);
- one hash lookup per *constrained* field — typically one or two, not twelve — to collect candidate
  rules, plus the always-evaluate list;
- no candidates → return. **No `RecordBatch` is built and DataFusion is never entered**;
- candidates → build the batch once, evaluate their `PhysicalExpr`s in rule order, first `true`
  wins.

So the realistic incident steady state — a rule keyed on the offender's `sql_hash`, and every other
client's query missing it — costs one hash lookup, ~25 ns. The offender's own query pays the 3.4 µs,
and is about to be rejected instead of spending milliseconds planning, so that cost is noise.

**The index can only skip, never deny.** DataFusion always makes the final call on a candidate; a
bug in the extractor can at worst fail to enforce a rule, never invent an enforcement. It is kept
deliberately dumb — one recognized shape, everything else falls through to always-evaluate — and a
property test asserts that filtered evaluation and brute-force evaluation of every rule agree over
a corpus of expressions and attributions.

#### Compiling the rules to native code — considered, not adopted

"The deny list is short and changes rarely" is exactly the premise that justifies a compiler, so the
compile-it-once options were costed rather than waved off.

**Cranelift (`cranelift-jit`).** The strongest version of this idea is not per-rule compilation but
compiling the *entire rule set* into one native function: every literal an immediate, every string
length known at compile time, no loop over rules, no dispatch, early-out branches laid out in rule
order. That plausibly lands at ~5 ns for a whole miss — at or below the 29 ns hand-written floor
measured above — and the ~1 ms it costs to compile is irrelevant when rules change a few times a
month. Cranelift is a real, maintained Rust backend (it is what Wasmtime generates code with), and
expression JIT is a proven technique in query engines: Postgres does it with LLVM, and HyPer/Umbra
built their reputation on it.

The reason it does not apply here is the row count. Expression JIT pays off when the compiled code
runs over *millions of rows*, so that per-row interpreter dispatch dominates and compilation
amortizes across the scan. This predicate runs over **exactly one row per query**. There is no inner
loop to amortize against — the compiled function is entered once and returns.

What it would cost:

- **A custom compiler, which is more custom machinery than the interpreter that was already
  rejected**, not less. Cranelift IR has no string type; `sql_hash = 'X'` becomes a length compare
  plus a `memcmp` call the code generator has to emit, and `LIKE`/regex/three-valued logic have to
  be either generated or called out to. Emitting SSA and managing blocks is strictly more code than
  a tree-walk, and it carries the same semantic-correctness burden in a form that is far harder to
  test.
- **Executable memory in the service process.** JIT pages need `PROT_EXEC`; hardened container
  runtimes and seccomp profiles restrict that, and "the observability server now mmaps executable
  memory" is a security-review conversation in most deployments. That is a steep price for a
  microsecond.
- **A large dependency** (`cranelift-codegen`/`-frontend`/`-module`/`-jit`) in compile time and
  binary size.
- **Miscompiles present as a wrongly allowed or wrongly denied query** with no stack to inspect.

**Wasmtime.** Same generation problem — something must still compile SQL into WASM bytes, and that
compiler is ours to write — plus a boundary that does not exist in the Cranelift case: the SQL text
and every attribute must be copied into guest linear memory on each call, which puts the floor well
above native. Wasmtime's actual value proposition is *sandboxing untrusted code*, and there is no
untrusted code here: rules come from admins, and whatever executes was generated by us. Note also
that the repo's existing WASM work is not a starting point — `datafusion-wasm` is DataFusion
compiled **to** `wasm32` to run in the browser, the opposite direction from embedding a host runtime
in the server; no `wasmtime`/`wasmi` dependency exists anywhere in the workspace today. Wasmtime
becomes the right answer the day admins supply *scripts* instead of expressions, where sandboxing
is the whole point.

**A hand-written interpreter.** ~200× faster than DataFusion and what an earlier version of this
plan proposed, but it means owning SQL three-valued logic, `LIKE` pattern semantics, and regex
handling — the things most often subtly wrong in a hand-rolled implementation, in code that decides
whether a query runs.

**Why none of them are needed.** The pre-filter already removes the cost from the common path: a
query that matches nothing pays one hash probe and never enters DataFusion. What remains is a
query that is *about to be denied*, where 3.4 µs replaces milliseconds of planning. There is no hot
path left for a JIT to accelerate — which is the honest reason to skip it, rather than any claim
that it would not be fast.

**What would flip this.** A deny list in the thousands of rules, dominated by unindexable shapes
(regex- and `LIKE`-heavy, no leading equality), would put real work back on every query. The first
answer then is not a JIT but `regex::RegexSet`, which matches N patterns in a single linear pass;
compiling the rule set to native code is the step after that.

#### Grammar

Because DataFusion evaluates, the accepted language is "any DataFusion boolean expression over the
match context", minus what validation rejects above. In practice that is `AND`/`OR`/`NOT`,
`=`/`!=`, `IN`, `LIKE`/`ILIKE`, `IS [NOT] NULL`, `regexp_like`, and the built-in string functions —
without any of it having to be specified, implemented, or kept in sync here.

Regex is safe and comes free: DataFusion's `regexp_like` is backed by the Rust `regex` crate, which
does not backtrack and guarantees linear-time matching. (This corrects an earlier version of this
plan, which excluded regex on ReDoS grounds — a hazard of backtracking engines, not of this one.)

Examples, all valid:

```sql
sql_hash = '9f2c41ab73de0155' AND entrypoint = 'grafana-alert'
service_account = 'dashboards-svc' AND notebook = 'fleet-overview'
client_ip = '10.4.9.221' AND sql LIKE '%thread_spans%'
client = 'grafana' AND regexp_like(sql, '(?i)from\s+view_instance')
email = 'jean@example.com' AND (notebook IS NOT NULL OR entrypoint = 'notebook')
```

All five are indexable: each has a top-level `AND` conjunct of the form `col = 'literal'`, so each
is pruned by a single hash probe. What is *not* indexable is a rule whose top level is a
disjunction — `sql LIKE '%thread_spans%' OR client_ip = '10.4.9.221'` — which lands in
`always_evaluate` and is evaluated by DataFusion on every query. That list is expected to stay
short, and the rule cap bounds it.

A `criterion` bench (`rust/analytics/benches/query_deny_match.rs`, alongside the existing
`property_get`/`parse_block` benches) pins `check` at 0, 1, 10, and 100 rules for both the
pre-filtered miss and the matching hit, so the numbers above stay true instead of decaying into
folklore.

#### Where this can go next

The column holds text and the expression language is DataFusion's, so richer semantics land without
a migration and mostly without code: numeric comparison the moment the match context carries a
numeric attribute, a structured `deny_queries` variant that renders an expression for a UI builder,
cost predicates once an estimate is available at check time, or a `test_query_denial(expr)` function
that dry-runs an expression against recent audit records before it goes live. The pre-filter is an
independent axis: if unindexable rules ever dominate, `regex::RegexSet` over the residual set is the
next step, and compiling the rule set to native code the one after that.

### 4. Rule model, evaluation, and cache

```rust
pub struct QueryAttribution<'a> {      // borrowed view of what execute_query already resolved
    pub user: &'a str, pub email: &'a str,
    pub service_account: Option<&'a str>,
    pub client: &'a str, pub agent: &'a str, pub entrypoint: &'a str,
    pub session: Option<&'a str>, pub notebook: Option<&'a str>, pub cell: Option<&'a str>,
    pub client_ip: &'a str,
    pub sql: &'a str, pub sql_hash: &'a str,
}

impl QueryAttribution<'_> {
    /// Borrowed field by match-context index, for the pre-filter's hash probes.
    fn field(&self, idx: FieldIdx) -> Option<&str>;
    /// One-row RecordBatch matching `match_schema()`. Built only when the pre-filter
    /// leaves at least one candidate rule -- ~2.8 us, so never on a miss.
    fn to_batch(&self) -> Result<RecordBatch>;
}

pub struct QueryDenyRule {
    pub rule_id: Uuid, pub created_at: DateTime<Utc>, pub created_by: String,
    pub reason: String, pub match_expr: String,
    compiled: Arc<dyn PhysicalExpr>,      // parsed/validated/simplified at refresh
    required_eq: Option<(FieldIdx, Box<str>)>,  // the conjunct this rule is indexed by
    hits: AtomicU64,                      // in-process delta since the last flush
    last_hit: AtomicI64,                  // unix seconds, 0 = not hit since the last flush
}

/// Rule vector plus the pre-filter built from it (§3). Rebuilt wholesale on every refresh,
/// which is cheap because rules are few and change rarely.
pub struct DenySnapshot {
    rules: Vec<Arc<QueryDenyRule>>,
    index: HashMap<(FieldIdx, Box<str>), Vec<RuleIdx>>,
    indexed_fields: Vec<FieldIdx>,
    always_evaluate: Vec<RuleIdx>,
}

pub struct QueryDenyList {               // owned by LakehouseContext, like AudienceIndex
    pool: sqlx::Pool<sqlx::Postgres>,
    snapshot: std::sync::RwLock<Arc<DenySnapshot>>,  // `arc-swap` is not a workspace dep;
                                   // a read lock held for one clone is enough
}

impl QueryDenyList {
    pub fn check(&self, q: &QueryAttribution<'_>) -> Option<Arc<QueryDenyRule>>;
    pub async fn refresh(&self) -> Result<()>;     // flush hit deltas, reload + recompile
    pub async fn insert(&self, match_expr: &str, reason: &str, created_by: &str)
        -> Result<QueryDenyRule>;
    pub async fn delete(&self, rule_id: Uuid) -> Result<bool>;
    pub async fn list(&self) -> Result<Vec<QueryDenyRule>>;
    pub fn spawn_refresh_task(self: Arc<Self>, shutdown: impl Future<Output = ()> + Send + 'static);
}
```

- **Zero rules cost nothing.** `check` returns on an empty-snapshot test — the steady state of
  every deployment that is not mid-incident.
- With rules present, `check` runs the pre-filter first (§3): one hash probe per constrained field,
  ~25 ns for the usual single-field rule set. On a miss it returns without building a `RecordBatch`
  or entering DataFusion at all. On a hit it builds the batch once and evaluates the candidates'
  `PhysicalExpr`s in rule order, first `true` winning — ~3.4 µs, paid only by a query that is about
  to be rejected instead of planned.
- `refresh` runs every `MICROMEGAS_QUERY_DENY_REFRESH_SECONDS` (default 10): it flushes each rule's
  accumulated `hits`/`last_hit` (`UPDATE query_deny_list SET hit_count = hit_count + $1,
  last_hit_at = greatest(coalesce(last_hit_at, $2), $2) WHERE rule_id = $3`, skipping rules with a
  zero delta), then reloads the table and recompiles each `match_expr`. Batching keeps a denied
  4-QPS offender at one write per tick instead of one per rejection, and `last_hit_at` is therefore
  accurate to within one tick — which is all "is this rule still firing?" needs.
- **A rule that fails to compile is skipped, never fatal.** It is dropped from the snapshot with a
  `warn!` naming the rule id and the compile error, plus
  `imetric!("query_deny_compile_error_count", ...)`. This is what makes it safe to extend the match
  context later: an older replica that cannot compile a newer rule declines to enforce it rather
  than denying everything or crashing.
- **Fail-open by design.** A failed refresh keeps the previous snapshot and emits
  `imetric!("query_deny_refresh_error_count", ...)` + a `warn!`. A failed *initial* load starts with
  an empty snapshot. The deny list is an availability valve, not a security control — failing closed
  would deny every query on a DB blip.
- The refresh task is spawned only by the FlightSQL server builder
  (`flight_sql_server.rs::build_and_serve`, which the monolith also uses). Other `LakehouseContext`
  holders (maintenance daemon, tests) keep an empty snapshot and never deny anything.
- `insert`/`delete` refresh the local snapshot synchronously before returning, so the admin who
  created a rule sees it in their own `list_query_denials()` immediately; other replicas pick it up
  within one tick.

### 5. The check in `execute_query`

Inserted immediately after `QueryAuditState` is constructed (`flight_sql_service_impl.rs:718`) and
before `scoped_runtime`/`caller_context`/`make_session_context`:

```rust
let denied = self.lakehouse.query_denials().check(&QueryAttribution { .. });
if let Some(rule) = denied
    && !skip_for_admin_recovery(caller_is_admin, &sql_tokens)
{
    let status = Status::resource_exhausted(format!(
        "query denied by rule {} (reason: {}); ask an admin to lift it with \
         remove_query_denial('{}'); query_id={query_id}",
        rule.rule_id, rule.reason, rule.rule_id));
    warn!(
        "query denied rule_id={} reason={:?} sql_hash={sql_hash} user={} email={} \
         client={client_type} entrypoint={client_entrypoint} client_ip={client_ip} \
         query_id={query_id}",
        rule.rule_id, rule.reason, attr.user_id, attr.user_email);
    imetric!("query_denied", "count", rule_tags(&rule.rule_id), 1_u64);
    rule.record_hit();
    return Err(audit_state.fail_with_class(status, "denied"));
}
```

- **Status code**: `ResourceExhausted`. It is the only existing code whose `error_class` bucket
  (`"resource"`) already means "the service refused to spend resources on this", and it keeps the
  rejection out of the `query_failed`/`error!` internal-error path. The message is the
  distinguishing part: it names the rule id and the reason, and — since a rule has no expiry to
  wait out — tells the caller exactly what an admin has to run to lift it.
- **Warning log (§6)**: every denial emits a `warn!` line, so a denied query is visible on any
  dashboard already watching warning-level logs — not only to someone who thought to query the
  audit target. This is the one new log point on the deny path; without it the rejection would be
  silent at log level, since the deny site builds its `Status` directly and never passes through
  `error_or_warn_log` (which only ever sees a `DataFusionError`).
- **Audit record**: emitted with `status: "error"` and a dedicated `error_class: "denied"` (a
  fourth value alongside `user`/`resource`/`internal`), via a new
  `QueryAuditState::fail_with_class(status, class)` — `fail()` becomes a thin wrapper that derives
  the class from the code. Denied traffic stays fully visible in the audit log, which is how an
  operator confirms the offender actually backed off.
- **Anti-jam escape hatch.** A rule keyed on identity alone (e.g. `client_ip`) could match the
  admin's own recovery query and lock the valve shut — and with no expiry, nothing lifts it on its
  own. So the check is skipped when the caller is admin **and** the statement references one of
  `deny_queries` / `remove_query_denial` / `list_query_denials` as an *identifier token* (checked
  over the token stream already produced for the fingerprint — not a substring match on the raw
  text). A non-admin cannot exploit this: those functions are not registered for them, and the skip
  requires `is_admin`. This is the primary recovery path, so it carries its own test.
- **Prepared statements**: the same check is applied at the top of
  `do_action_create_prepared_statement` (which plans, and is therefore worth protecting from a
  prepare loop). That path does not resolve attribution today; it will call
  `validate_and_resolve_user_attribution_grpc` for this. No audit record exists on that RPC, so the
  rejection is logged and counted but not audited.

### 6. Making a denial visible

Three signals, deliberately at three different volumes:

**`warn!` per denial → `log_entries`.** Level `Warn` (3), under the
`micromegas::servers::flight_sql_service_impl` target, carrying the rule id, the reason, the
`sql_hash`, the caller (user/email/client/entrypoint/client_ip) and the `query_id`. Any panel that
already charts or lists warnings picks this up with no new wiring:

```sql
SELECT time, msg
FROM log_entries
WHERE level <= 3          -- Fatal, Error, Warn
  AND msg LIKE 'query denied%'
  AND time >= NOW() - INTERVAL '1 hour'
ORDER BY time DESC;
```

(A deployment watching `log_stats` instead sees the denials as a `Warn` rise on the
`micromegas::servers::flight_sql_service_impl` target — cheaper, but not denial-specific: that
target also carries the client-error warnings `error_or_warn_log` emits.)

**`imetric!("query_denied", "count", {rule_id}, 1)` → `measures`.** Tagged with the rule id (the
per-rule counter the issue asks for), which is why it is a tagged metric rather than an untagged
one: cardinality is bounded by `MICROMEGAS_QUERY_DENY_MAX_RULES` (100), well inside what a
`PropertySet` should carry. This is the rate signal a dashboard graphs:

```sql
SELECT date_bin(INTERVAL '1 minute', time) AS minute,
       property_get(properties, 'rule_id')  AS rule_id,
       sum(value)                           AS denied
FROM measures
WHERE name = 'query_denied'
  AND time >= NOW() - INTERVAL '6 hours'
GROUP BY minute, rule_id
ORDER BY minute;
```

**The audit record** (`error_class = 'denied'`) stays the full-detail row: SQL text, fingerprint,
full attribution. It is what an operator drills into once a panel says something is being denied.

**Volume.** A denied 4-QPS dashboard produces ~4 warnings/second for as long as its rule stands,
and rules no longer expire. That is the intended behavior — a standing denial *should* keep saying
so — but a deployment that finds a long-lived rule too chatty can set
`MICROMEGAS_QUERY_DENY_WARN_WINDOW_SECONDS` (default `0` = warn on every denial) to throttle the
line to at most once per rule per window, using the same checked-and-set `AtomicI64` pattern as
`db_api_key.rs::maybe_log_error`. The metric and the audit record are never throttled, so the exact
count survives regardless of what the log does.

### 7. `CallerContext` gains the caller's identity

`deny_queries` must record `created_by`, and the UDTF only ever sees `CallerContext`. Add:

```rust
pub struct CallerIdentity { pub user: String, pub email: String, pub service_account: Option<String> }

pub struct CallerContext {
    // ... existing fields ...
    pub identity: Option<CallerIdentity>,   // None on internal/maintenance paths
}
```

`FlightSqlServiceImpl::caller_context` takes the already-resolved attribution and populates it.
This is a **minor breaking change** to a `pub` Rust struct (CHANGELOG entry required); the compiler
enumerates every construction site, which is the intended failure mode.

`QueryAuditRecord` gains `pub sql_hash: String` — appended **last**, so existing JSON consumers are
unaffected. The doc comment on `error_class` is updated to enumerate `"denied"`.

### 8. Admin SQL surface

Registered inside the existing `caller.is_admin || !caller.admin_principal_possible` block in
`register_lakehouse_functions`.

**`list_query_denials()`** — UDTF, no args. Returns every rule currently in force:

| Column | Type | Notes |
|---|---|---|
| `rule_id` | Utf8 | |
| `created_at` | Timestamp(ns, UTC) | |
| `created_by` | Utf8 | |
| `reason` | Utf8 | |
| `match_expr` | Utf8 | the expression as written |
| `hit_count` | Int64 | last flushed value |
| `last_hit_at` | Timestamp(ns, UTC), nullable | NULL until the rule first fires |

**`deny_queries(match_expr, reason)`** — UDTF returning a single row (`rule_id`). The expression is
a boolean SQL predicate over the match context (§3); inner quotes are doubled, as anywhere else in
SQL:

```sql
SELECT * FROM deny_queries(
  'sql_hash = ''9f2c41ab73de0155'' AND entrypoint = ''grafana-alert''',
  'alert rule re-firing on failure; owner notified');
```

Validation, all fail-loud with `plan_err!`/a returned error: any `compile_match_expr` failure (§3),
reported with the offending token where the parser gives one; an expression with no column
reference; an empty `reason`; rule count already at `MICROMEGAS_QUERY_DENY_MAX_RULES`.

**`remove_query_denial(rule_id)`** — scalar UDF returning a status string; deletes the row (returns
a clear "no such rule" message when it matched nothing). The audit log is the durable record of
what was denied and what it rejected, so the row itself does not need to survive its removal.

### 9. Web app — Admin → Query Deny List

The screen drives the **same SQL functions** through the existing `useStreamQuery` →
`/api/stream-query` path, against the data source the admin selects. No new REST routes and no
second copy of the rule store — which also means the screen manages the deny list of the
deployment it is pointed at, instead of whatever DB `analytics-web-srv` happens to hold (the
API-key pages' single-DB assumption would be wrong here).

Layout — a single rules table plus a create dialog (`tasks/query_deny_list_mockups/query-deny-list-screen.html`):

- **Rules table** — `SELECT * FROM list_query_denials()`: the match expression in monospace, reason,
  creator, created-at, hit count, **last hit** (relative — "4 s ago" reads as "still firing", "3
  weeks ago" as "probably removable"), **Remove** (via `ConfirmDialog`). Empty state points at the
  audit-log doc for finding an offender's fingerprint.
- **Deny a Query dialog** — an expression textarea plus the required reason, with insert-chips for
  the common predicates and a link to the match-context reference. A textarea rather than a field
  grid: the expression *is* the rule, and a grid can only ever express the AND-of-equalities
  subset. A compile error from the server is shown inline against the expression, not as a
  page-level banner.

The dialog issues `SELECT * FROM deny_queries('<expr>', '<reason>')`. **SQL literal escaping**: both
the expression and the reason are user-supplied and must go through a single `escapeSqlLiteral`
helper (`'` → `''`), applied at the one place that builds these statements — the same rule
`substitute_macros` already follows server-side. The expression itself already contains doubled
quotes by the time it reaches that helper, so the round trip is worth a dedicated test.

Non-admins never reach the page (`AuthGuard requireAdmin`), and a non-admin who hand-typed the SQL
would get "function not found" from `flight-sql-srv` — the gate is server-side, the guard is UX.

`BLOCKED_FUNCTIONS` in `stream_query.rs` is deliberately **not** extended: these three functions are
admin-gated at `flight-sql-srv` and are precisely what the web screen needs to call.

### 10. Configuration

| Env var | Default | Meaning |
|---|---|---|
| `MICROMEGAS_QUERY_DENY_REFRESH_SECONDS` | 10 | Snapshot refresh / hit-count flush interval; also the bound on cross-replica propagation |
| `MICROMEGAS_QUERY_DENY_MAX_RULES` | 100 | Cap on rules in force at once (bounds per-query cost) |
| `MICROMEGAS_QUERY_DENY_WARN_WINDOW_SECONDS` | 0 | `0` warns on every denial; `>0` throttles the `warn!` line to once per rule per window (metric/audit unaffected) |

## Mockups

- `tasks/query_deny_list_mockups/query-deny-list-screen.html` — the rules table plus the "Deny a Query"
  dialog. The screen is purely a front end for the three SQL functions; the admin copies the
  fingerprint over from the audit log by hand.

An "incident console" variant was considered and dropped (a top-query-load panel driven by the
audit log, with a per-row *Deny…* button that prefills the dialog, plus a rejected-queries panel).
It can be layered onto the same page later without reworking the table or the dialog — see the
Open Questions.

## Implementation Steps

### Phase 1 — Store and matching (analytics crate, no wiring)

1. `rust/analytics/src/lakehouse/migration.rs`: `upgrade_v8_to_v9` creating `query_deny_list`;
   `LATEST_LAKEHOUSE_SCHEMA_VERSION = 9`.
2. New `rust/analytics/src/lakehouse/query_deny_list.rs`: `sql_fingerprint`, the match context
   schema, `compile_match_expr` (parse → validate → simplify → `create_physical_expr` → extract the
   required equality), `QueryAttribution`, `QueryDenyRule`, `DenySnapshot` (rules + pre-filter
   index), `QueryDenyList` (`check` / `refresh` / `insert` / `delete` / `list` /
   `spawn_refresh_task`), env knobs. Add `sha2` to `analytics/Cargo.toml`.
   Register in `rust/analytics/src/lakehouse/mod.rs`. Add `sha2` to `analytics/Cargo.toml`.
3. Unit tests for `sql_fingerprint` and `matches` (see Testing Strategy).

### Phase 2 — Wiring and enforcement

4. `lakehouse_context.rs`: construct and expose `query_denials()` (mirrors `audience_index()`).
5. `read_scope.rs`: add `CallerIdentity` + `CallerContext::identity`; fix every construction site
   the compiler flags.
6. `flight_sql_service_impl.rs`: compute the fingerprint once; add `sql_hash` to `QueryAuditState`
   and `QueryAuditRecord`; add `fail_with_class`; insert the deny-list check after `QueryAuditState`
   is built, with its `warn!` line and the rule-tagged `query_denied` metric (§6); populate
   `CallerContext::identity` in `caller_context`; add the check + attribution resolution to
   `do_action_create_prepared_statement`.
7. `flight_sql_server.rs`: spawn the refresh task with the existing shutdown fanout.

### Phase 3 — Admin SQL functions

8. `list_query_denials_table_function.rs` (pattern: `list_partitions_table_function.rs`).
9. `deny_queries_table_function.rs` — validates, inserts, refreshes the local snapshot, returns
   one row.
10. `remove_query_denial_udf.rs` (pattern: `retire_partition_by_file_udf.rs`).
11. Register all three in `query.rs`'s admin block.

### Phase 4 — Web app screen

12. `analytics-web-app/src/lib/query-deny-list-api.ts` — SQL builders (`escapeSqlLiteral`, the
    three statements) and Arrow→row decoding.
13. `analytics-web-app/src/routes/QueryDenyListPage.tsx` — the page, reusing
    `PageLayout` / `AuthGuard requireAdmin` / `ErrorBanner` / `ConfirmDialog` / `Button` /
    `DataSourceField`.
14. Register the route (`/admin/query-deny-list`) in `router.tsx` and add the card to
    `AdminPage.tsx` (lucide `ShieldBan` or `Ban` icon).
15. Vitest coverage for the SQL builders (escaping in particular) and a page render test, matching
    `routes/__tests__/AnalyticsApiKeysPage.test.tsx`.

### Phase 5 — Docs and changelog

16. `mkdocs/docs/admin/functions-reference.md`: the three functions, with the incident runbook.
17. `mkdocs/docs/query-guide/query-audit-log.md`: document `sql_hash` and `error_class = "denied"`,
    plus the "find the offender, copy its fingerprint" query.
18. `mkdocs/docs/admin/flight-sql.md`: the two env knobs, propagation delay, fail-open behavior.
19. `mkdocs/docs/admin/web-app.md`: the new admin screen.
20. `CHANGELOG.md`: feature entry + **Minor breaking change** clause for `CallerContext`.

## Files to Modify

**Create**

- `rust/analytics/src/lakehouse/query_deny_list.rs`
- `rust/analytics/src/lakehouse/list_query_denials_table_function.rs`
- `rust/analytics/src/lakehouse/deny_queries_table_function.rs`
- `rust/analytics/src/lakehouse/remove_query_denial_udf.rs`
- `rust/analytics/benches/query_deny_match.rs` (criterion, per-query `check` cost)
- `rust/analytics/tests/query_deny_list_tests.rs` (unit)
- `rust/analytics/tests/query_deny_list_db_test.rs` (DB-backed, `#[ignore]`d)
- `analytics-web-app/src/lib/query-deny-list-api.ts`
- `analytics-web-app/src/routes/QueryDenyListPage.tsx`
- `analytics-web-app/src/lib/__tests__/query-deny-list-api.test.ts`
- `analytics-web-app/src/routes/__tests__/QueryDenyListPage.test.tsx`
- `python/micromegas/tests/test_query_deny_list.py`

**Modify**

- `rust/analytics/src/lakehouse/migration.rs`, `mod.rs`, `query.rs`, `read_scope.rs`,
  `lakehouse_context.rs`
- `rust/analytics/Cargo.toml` (`sha2`)
- `rust/public/src/servers/flight_sql_service_impl.rs`, `query_audit.rs`, `flight_sql_server.rs`
- `analytics-web-app/src/router.tsx`, `src/routes/AdminPage.tsx`
- `mkdocs/docs/admin/functions-reference.md`, `admin/flight-sql.md`, `admin/web-app.md`,
  `query-guide/query-audit-log.md`
- `CHANGELOG.md`

## Trade-offs

**SQL functions as the single admin surface, with the web screen driving them.** The alternative —
REST routes on `analytics-web-srv` hitting Postgres directly, mirroring `analytics_keys.rs` — would
duplicate the rule store, its validation, and its error shape, and would target whichever DB
`analytics-web-srv` is configured with rather than the data source the admin selected. The SQL path
also gives CLI and notebook admins the same capability for free.

**Rules stored in Postgres and polled, not pushed.** Postgres is already the coordination point for
every replica and needs no new infrastructure. The cost is up to one refresh interval (10 s) of
propagation delay — irrelevant against an incident measured in minutes, and the inserting replica
applies its own rule immediately.

**`ResourceExhausted` rather than a new code or `PermissionDenied`.** `PermissionDenied` lands in
the `"internal"` `error_class` bucket, which would fire `query_failed` and `error!` logs for what is
a deliberate, expected rejection. `ResourceExhausted` already means "refused to spend resources
here"; the rule id, the reason, and the command that lifts it carry the distinguishing detail.

**One expression column, not a column per matcher.** A fixed matcher schema fossilizes the matching
language: every new attribute is a migration, and AND-of-equalities is the only combinator it can
ever express. A text column plus a compiler moves that evolution out of the schema entirely. The
cost is that the stored form is no longer directly queryable in SQL (`WHERE match_user = 'bob'` over
the rules table is gone) — which the rules table's size, and the fact that nobody queries it
programmatically, makes a non-issue.

**DataFusion evaluates; a pre-filter keeps it off the common path.** Single-row evaluation is the
shape a columnar engine is worst at — measured at 3.4 µs, of which 2.8 µs is just constructing the
one-row batch (§3). The alternatives that beat it (a hand-written interpreter, a Cranelift JIT, a
WASM runtime) all require owning SQL semantics or a code generator, in the component that decides
whether a query runs. Indexing rules by a required equality instead means a non-matching query pays
one hash probe and never enters DataFusion, so the engine's cost is paid only by queries that are
about to be rejected. The price is a small extractor whose worst failure is a rule that fails to
fire — never one that fires wrongly — held down by a property test against brute-force
evaluation.

**Regex is in, and the earlier ReDoS objection was wrong.** An earlier version of this plan excluded
regex because "caller-influenced regexes on a hot path invite ReDoS". That reasoning applies to
backtracking engines; Rust's `regex` crate does not backtrack and guarantees linear time, and
patterns here are compiled once per rule rather than per query. Regex costs nothing to allow, so it
is allowed.

**Fail-open on store errors.** A deny list that fails closed turns a Postgres blip into a total
outage — strictly worse than the problem it exists to solve. This is an availability valve, not an
authorization control; the authorization controls (`ReadScope`, `AudienceGuard`, `OwnershipRewrite`)
fail closed and are unaffected.

**No expiry: rules stand until removed.** The alternative — a mandatory, capped TTL — bounds the
damage of a forgotten rule automatically, but it also means a rule can silently lapse mid-incident
and let the offender back in while nobody is looking, and it forces an operator to guess a duration
up front. Standing rules keep the state of the world explicit: what is denied is exactly what
`list_query_denials()` shows. The cost is that a forgotten rule stays forgotten, which is mitigated
by three things — the mandatory `reason` and recorded `created_by`, the rejection message telling
the caller precisely what to ask for, and `hit_count`/`last_hit_at`, which separate a rule still
rejecting traffic from one that has not fired in weeks and is safe to remove.

**Hard delete rather than soft delete.** `analytics_api_keys` keeps revoked rows with
`revoked_at`/`revoked_by`; the deny list does not, because the audit log already records every
denial the rule ever caused, which is the part worth keeping. If a "who removed this rule and when"
trail turns out to be wanted, a `removed_at`/`removed_by` pair plus a `WHERE removed_at IS NULL`
filter in the refresh query is an additive change.

**64-bit fingerprint.** Short enough to read off a log line and paste into a terminal; collisions
are astronomically unlikely and bounded in blast radius by the rule's other predicates.

## Documentation

- `mkdocs/docs/admin/functions-reference.md` — reference for the three functions, the **match
  context** columns and the **expression language** (§3) with worked examples, plus an "incident
  runbook" section: find the offender in the audit log → copy `sql_hash` → `deny_queries` → confirm
  rejections → `remove_query_denial` once the offending client is fixed.
- `mkdocs/docs/query-guide/query-audit-log.md` — the new `sql_hash` field, `error_class = "denied"`,
  and the top-offenders query an operator runs to find the fingerprint.
- `mkdocs/docs/admin/flight-sql.md` — env knobs, propagation delay, fail-open behavior, the admin
  escape hatch, and a **"Watching for denials"** section carrying both dashboard queries from §6
  (the warning-level `log_entries` panel and the per-rule `query_denied` rate panel) so an operator
  can paste them straight into a dashboard.
- `mkdocs/docs/admin/web-app.md` — the Admin → Query Deny List screen.

## Testing Strategy

**Unit (`rust/analytics/tests/query_deny_list_tests.rs`, no DB)**

- `sql_fingerprint`: two dashboard refreshes differing only in timestamp/limit literals produce the
  same fingerprint; different column lists produce different ones; whitespace/comment/case changes
  are absorbed; unparseable SQL still yields a fingerprint.
- `compile_match_expr` rejects, each with a distinct message: unknown column, unknown function,
  non-boolean result, subquery, aggregate, window function, a non-`Immutable` function (`now()`),
  and a no-column expression (`true`, `1 = 1`).
- Required-equality extraction: recognized for a top-level `AND` chain containing
  `col = 'literal'`; *not* claimed for an `OR` at the top level, a `LIKE`, or a column-to-column
  comparison — those must land in `always_evaluate`.
- **Pre-filter equivalence (property test).** Over a corpus of expressions × attributions,
  `check` through the index returns exactly what evaluating every rule by brute force returns. This
  is the test that makes the index safe to trust.
- Three-valued logic end-to-end: `notebook = 'x'` does not deny a query that carried no notebook
  header; `notebook IS NULL` does.
- Every example expression in the docs compiles and evaluates as documented.
- `skip_for_admin_recovery`: an admin statement calling `remove_query_denial` is exempt; the same
  statement from a non-admin is not; a non-admin query that merely aliases a column
  `remove_query_denial` is not exempt.

**Integration (`rust/analytics/tests/query_deny_list_db_test.rs` — `#[ignore]`d `#[tokio::test]`
requiring a live `MICROMEGAS_SQL_CONNECTION_STRING`, `mod common;` for `db_fixtures`, same
convention as `ownership_rewrite_db_test.rs`)**

- Migration v8 → v9 applies cleanly on a pre-existing lakehouse schema.
- `insert` → `refresh` → `check` matches; `delete` → `refresh` → no longer matches.
- Hit flush: N `record_hit` calls then `refresh` leaves `hit_count = N` and a `last_hit_at` at the
  most recent of them; a rule with no hits this tick is not written at all (and keeps its earlier
  `last_hit_at`).
- Refresh failure keeps the previous snapshot (point the pool at a closed connection).
- A row whose `match_expr` does not compile (written directly with `INSERT`, simulating a newer
  version) is skipped with a warning while every other rule stays enforced.

**Rust service tests (`rust/public/tests/`)**

- `QueryAuditRecord` with `error_class: "denied"` and `sql_hash` serializes as expected
  (extend `query_audit_tests.rs`).
- The denial `warn!` line contains the rule id, `sql_hash`, and caller attribution — asserted
  against the formatted string the same way `build_log_line`'s content is asserted today, rather
  than by capturing log output.

**End-to-end (`python/micromegas/tests/test_query_deny_list.py`, against `local_test_env`)**

- `deny_queries` with a `sql_hash` predicate → the matching query fails with a `ResourceExhausted`
  naming the rule id → `remove_query_denial` → the query succeeds again.
- A non-matching query is unaffected while the rule is in force.
- Each denial lands one `Warn`-level `log_entries` row (`msg LIKE 'query denied%'`) and one
  `query_denied` measure tagged with the rule id — the two dashboard signals from §6, checked
  end-to-end rather than only at the call site.
- A no-column expression is rejected, as is a syntactically invalid one, each with a message that
  names the problem.
- `list_query_denials()` shows the rule while it stands and drops it after removal; `hit_count`
  reflects the rejections once a refresh tick has flushed.
- A rule matching *everything the test client sends* still leaves `remove_query_denial` callable —
  the escape hatch (§5), and the only recovery path now that rules do not expire.
- Note: `local_test_env` runs with auth disabled, so every caller is an admin there.

**Web app (Vitest)**

- SQL builders: a reason containing `'` is escaped exactly once, and an expression already
  containing doubled quotes survives the round trip unchanged.
- Page: renders rules, opens the deny dialog, calls the right SQL on confirm, shows the error
  banner on a failed query.

## Open Questions

1. **Quoted identifiers in the fingerprint.** `sqlparser` tokenizes `"My Column"` as a
   `DoubleQuotedString`, indistinguishable from a string literal at the token level. Replacing it
   with `?` would collapse queries that differ only in a quoted column name into one fingerprint.
   Proposed: treat `DoubleQuotedString` as an identifier (keep it verbatim) and only strip
   single-quoted/number/hex/national literals. Worth confirming against real queries before
   implementing.
2. **Where should the "find the offender" step live?** This plan leaves it in the audit-log docs:
   the admin runs the top-offenders query themselves and pastes the fingerprint into the dialog.
   The mocked-up incident-console variant folded that into the same screen, and a standalone
   "Query Load" admin screen would serve it outside an incident too ("what is the service spending
   its time on?"). Either can be built later against the same `sql_hash` field; worth deciding
   before the screen's layout hardens.
3. **`match_email` case sensitivity.** Exact match is proposed; if the deployment's OIDC provider
   ever varies email case, a case-insensitive compare for `email`/`user` specifically may be worth
   it. Deferred until observed.
