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
`execute_query`. `do_action_create_prepared_statement` (line 1235) builds a session context and
plans without executing, and `do_get_prepared_statement` (line 1046) is `api_entry_not_implemented!()`
— so every executed query reaches `execute_query`, prepared or not (§5).

### Audit record

`QueryAuditRecord` (`rust/public/src/servers/query_audit.rs:79`) already carries every field a rule
would want to match on: `client_ip`, `client`, `agent`, `entrypoint`, `session`, `notebook`, `cell`,
`user`, `email`, `service_account_name`, and the raw `sql`. It is emitted as one JSON line under the
`flightsql_query_audit` target on every terminal path — success, failure, and abandoned-mid-drain
(`QueryAuditState::emit`, `fail`, and its `Drop` impl — all in
`rust/public/src/servers/flight_sql_service_impl.rs:314,369,406`). It does **not** currently
carry a normalized-SQL fingerprint.

`error_class` is derived from the gRPC code by `error_class()` (`flight_sql_service_impl.rs:116`): `InvalidArgument` /
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
  `LATEST_LAKEHOUSE_SCHEMA_VERSION = 8`. `LakehouseContext::from_connection`/`from_env` run
  `migrate_lakehouse` at startup (`lakehouse_context.rs:45,60`), so **`flight-sql-srv` itself
  migrates on boot**.
- `AudienceIndex` (`rust/analytics/src/lakehouse/audience_guard.rs:202`) is the precedent for a
  Postgres-backed, TTL-cached lookup owned by `LakehouseContext` and reached from `query.rs` via
  `lakehouse.audience_index()`.
- Row-returning UDTF over Postgres: `list_partitions_table_function.rs`. Mutating UDTF:
  `retire_partitions_table_function.rs` (streams a log via `LogStreamTableProvider`).
- `sqlparser` 0.62 (with the `visitor` feature) is reachable as `datafusion::sql::sqlparser`;
  `sha2` is a workspace dependency; `regex` is **not** a direct dependency anywhere, so it needs its
  own `[workspace.dependencies]` entry in `rust/Cargo.toml` alongside `sha2`'s, not just an
  `analytics/Cargo.toml` line — every other non-dev dependency in that crate is declared as
  `<name>.workspace = true`, and `regex` should follow the same convention.

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
/// Tokenizes once; both the fingerprint and the anti-jam escape hatch (§5) consume the same
/// token stream instead of each re-tokenizing the statement.
pub fn tokenize(sql: &str) -> Vec<Token>;

/// Literal-stripped fingerprint of a statement: the first 16 hex chars of the
/// SHA-256 of the normalized token stream.
pub fn fingerprint_of(tokens: &[Token]) -> String;
```

`execute_query` calls `tokenize` once per query and derives both the fingerprint and the escape-hatch
check from the result; there is no standalone `sql_fingerprint(sql: &str) -> String` that discards
the tokens.

Implementation: `sqlparser::tokenizer::Tokenizer` over `GenericDialect`, then

- every `Token::Number`, `SingleQuotedString`, `NationalStringLiteral`, `HexStringLiteral`,
  `EscapedStringLiteral`, `TripleSingleQuotedString`, `TripleDoubleQuotedString` → `?`
- whitespace runs collapse to one space; comments dropped
- `Token::Word` is lowercased only when `quote_style.is_none()` — keywords and unquoted identifiers
  fold to lowercase (identifiers are already case-insensitive in DataFusion unless quoted), but a
  quoted word is kept verbatim, case included. This, not a separate `DoubleQuotedString` carve-out,
  is what protects this product's dotted column names: under `GenericDialect`, a double-quoted
  identifier like `b."processes.exe"` tokenizes as `Token::Word { quote_style: Some('"') }`, not
  `Token::DoubleQuotedString` (the tokenizer's double-quote arm is gated on
  `is_delimited_identifier_start`, which `GenericDialect` sets true for `"`). Lowercasing on
  `quote_style` rather than stripping a `DoubleQuotedString` token that never actually appears here
  is what keeps `"processes.exe"` and `"processes.username"` from collapsing into the same
  fingerprint and denying a query never meant to match.
- tokens joined with a single space, then SHA-256, hex, truncated to 16 chars (64 bits)

Tokenization failure falls back to hashing the whitespace-collapsed raw text, so a fingerprint
always exists.

This is what makes the dashboard case work: consecutive refreshes differ only in their time-range
literals, so they collapse to one fingerprint. A 64-bit fingerprint's collision odds across a
realistic query population are negligible, and the consequence of a collision is bounded by the
rule's other match predicates.

The fingerprint is computed once per query in `execute_query` and stored on `QueryAuditState`, so
it costs nothing extra to also emit it in the audit record (§7) — which is how an operator gets the
value to paste into `deny_queries`. Unlike the deny-list check itself, this cost is **not**
conditional on any rule existing: `QueryAuditState::emit` writes `sql_hash` on every terminal path
(success, failure, and abandoned-mid-drain), so `tokenize` + `fingerprint_of` run on every query
regardless of whether the deny list is empty. This fingerprint cost is measured alongside the
evaluator in §3's bench, and accounted for there rather than assumed away.

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
| **A boolean SQL expression** | — | Not a "matcher standard", but a standard *language* — already this product's stable interface, already parsed by an engine in the process, and already what an admin reading the audit log thinks in. |

**This plan uses a boolean SQL expression**, parsed by DataFusion and compiled to a small evaluator
of our own (next subsection — the split is driven by measurement). No new language for the admin to
learn and no grammar of ours to specify. CEL becomes the better answer the day the matcher has to be
authored by non-admins, or evaluated somewhere with no DataFusion to parse it — and that is the
trigger to revisit this.

#### The match context

Every rule is a predicate over one fixed, documented schema (`match_schema()`, a `DFSchema` of
nullable `Utf8` fields) — the attributes `execute_query` has already resolved by the time the check
runs:

| Column | NULL when |
|---|---|
| `user_id`, `email` | never — `user_id` is the *authenticated* OIDC subject only for a non-delegating caller under OIDC. A delegating service account (`x-allow-delegation: true` plus a client-supplied `x-user-id`/`x-user-email`) puts its own authenticated identity in `service_account` and echoes the client-asserted id into `user_id` instead; with no `x-auth-subject` at all (`--disable-auth`), `user_id`/`email` fall back to the client-supplied headers, defaulting to `'unknown'`. So a rule keyed on `user_id`/`email` targets a client-asserted value in the delegating-service-account case, and can be evaded by changing a header |
| `service_account` | the caller is a service account that is **not delegating** — i.e. also NULL for an ordinary human caller. It is set only when a service account calls on behalf of a user (`x-user-id`/`x-user-email` present alongside its own credentials); a non-delegating service account's identity is in `user_id` instead, so a rule meant to target one should match on `user_id`, not `service_account` |
| `client`, `agent`, `entrypoint` | never (`'unknown'` when the header is absent) |
| `session`, `notebook`, `cell` | the caller sent no such header |
| `client_ip` | never |
| `sql` | never — the raw statement text |
| `sql_hash` | never — the normalized fingerprint (§2) |

Server-derived, not client-influenceable: `client_ip`, `sql`, `sql_hash`, `service_account`.
Client-asserted, and therefore something an offender can change to evade a rule: `user_id`,
`email`, `client`, `agent`, `entrypoint`, `session`, `notebook`, `cell`.

The identity column is named `user_id`, not `user`: DataFusion's default (`Generic`) SQL dialect
parses a bare `user` in expression position as the zero-argument function call `user()`, not a
column reference, and no such scalar function is registered — `user = 'jean'` would fail at
planning with "Invalid function 'user'". None of the other columns collide with a reserved word.

Every attribute is a string, so there are no coercion surprises. Adding an attribute later means
appending one field to this schema: existing expressions keep compiling, and no migration is
involved. That is the point of the single-column design.

NULL semantics come from SQL and are the ones we want: `notebook = 'fleet-overview'` evaluates to
NULL — not true — for a query that carried no notebook header, so the rule does not fire.

`user_id` and `email` match exactly, case-sensitive — `=` with no case folding — matching the existing
precedent at `OidcProvider::is_admin` (`rust/auth/src/oidc.rs`), which already compares admin
identities against the configured list with plain `==` and has no case-insensitive path anywhere in
`rust/auth/src`. This is not a new decision, just the same rule applied here.

#### Measured: what evaluation actually costs

This check runs in front of **every** query, for as long as a rule stands, so it was measured
rather than argued about. Release build, one rule set evaluated against one query's attribution
(12 string attributes), development machine. Two columns matter: the **miss** — no rule matches,
which is what nearly every query does — and the **hit**, paid only by a query about to be rejected.

| Evaluation strategy | miss @1 rule | miss @10 | miss @100 |
|---|---|---|---|
| DataFusion `PhysicalExpr` on a one-row `RecordBatch` | 3 384 ns | 6 230 ns | 44 983 ns |
| Bytecode program over interned symbols | 31.9 ns | 39.2 ns | 158.6 ns |
| Compiled tree walk (resolved indices, owned literals) | **7.3 ns** | 71.8 ns | 763 ns |
| Flat conjunction scan (the shape most rules reduce to) | **1.1 ns** | 6.0 ns | 72.6 ns |
| Anchor probe only (one hash lookup, no rule touched) | 12.4 ns | 13.9 ns | **20.6 ns** |
| Compiled tree walk where each rule regexes the SQL text | 72.5 ns | 1 490 ns | 37 769 ns |
| `tokenize` + `fingerprint_of` (SHA-256, ~130-char statement) | **paid unconditionally, on every query, rule count irrelevant** | 1 180 ns | |

Of the DataFusion figure, 2 804 ns is building the one-row batch and only ~400 ns per rule is
evaluation — entering Arrow at all is the cost.

The `tokenize` + `fingerprint_of` cost is not a `check` cost at all — it runs whether or not any rule exists, because
`QueryAuditRecord` carries `sql_hash` on every terminal path (§7). So "zero rules cost nothing"
is true of the evaluator but not of the feature: every query pays ~1.2 µs of tokenizing and
hashing it did not pay before this plan, and that is roughly a third of the 3.4 µs Phase 1b exists
to remove. Phase 1b's payoff is therefore not "3.4 µs → ~20 ns"; it is "3.4 µs of evaluation
collapses to ~20 ns, sitting behind a ~1.2 µs fingerprint cost that exists independently of the
evaluator and of how many rules are configured." That fingerprint cost is accepted because the
audit log needs it for the "paste the fingerprint into `deny_queries`" workflow regardless of
whether the deny list is in active use — it is not something either evaluation strategy can shed.

Two further results were surprises worth recording,
because both contradict what this plan previously proposed:

1. **OR-folding all rules into one DataFusion expression does not help** (7 338 ns vs 6 230 ns at
   ten rules). Each disjunct is still its own array operation with its own allocation.
2. **Interning literals into symbols and running a bytecode program is a pessimization at this
   scale** — 31.9 ns against 1.1 ns for a plain scan at one rule. Hashing a string costs more than
   comparing it, and a length check rejects a mismatched literal in about a nanosecond. Cleverness
   loses to straight-line code until the rule count is in the hundreds.

And one result drives the design:

3. **Running a regex or `LIKE` over the SQL text is the only genuinely expensive predicate** —
   ~150 ns per rule against a 130-character statement, i.e. 1.5 µs at ten such rules and 37 µs at a
   hundred. That is DataFusion-grade cost, and it is incurred by the *evaluator we control*. The
   thing worth engineering is not the interpreter; it is making sure text-scanning predicates are
   pruned before they run.

#### How a text predicate is lowered — and why there is no anchor requirement

An earlier version of this section required every rule to carry a top-level `column = 'literal'`
conjunct, on the grounds that a text-scanning rule costs ~150 ns per query. That requirement was
wrong twice over, and both errors are worth recording:

- It **forbids legitimate rules.** `sql LIKE '%thread_spans%' OR client_ip = '10.4.9.221'` — "stop
  anything scanning thread_spans, and anything from that host" — is exactly what an on-call admin
  reaches for. A top-level disjunction has no conjunct to anchor *on*, so the advice to "add an
  anchor" was incoherent; and splitting it into two rules does not help either, since
  `sql LIKE '%thread_spans%'` alone is unanchorable by construction. A blanket "nothing may scan
  this view right now" is one of the strongest levers the deny list offers, and forbidding it to
  save nanoseconds is the wrong trade.
- The 150 ns figure came from a `(?i)…\s+` **regex**, and was generalized to all text matching. It
  does not survive measurement:

| Text predicate against a 127-char statement | miss | match |
|---|---|---|
| `str::contains` — what `LIKE '%literal%'` lowers to | **6.9 ns** | 7.4 ns |
| `Regex::is_match`, plain literal pattern | 12.2 ns | 12.9 ns |
| `Regex::is_match`, `(?i)` + `\s+` pattern | 51.9 ns | 58.5 ns |
| 4 / 8 / 32 separate `str::contains` | 27.9 / 55.3 / 217.3 ns | |
| 4 / 8 / 32 separate `Regex::is_match` | 49.3 / 107.1 / 414.7 ns | |
| **`RegexSet::is_match` over 4 / 8 / 32 patterns** | **21.7 / 21.5 / 21.7 ns** | |

A rule that scans the SQL text costs ~7 ns, not 150. That is the same order as the hash probe it
was supposed to be avoiding, and `RegexSet` collapses any number of such patterns into a single
flat ~22 ns pass. There is no performance problem to legislate against, so the anchor requirement
is dropped: **any boolean expression over the match context is accepted, anchored or not.**

What survives from that idea is the *optimization*, applied where it happens to fit rather than
demanded of the admin:

1. **Pattern lowering.** A `LIKE` literal is inspected at compile time and lowered to the cheapest
   equivalent: `'%lit%'` → `str::contains` (memchr-accelerated, ~7 ns), `'lit%'` → `str::starts_with`,
   `'%lit'` → `str::ends_with`, `'lit'` → equality. Only patterns with interior `_`/`%` become a
   `Regex`, and `ILIKE` adds `(?i)`. This is where most of the win is, and it costs one match
   statement at compile time.
2. **Anchor index, when a rule has one.** A rule with a top-level `column = 'literal'` conjunct is
   filed under it, so a query disagreeing on that field never evaluates the rule at all — one hash
   probe, flat in rule count.
3. **`RegexSet` for the residue.** Unanchored rules whose top-level predicate is a single text match
   share one compiled `RegexSet`, evaluated in one pass (~22 ns for up to 32 patterns) to decide
   which of them can match at all. Anything left over is walked directly.

So the per-query cost is a hash probe plus, if any unanchored text rules exist, one `RegexSet` pass:
**~13–35 ns, zero allocation, flat in the number of rules** for every realistic rule set.

The only expression still rejected on principle is one with **no column reference at all** (`true`,
`1 = 1`) — a rule that would deny every query in the deployment. That is a semantic guard, and
conflating it with the performance guard is what produced the mistaken anchor rule in the first
place.

#### The design that follows

Everything expensive happens at refresh, since rules are few and static:

1. **Parse** with `ctx.parse_sql_expr(match_expr, &match_schema())`. `ctx` is a bare
   `SessionContext::new()` held on `QueryDenyList` for exactly this call — no lakehouse tables or
   catalog registered against it, just DataFusion's parser and name resolution, so there is no
   grammar of ours to specify or keep in sync. `compile_match_expr(ctx: &SessionContext, match_expr:
   &str) -> Result<CompiledExpr>` is the function that owns steps 1-3; `refresh` calls it with
   `&self.ctx` on every reload, and `deny_queries`'s `call_with_args` calls it the same way, still
   synchronously, before `insert` ever runs (§8).
2. **Validate** by walking the `Expr`: reject subqueries, aggregates, window functions, and any
   scalar function that is not `Immutable` (which keeps `now()`/`random()` out, so a rule means the
   same thing on every replica); require a `Boolean` result and at least one column reference.
3. **Compile** into the shared flat program — field names resolved to indices, literals appended to
   one contiguous blob, patterns lowered per the table above, each rule occupying its own op range
   (see "One bundled program" below for the opcode set and the ~2× it is worth over boxed trees).
   Kleene three-valued logic is preserved: a test carries true/false/null jump targets, and only a
   fall-through to `Op::Match` denies.
4. **Index what can be indexed**: rules with a top-level equality go in a per-field
   `Vec<HashMap<Box<str>, Vec<RuleIdx>>>` indexed by `FieldIdx` — keying each field's map on
   `Box<str>` alone (rather than a `(FieldIdx, Box<str>)` tuple) is what lets a probe use a
   borrowed `&str` with no allocation, since `Borrow` does not decompose through tuples and a
   tuple-keyed map could only be probed by building a new `Box<str>` per lookup. Unanchored
   single-text-predicate rules go into a `RegexSet`, one per field that has any — a query denies
   nothing on a text match alone, so this is still gated by which field's `RegexSet` actually hits.
   The remaining unanchored rules (no top-level equality and not a single text predicate) are laid
   out contiguously in the program as the `unanchored` range, which the `RegexSet`-gated rules are
   *not* part of, so nothing is evaluated twice.

`RuleIdx` is assigned at compile time in stable rule order — by `(created_at, rule_id)`, oldest
first — so a lower `RuleIdx` always means an older rule. Per query, `check` does: empty snapshot →
return; otherwise probe the anchor index (one hash lookup per anchored field, usually one), run each
field's `RegexSet` pass and collect the `RuleIdx`s of the patterns that hit, run the `unanchored`
range of the program, and run the op ranges of whatever candidates the anchor index and the
`RegexSet` passes produced — and returns the rule behind the **minimum** matching `RuleIdx` across
all of that, not the first `Op::Match` encountered in evaluation order. This is what keeps "oldest
matching rule wins" (§4) true regardless of which pass happens to run first. Nothing allocates: the
attribution is borrowed `&str`, the program and its literal blob live in the snapshot `Arc`, and
neither `str::contains` nor `Regex::is_match` allocates.

#### One bundled program, not one tree per rule

`check` returns `Option<Arc<QueryDenyRule>>` — "denied, and by which rule". How that identity is
produced was measured, because bundling every rule into one evaluation removes a layer of
interpreter entry per rule: no recursion, no `Box` chasing between nodes, ops and literals
contiguous in memory, short-circuit as a forward jump.

Per-rule boxed trees versus one flat program, restricted to the conjunctive subset with NULL
treated as false (same rules, same semantics, miss = no rule matches):

| Rule shape | rules | per-rule tree walk | bundled flat program |
|---|---|---|---|
| two equality conjuncts, miss | 1 | 6.5 ns | **4.9 ns** |
| | 10 | 59.6 ns | **33.0 ns** |
| | 100 | 609.6 ns | **310.0 ns** |
| two equality conjuncts, hit | 100 | 600.9 ns | **302.8 ns** |
| equality + substring on SQL, miss | 10 | 36.8 ns | **17.0 ns** |
| | 100 | 359.3 ns | **186.6 ns** |

A consistent **~2×** on the conjunctive, NULL-as-false subset measured above, on both the miss and
the hit, at every rule count. So the compiled form is a flat program, not a tree — and since the
grammar (below) admits `OR`/`NOT` and NULL is three-valued, every test carries all three jump
targets rather than a single `jf`:

```rust
/// All rules in one contiguous op array; every literal in one contiguous blob, referenced by
/// (offset, len). No pointer chasing, no recursion, no per-rule call. Every test carries explicit
/// true/false/null jump targets so OR, NOT, and Kleene-logic NULL propagation are representable
/// directly in the op stream, not bolted on later.
pub enum Op {
    Eq       { field: u8, off: u32, len: u32, jt: u32, jf: u32, jn: u32 },
    Contains { field: u8, off: u32, len: u32, jt: u32, jf: u32, jn: u32 },
    StartsWith { .. }, EndsWith { .. }, Regex { field: u8, re: u32, jt: u32, jf: u32, jn: u32 },
    IsNull   { field: u8, jt: u32, jf: u32 },
    Match(RuleIdx),           // reached with the rule's expression evaluating to true
}
struct Program { ops: Vec<Op>, blob: Vec<u8>, regexes: Vec<Regex>, rule_ranges: Vec<Range<u32>> }
```

Note what this does *not* change: bundling is a representation choice, not a fusion of the rules
into a single predicate. Each rule still occupies its own op range, `rule_ranges` records where,
and `Op::Match` carries the rule index — so first-match-wins ordering, per-rule identity, and
per-rule failure isolation all survive. A rule that no longer compiles is simply left out of the
program at build time, with a warning; the rest still run. That was the decisive objection to a
fused `CASE WHEN r0 THEN … WHEN r1 THEN … END` expression, and a bundled *program* avoids it while
capturing the speed.

Two caveats recorded honestly:

- **The 2× was measured on conjunctive rules with NULL treated as failure**, which is the dominant
  shape. General expressions with `OR`/`NOT` need genuine three-valued logic, which the `jt`/`jf`/
  `jn` layout above provides for every test, not only conjunctions. Exercising all three targets on
  the conjunctive/NULL-as-false subset costs a little of the measured margin; it does not change
  the representation.
- **Under DataFusion, bundling remains a wash** — OR-folding ten rules measured 7 338 ns against
  6 230 ns separately, and a `CASE` behaves the same, since a columnar engine evaluates every branch
  as its own array operation and then selects. The bundling win is specific to the compiled
  evaluator, so Phase 1 (DataFusion) evaluates candidates per rule and Phase 1b introduces the
  program.

**The index still matters more than the bundle.** At a hundred rules the flat program costs 310 ns
where the anchor probe costs 20.6 ns, so pruning remains the primary lever and the program is what
runs on what pruning leaves behind:

- anchored rules → the index yields candidate rule ids → run those rules' op ranges;
- unanchored single-text-predicate rules → gated by their field's `RegexSet` → only a hit runs that
  rule's op range;
- everything else unanchored → laid out contiguously as the `unanchored` range and run as one
  bundled pass on every query, which is exactly where the 2× is collected, since that is the set
  that cannot be pruned at all.

**The custom evaluator is the accepted price, and DataFusion is the oracle that keeps it honest.**
Owning Kleene logic and `LIKE`-to-predicate lowering is where hand-rolled implementations go subtly
wrong. So a differential test compiles the same expression down both paths — the flat program and
DataFusion's own `PhysicalExpr` — and asserts they agree over a corpus of expressions × attributions
(NULL attributes, `NOT` over NULL, `%`/`_` patterns, regex metacharacters inside a `LIKE` literal,
`ILIKE` casing). DataFusion stays in the test binary as the reference implementation; production
never calls it.

#### Compiling the rules to native code — considered, not adopted

**A JIT would beat this evaluator at evaluating.** That is not in dispute, and nothing below claims
otherwise. A tree walk pays enum dispatch, pointer chasing, and bounds checks per node; compiled
code with literals as immediates and lengths known pays none of that. On the measured 763 ns tree
walk over 100 rules, native code would plausibly be several times faster. **No JIT was built or
measured here** — every Cranelift figure in this section is an estimate, and should be read as one.

The argument is narrower, and it is about *which* work survives:

- The amortization case for a compiler is sound, and I had it backwards earlier: the unit is
  **queries**, not rows. A rule compiled once and evaluated on every query for a week amortizes
  across millions of evaluations — a better ratio than a columnar scan gets.
- But after the anchor probe and the `RegexSet` pass, what remains on the common path is **one hash
  lookup and one regex-crate scan**. A JIT emits the same hash function and calls into the same
  `regex` machinery; there is no dispatch left for it to remove. The cheap win on the hash is a
  faster hasher (`rustc-hash`/FxHash) — perhaps 3–5 ns off the 12–20 ns measured, one line, no code
  generator.
- At small rule counts the residual work is already at the floor: 1.1 ns for a single rule is one
  length comparison that fails. Nothing — interpreted, compiled, or hand-written in assembly — gets
  meaningfully under that.

So the honest framing is not "the interpreter beats a JIT". It is that **pruning beat code
generation**: an index that skips 99 of 100 rules (20.6 ns) does better than compiling all 100 into
fast code would, and the two compose rather than compete — a JIT layered on the pruned design would
be optimizing the ~13–35 ns that is left. That is the general principle at work; better asymptotics
beat better constants, and the deny list had an asymptotic fix available.

What a JIT would additionally cost, if that residue ever mattered:

**Cranelift (`cranelift-jit`)** is the strongest version — compile the whole rule set into one
function, no loop, no dispatch. But Cranelift IR has no string type: `sql_hash = 'X'` becomes a
length compare plus an emitted `memcmp` call, and `LIKE`, regex, and three-valued logic must all be
generated or called out to. That is strictly more custom machinery than the flat program above,
with a much harder correctness story (a miscompile is a wrongly allowed or wrongly denied query with
no stack to inspect), plus `PROT_EXEC` pages in the service process — a security review in most
hardened deployments — and a large dependency.

**Wasmtime** has the same code-generation problem plus a boundary the Cranelift case does not: the
SQL text and every attribute must be copied into guest linear memory on each call, which puts its
floor *above* a native evaluator rather than below it. Its value proposition is sandboxing untrusted
code, and there is none here — rules come from admins, and whatever runs was generated by us. The
repo's existing WASM work is not a head start either: `datafusion-wasm` is DataFusion compiled
**to** `wasm32` to run in the browser, the opposite direction from embedding a host runtime, and no
`wasmtime`/`wasmi` dependency exists in the workspace.

**What would flip this**: rules in the thousands, where pruning stops being enough. `RegexSet`
already covers the text predicates; the next levers are a faster hasher and multi-field probes in
the anchor index. Native compilation is the step after those, and it should be measured against
them rather than assumed to win.

#### Grammar

DataFusion parses, so the syntax is SQL's; the compiler accepts the subset it can lower to
the program: `AND`/`OR`/`NOT`, `=`/`!=`, `IN`, `LIKE`/`ILIKE`, `IS [NOT] NULL`, and `regexp_like`,
over the match-context columns and string literals. Anything else — arithmetic, subqueries,
aggregates, non-`Immutable` functions, column-to-column comparison — is rejected at insert with the
parser's own diagnostic where there is one.

The only expression rejected on principle is one with no column reference at all (`true`, `1 = 1`),
which would deny every query in the deployment. There is no anchor requirement: any boolean shape,
including a top-level `OR`, is accepted (§3).

Regex is safe: the `regex` crate does not backtrack and guarantees linear-time matching, and
patterns are compiled once per rule rather than per query. (This corrects an earlier version of this
plan, which excluded regex on ReDoS grounds — a hazard of backtracking engines, not of this one.)
It is also cheap: a `LIKE '%literal%'` lowers to a ~7 ns substring search, and many text patterns
share one ~22 ns `RegexSet` pass.

Examples, all valid:

```sql
sql_hash = '9f2c41ab73de0155' AND entrypoint = 'grafana-alert'
user_id = 'dashboards-svc' AND notebook = 'fleet-overview'
client_ip = '10.4.9.221' AND sql LIKE '%thread_spans%'
client = 'grafana' AND regexp_like(sql, '(?i)from\s+view_instance')
email = 'jean@example.com' AND (notebook IS NOT NULL OR entrypoint = 'notebook')
```

A blanket rule with no equality to key on is equally valid, and is the strongest lever the deny
list offers during an incident:

```sql
sql LIKE '%thread_spans%' OR client_ip = '10.4.9.221'
sql LIKE '%thread_spans%'
```

The first five are anchored, so a single hash probe prunes them. The last two are not; their text
predicates lower to `str::contains` and share the `RegexSet` pass, costing tens of nanoseconds per
query for as long as they stand (§3).

A `criterion` bench (`rust/analytics/benches/query_deny_match.rs`, alongside the existing
`property_get`/`parse_block` benches) pins `check` at 0, 1, 10, and 100 rules, for anchored and
unanchored rule sets, on both the miss and the matching hit — so the numbers above stay true
instead of decaying into folklore. The same bench also pins the `tokenize` + `fingerprint_of` cost on
a representative statement, since that cost sits in front of `check` on every query regardless of rule count and is
otherwise easy to let decay unmeasured. Both tables came from throwaway versions of exactly that
bench.

#### Where this can go next

The column holds text and the expression language is DataFusion's, so richer semantics land without
a migration and mostly without code: numeric comparison the moment the match context carries a
numeric attribute, a structured `deny_queries` variant that renders an expression for a UI builder,
cost predicates once an estimate is available at check time, or a `test_query_denial(expr)` function
that dry-runs an expression against recent audit records before it goes live. Performance is an
independent axis: the anchor index and the `RegexSet` pass both widen without touching the stored
form, and native compilation stays available behind them (§3).

### 4. Rule model, evaluation, and cache

```rust
pub struct QueryAttribution<'a> {      // borrowed view of what execute_query already resolved
    pub user_id: &'a str, pub email: &'a str,
    pub service_account: Option<&'a str>,
    pub client: &'a str, pub agent: &'a str, pub entrypoint: &'a str,
    pub session: Option<&'a str>, pub notebook: Option<&'a str>, pub cell: Option<&'a str>,
    pub client_ip: &'a str,
    pub sql: &'a str, pub sql_hash: &'a str,
}

impl QueryAttribution<'_> {
    /// Borrowed field by match-context index -- what both the anchor probe and
    /// the program's tests read. Nothing here copies or allocates.
    fn field(&self, idx: FieldIdx) -> Option<&str>;
}

/// The DB row, exactly as `list_query_denials()` returns it (§8) and as `insert` echoes back.
/// No `ops`/`anchor`/in-process counters here — those exist only for a rule that has been
/// compiled into a `DenySnapshot`, and a freshly inserted or listed row has not necessarily
/// been through that yet on every replica.
pub struct QueryDenyRow {
    pub rule_id: Uuid, pub created_at: DateTime<Utc>, pub created_by: String,
    pub reason: String, pub match_expr: String,
    pub hit_count: i64,                       // last flushed value
    pub last_hit_at: Option<DateTime<Utc>>,   // None until the rule first fires
}

/// A row compiled into one snapshot: adds the pieces `check` needs and lives only inside
/// `DenySnapshot`. **This is the Phase-1b form** (Implementation Steps, Phase 1b step 4) — it is
/// what the bundled flat program (§3) needs. Phase 1's `check` runs DataFusion `PhysicalExpr`s
/// directly and has no `ops`/`anchor`; its snapshot is a plain
/// `Vec<(QueryDenyRow, Arc<dyn PhysicalExpr>)>`.
pub struct QueryDenyRule {
    pub row: QueryDenyRow,
    ops: Range<u32>,                          // this rule's slice of DenySnapshot::program
    pub anchor: Option<(FieldIdx, Box<str>)>, // Some when the rule has a top-level equality
    hits: AtomicU64,                      // in-process delta since the last flush
    last_hit: AtomicI64,                  // unix seconds, 0 = not hit since the last flush
}

/// Rule vector plus the anchor index built from it (§3). Rebuilt wholesale on every refresh,
/// which is cheap because rules are few and change rarely. **This is the Phase-1b form**; see
/// `QueryDenyRule` above for Phase 1's minimal stand-in.
pub struct DenySnapshot {
    rules: Vec<Arc<QueryDenyRule>>,
    program: Program,                      // every rule's ops + one literal blob (§3)
    index: Vec<HashMap<Box<str>, Vec<RuleIdx>>>,  // indexed by FieldIdx; each map probes with &str, no allocation
    anchored_fields: Vec<FieldIdx>,        // usually one; probed on every query
    text_sets: Vec<(FieldIdx, RegexSet, Vec<RuleIdx>)>,  // one per field with unanchored text rules;
                                            // NOT part of `unanchored` below — RegexSet-gated
                                            // rules are pruned there, not bundled
    unanchored: Range<u32>,                // remaining unanchored rules; contiguous, run as one pass
}

pub struct QueryDenyList {               // owned by LakehouseContext, like AudienceIndex
    pool: sqlx::Pool<sqlx::Postgres>,
    // Bare `SessionContext::new()` — no lakehouse tables or catalog registered, held only so
    // `compile_match_expr` can call `ctx.parse_sql_expr(expr, &match_schema())`; both `refresh`
    // and `insert` compile through it.
    ctx: SessionContext,
    snapshot: std::sync::RwLock<Arc<DenySnapshot>>,  // `arc-swap` is not a workspace dep;
                                   // a read lock held for one clone is enough
}

impl QueryDenyList {
    pub fn check(&self, q: &QueryAttribution<'_>) -> Option<Arc<QueryDenyRule>>;
    pub async fn refresh(&self) -> Result<()>;     // flush hit deltas, reload + recompile
    // `compiled` was already produced by `call_with_args`'s synchronous `compile_match_expr`
    // (§8); `insert` stores `match_expr` verbatim and does not re-validate it.
    pub async fn insert(&self, match_expr: &str, compiled: CompiledExpr, reason: &str, created_by: &str)
        -> Result<QueryDenyRow>;
    pub async fn delete(&self, rule_id: Uuid) -> Result<bool>;
    pub async fn list(&self) -> Result<Vec<QueryDenyRow>>;
    pub fn spawn_refresh_task(self: Arc<Self>, shutdown: impl Future<Output = ()> + Send + 'static);
}
```

- **Zero rules make `check` itself free.** `check` returns on an empty-snapshot test — the steady
  state of every deployment that is not mid-incident. This is a claim about `check`, not about the
  feature end to end: `tokenize` + `fingerprint_of` run unconditionally regardless of rule count, at
  ~1.2 µs per query (§3, §7), because the audit record carries `sql_hash` on every terminal path.
- With rules present, `check` probes the anchor index (one hash lookup per anchored field, usually
  one) and runs each field's `RegexSet` pass, if any exist. **~13–35 ns and zero
  allocation, flat in the number of rules** (§3).
- The `unanchored` program range runs as one bundled pass, and the candidates the anchor index and
  the `RegexSet` passes produced have their own op ranges run — the minimum matching `RuleIdx`
  across all of it winning, not the first `Op::Match` evaluated.
- **Candidates are compared by a stable rule order** — `RuleIdx`, assigned by `created_at`, `rule_id`
  breaking ties — not the order the index buckets or the program happen to evaluate them in. Two
  replicas must name the same rule for the same
  query, since the rule id reaches the caller's error message, the `warn!` line, the audit record,
  and the per-rule metric. The oldest matching rule wins, which is also the one an operator is most
  likely to have forgotten about.
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
  than denying everything or crashing. It also absorbs the one real cost of borrowing DataFusion's
  parser — **the stored rule format is coupled to DataFusion's SQL dialect**, so an upgrade could in
  principle change how an existing `match_expr` parses. A rule that stops compiling stops being
  enforced, loudly, instead of silently changing meaning; the `query_deny_compile_error_count`
  metric is what an upgrade should be watched on.
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
    && !skip_for_admin_recovery(caller_is_admin, self.admin_principal_possible, &sql_tokens)
{
    let status = Status::resource_exhausted(format!(
        "query denied by rule {} (reason: {}); ask an admin to lift it with \
         remove_query_denial('{}'); query_id={query_id}",
        rule.row.rule_id, rule.row.reason, rule.row.rule_id));
    warn!(
        "query denied rule_id={} reason={:?} sql_hash={sql_hash} user={} email={} \
         client={client_type} entrypoint={client_entrypoint} client_ip={client_ip} \
         query_id={query_id}",
        rule.row.rule_id, rule.row.reason, attr.user_id, attr.user_email);
    imetric!("query_denied", "count", rule_tags(&rule.row.rule_id), 1_u64);
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
  own. So the check is skipped when the caller can reach the admin functions at all —
  `is_admin || !admin_principal_possible`, the same predicate `register_lakehouse_functions` gates
  `deny_queries`/`remove_query_denial` on (Current State, "Admin gating") — **and** the statement
  *calls* one of `deny_queries` / `remove_query_denial` / `list_query_denials` — the name token
  immediately followed by a `(` token (checked over the token stream already produced for the
  fingerprint, which has already dropped comments and collapsed whitespace runs, so no intervening
  trivia can separate the two). Call position, not mere identifier presence, is required: a column
  alias (`SELECT x AS remove_query_denial FROM …`) is an identifier token too, and would otherwise
  exempt any statement carrying it. Gating on `is_admin` alone would leave the hatch permanently shut in any
  deployment where `admin_principal_possible` is false (no admin principal configured: every API-key
  provider, or OIDC with an empty `admin_users` list) — `is_admin` is then always false while the
  mutating functions are registered for every caller, exactly the deployment this hatch most needs to
  work in. A non-admin cannot exploit the wider gate: the mutating functions are registered under the
  identical predicate, so any caller for whom the escape hatch opens is, by construction, a caller who
  could call them directly anyway. `skip_for_admin_recovery(caller_is_admin, admin_principal_possible,
  sql_tokens)` lives in `query_deny_list.rs` alongside the rest of the matching logic — it only needs
  the token stream and the two booleans, no `flight_sql_service_impl.rs`-private state — so it is a
  `pub` function in the analytics crate and its unit tests sit with the rest of that crate's tests.
  This is the primary recovery path, so it carries its own test, including the
  `admin_principal_possible == false` deployment shape.
- **Prepared statements are deliberately not checked.** `do_action_create_prepared_statement` is not
  a second insertion point, because nothing executes through it. It only plans — `ctx.sql()` to
  recover the result schema — and echoes the SQL back as the handle; `do_get_prepared_statement` is
  `api_entry_not_implemented!()`, so a prepared handle can never be executed. The Python client's
  `prepared_statement_stream` reflects this: it calls `query_stream(statement.query)`, which returns
  through `do_get_statement` → `execute_query`, where the check already sits. So there is no bypass
  to close and no scan cost to shed on that path — planning touches the catalog, not the data.
  Checking there would buy only earlier feedback during schema discovery, at the price of resolving
  attribution, `get_client_ip`, the `x-client-*` headers, and a tokenize pass on an RPC that has none
  of them today and no audit record to write them to. One check site, on the path that actually
  spends the money, is the whole design. If `do_get_prepared_statement` is ever implemented, the
  check has to be added there — that is the trigger to revisit this, not the prepare RPC itself.

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
    pub identity: Option<CallerIdentity>,   // None on internal/maintenance paths; such a caller
                                             // cannot call `deny_queries`, which requires `Some` (§8)
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

The table function's `call_with_args` is handed a `&dyn Session` (via `TableFunctionArgs::session`),
which has no SQL-expression parser — and rule compilation must produce the same result on every
replica regardless of whose session triggered it, so compilation goes through the bare
`SessionContext` the list holds for exactly this (§3, §4) either way, not the caller's session.
`call_with_args` itself runs, synchronously and all fail-loud with `plan_err!`: `compile_match_expr`
against that context (§3), reported with the offending token where the parser gives one; the
empty-`reason` check; the caller-identity check (`CallerContext::identity` must be `Some` — a `None`
identity, the maintenance/internal path, is rejected here, since `created_by` is `NOT NULL` and has
no sentinel to fall back on, §7); and the rule-count check against the current snapshot
(`MICROMEGAS_QUERY_DENY_MAX_RULES`). Only the DB write and the local snapshot refresh happen in the
async body behind `LogStreamTableProvider`/`TaskLogExecPlan`: `QueryDenyList::insert` takes the
already-compiled expression (plus the original `match_expr` text, stored verbatim) and cannot itself
fail on a bad expression.

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
`substitute_macros` already follows server-side. The textarea holds the expression in its natural,
single-quoted form (as the mockup shows — e.g. `sql_hash = '9f2c41ab73de0155' AND entrypoint =
'grafana-alert'`, and every insert-chip inserts a single-quoted fragment), so `escapeSqlLiteral`
doubles those quotes on the way out, exactly as it does for the reason field. The round trip is
worth a dedicated test.

Non-admins never reach the page (`AuthGuard requireAdmin`), and a non-admin who hand-typed the SQL
would get "function not found" from `flight-sql-srv` — the gate is server-side, the guard is UX.

`BLOCKED_FUNCTIONS` in `stream_query.rs` is deliberately **not** extended for the three new admin
functions themselves: they are admin-gated at `flight-sql-srv`, which is precisely what the web
screen needs to call. But `contains_blocked_function` substring-matches the entire lowercased SQL
text, not just call position, so a `deny_queries(...)` statement whose *expression* merely mentions
one of the blocked names — e.g. `SELECT * FROM deny_queries('sql LIKE ''%retire_partitions%''',
'…')`, a reasonable incident rule aimed at exactly that function — would be rejected by
`/api/stream-query` with a misleading "destructive function" error before it ever reaches
`flight-sql-srv`. `contains_blocked_function` is narrowed alongside this feature to match a blocked
name only in call position — the name immediately followed by `(`, skipping intervening whitespace
**and SQL comments** (`/* … */` and `-- …`), so a comment inserted between the name and the
parenthesis cannot slip a real call past the check. This also closes the same false positive for any
other statement that merely quotes one of those names without calling it.

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
Triage does not belong on this screen: **finding the offender is a notebook's job** — a notebook
over the audit log listing the queries the service ran in the last few minutes, grouped by
`sql_hash` with their cost and attribution. That is an interactive, exploratory task where the
useful next step is usually another query, not a button; a fixed panel would answer one shape of
question and get in the way of the rest. The screen stays a front end for the three SQL functions,
and the admin pastes the fingerprint the notebook surfaced into the dialog.

## Implementation Steps

### Phase 1 — Store and matching (analytics crate, no wiring)

1. `rust/analytics/src/lakehouse/migration.rs`: `upgrade_v8_to_v9` creating `query_deny_list`;
   `LATEST_LAKEHOUSE_SCHEMA_VERSION = 9`.
2. New `rust/analytics/src/lakehouse/query_deny_list.rs`: `tokenize`/`fingerprint_of`, the match-context
   schema, `compile_match_expr` (parse → validate → `Arc<dyn PhysicalExpr>` in this phase),
   `QueryAttribution`, `QueryDenyRow`, and Phase 1's own minimal snapshot shape — a
   `Vec<(QueryDenyRow, Arc<dyn PhysicalExpr>)>`, with none of `QueryDenyRule`'s `ops`/`anchor` or
   `DenySnapshot`'s `program`/`index`/`text_sets`/`unanchored` (those are the Phase-1b form built in
   step 4, §4) — plus `QueryDenyList` (`check` / `refresh` / `insert` / `delete` / `list` /
   `spawn_refresh_task`), env knobs. Register in `rust/analytics/src/lakehouse/mod.rs`. Add `sha2`
   to `analytics/Cargo.toml`.

   **Evaluate with DataFusion's `PhysicalExpr` in this step** — `create_physical_expr` on the
   validated `Expr`, one row batch per candidate. It is a handful of lines, owns no SQL semantics,
   and is correct by construction. `check`'s signature and the whole rest of this plan are
   unchanged by which evaluator sits behind it.
3. Unit tests for `tokenize`/`fingerprint_of` and validation (see Testing Strategy), plus the corpus that
   Phase 1b's differential test will reuse.

### Phase 1b — Swap in the compiled evaluator (optional, measurable)

4. Replace the `PhysicalExpr` evaluation with the bundled flat program (§3): resolve field indices,
   append literals to one blob, lower `LIKE` to `contains`/`starts_with`/`ends_with`/`Regex`, lay
   every rule out as its own op range with the unanchored ones contiguous, and build the anchor
   index and shared `RegexSet` — this is where `QueryDenyRule`'s `ops`/`anchor` and `DenySnapshot`'s
   `program`/`index`/`text_sets`/`unanchored` (§4) get built for the first time. Add `regex = "1"` to
   `rust/Cargo.toml`'s `[workspace.dependencies]`
   (already in the tree transitively via `datafusion-functions`, but not declared there today) and
   `regex.workspace = true` to `analytics/Cargo.toml`, matching the crate's existing convention.
5. Turn the Phase 1 corpus into the **differential test**: every expression compiled both ways, and
   the two evaluators must agree on every attribution. DataFusion becomes the oracle exactly at the
   moment we stop using it in production.
6. Land `benches/query_deny_match.rs`, including a `tokenize` + `fingerprint_of` bench, and confirm
   the §3 numbers on the target hardware.

   Splitting here is deliberate: Phase 1 is correct and shippable on its own at 3.4 µs/query, and
   Phase 1b is a pure optimization behind an unchanged interface, with the oracle test already
   written. If the deny list turns out to be used a few times a year, Phase 1b can simply wait for
   a profile to justify it.

### Phase 2 — Wiring and enforcement

7. `lakehouse_context.rs`: construct and expose `query_denials()` (mirrors `audience_index()`).
8. `read_scope.rs`: add `CallerIdentity` + `CallerContext::identity`; fix every construction site
   the compiler flags.
9. `flight_sql_service_impl.rs`: call `tokenize` once per query and derive both `fingerprint_of` and
   the escape-hatch identifier check from its result; add `sql_hash` to `QueryAuditState`
   and `QueryAuditRecord`; add `fail_with_class`; insert the deny-list check after `QueryAuditState`
   is built, with its `warn!` line and the rule-tagged `query_denied` metric (§6); populate
   `CallerContext::identity` in `caller_context`. `do_action_create_prepared_statement` is left
   untouched — it plans without executing and cannot be executed through, so it is not a check site
   (§5).
10. `flight_sql_server.rs`: spawn the refresh task with the existing shutdown fanout.

### Phase 3 — Admin SQL functions

11. `list_query_denials_table_function.rs` (pattern: `list_partitions_table_function.rs`).
12. `deny_queries_table_function.rs` — validates, inserts, refreshes the local snapshot, returns
   one row.
13. `remove_query_denial_udf.rs` (pattern: `retire_partition_by_file_udf.rs`).
14. Register all three in `query.rs`'s admin block.

### Phase 4 — Web app screen

15. `analytics-web-app/src/lib/query-deny-list-api.ts` — SQL builders (`escapeSqlLiteral`, the
    three statements) and Arrow→row decoding.
16. `analytics-web-app/src/routes/QueryDenyListPage.tsx` — the page, reusing
    `PageLayout` / `AuthGuard requireAdmin` / `ErrorBanner` / `ConfirmDialog` / `Button` /
    `DataSourceField`.
17. Register the route (`/admin/query-deny-list`) in `router.tsx` and add the card to
    `AdminPage.tsx` (lucide `ShieldBan` or `Ban` icon).
18. Vitest coverage for the SQL builders (escaping in particular) and a page render test, matching
    `routes/__tests__/AnalyticsApiKeysPage.test.tsx`.

### Phase 5 — Docs and changelog

19. `mkdocs/docs/admin/functions-reference.md`: the three functions, with the incident runbook.
20. `mkdocs/docs/query-guide/query-audit-log.md`: document `sql_hash` and `error_class = "denied"`,
    plus the "find the offender, copy its fingerprint" query.
21. `python/notebooks/query_load.ipynb`: the triage notebook built on that query — recent queries
    grouped by `sql_hash` with cost and attribution. This is the "find the offender" step the
    runbook sends operators to; the admin screen deliberately does not host it (see Mockups).
22. `mkdocs/docs/admin/flight-sql.md`: the three env knobs, propagation delay, fail-open behavior.
23. `mkdocs/docs/admin/web-app.md`: the new admin screen.
24. `mkdocs/docs/query-guide/python-api.md`: update the exception-types table and the "tell them
    apart" guidance (§ Exception types) — a denial is also `ResourceExhausted` /
    `pyarrow.lib.ArrowInvalid` with the same message prefix as a resource-budget failure, so both
    existing discriminators (message prefix, `error_class: "resource"`) stop being sufficient; add
    the deny-list row and point readers at `error_class: "denied"`.
25. `CHANGELOG.md`: feature entry + **Minor breaking change** clause for `CallerContext`.

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
- `python/notebooks/query_load.ipynb` — the triage entry point (see Mockups): the queries the
  service ran over a recent window, grouped by `sql_hash` with cost and attribution, so an operator
  can identify the offender and copy its fingerprint. Alongside the existing notebooks in that
  directory, and built on the audit-log query documented in `query-audit-log.md`.

**Modify**

- `rust/analytics/src/lakehouse/migration.rs`, `mod.rs`, `query.rs`, `read_scope.rs`,
  `lakehouse_context.rs`
- `rust/Cargo.toml` (add `regex` to `[workspace.dependencies]`)
- `rust/analytics/Cargo.toml` (`sha2.workspace = true`, `regex.workspace = true`, plus a new
  `[[bench]] name = "query_deny_match"` stanza alongside the existing `property_get`/`parse_block`
  entries)
- `rust/public/src/servers/flight_sql_service_impl.rs`, `query_audit.rs`, `flight_sql_server.rs`
- `rust/analytics-web-srv/src/stream_query.rs` (narrow `contains_blocked_function` to call position)
  and its existing test block, `rust/analytics-web-srv/tests/stream_query_tests.rs`
- `analytics-web-app/src/router.tsx`, `src/routes/AdminPage.tsx`
- `mkdocs/docs/admin/functions-reference.md`, `admin/flight-sql.md`, `admin/web-app.md`,
  `query-guide/query-audit-log.md`, `query-guide/python-api.md`
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

**DataFusion parses; a bundled flat program evaluates.** This check is a tax on every query for as long as
a rule stands, so it was benchmarked (§3). DataFusion costs 3.4 µs per query, 2.8 µs of which is
just building the one-row batch — 200× a compiled tree walk and 2000× a plain scan, plus tens of
allocations on the front door. Keeping DataFusion for parsing and name resolution but lowering to
a flat program for evaluation buys the whole gap while leaving no grammar of ours to specify. The
price is owning Kleene logic and `LIKE` lowering, which a differential test against DataFusion's
own evaluator keeps honest — the oracle stays in the test binary, never in production.

**Any boolean shape is accepted; the optimizer works around it rather than the admin.** An earlier
draft required every rule to carry a top-level equality "anchor", which would have rejected
`sql LIKE '%thread_spans%' OR client_ip = '…'` — a legitimate and powerful incident rule that cannot
be anchored at all, since a disjunction has no conjunct to anchor on. It rested on a bad number too:
a text scan costs ~7 ns once `LIKE '%lit%'` is lowered to `str::contains`, not the ~150 ns measured
for an elaborate regex, and a `RegexSet` collapses any number of such patterns into one ~22 ns pass.
Anchoring survives as an optimization applied where it fits, never as a demand on the author. The
"must not match everything" guard is a separate, semantic rule: an expression referencing no column
is rejected.

**Cleverness measured worse than straight-line code.** Interning literals into symbols and running a
bytecode program — the obvious "pre-compile it properly" design — benchmarked at 32 ns against
1.1 ns for a plain comparison scan at one rule. Hashing a string costs more than comparing it. That
result, and OR-folding rules into a single DataFusion expression turning out to be a wash, are both
recorded in §3 so the next person does not re-derive them.

**A JIT was not rejected on speed.** Compiled code would evaluate faster than the tree walk — that
is not contested, and no JIT was measured to claim otherwise. It was rejected because pruning
removes the work it would have optimized: after the anchor index and the `RegexSet` pass, the common
path is a hash lookup and a regex scan, both of which compiled code would perform identically. The
lever a JIT competes with here is a faster hasher, not an interpreter.

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
  runbook" section: find the offender in the query-load notebook → copy `sql_hash` → `deny_queries`
  → confirm rejections → `remove_query_denial` once the offending client is fixed.
- `mkdocs/docs/query-guide/query-audit-log.md` — the new `sql_hash` field, `error_class = "denied"`,
  and the top-offenders query the notebook is built around, so it is documented once and reusable
  outside the notebook.
- `mkdocs/docs/admin/flight-sql.md` — env knobs, propagation delay, fail-open behavior, the admin
  escape hatch, and a **"Watching for denials"** section carrying both dashboard queries from §6
  (the warning-level `log_entries` panel and the per-rule `query_denied` rate panel) so an operator
  can paste them straight into a dashboard.
- `mkdocs/docs/admin/web-app.md` — the Admin → Query Deny List screen.
- `mkdocs/docs/query-guide/python-api.md` — a denial is `ResourceExhausted` /
  `pyarrow.lib.ArrowInvalid` with the same message prefix as a resource-budget failure, so update
  the exception-types table with a deny-list row and tell readers to use `error_class: "denied"`
  (not the message prefix or `"resource"`) to distinguish the two.

## Testing Strategy

**Unit (`rust/analytics/tests/query_deny_list_tests.rs`, no DB)**

This is an external integration-test crate for `micromegas-analytics`, so it can only see `pub`
items — `CompiledExpr`, `Op` (and its variants), and `QueryDenyRule::anchor` are `pub` for exactly
this reason, so the lowering and anchor-extraction assertions below compile.

- `tokenize`/`fingerprint_of`: two dashboard refreshes differing only in timestamp/limit literals produce the
  same fingerprint; different column lists produce different ones; whitespace/comment/case changes
  are absorbed; unparseable SQL still yields a fingerprint.
- `compile_match_expr` rejects, each with a distinct message: unknown column, unknown function,
  non-boolean result, subquery, aggregate, window function, a non-`Immutable` function (`now()`),
  arithmetic, column-to-column comparison, and an expression referencing no column (`true`, `1 = 1`).
- `user_id = 'svc-acct'` compiles and matches — pinning the reason the identity column is named
  `user_id` rather than `user`: under `GenericDialect`, a bare `user` parses as the zero-arg
  function `user()`, not a column reference.
- `LIKE` lowering picks the right predicate: `'%lit%'` → `Contains`, `'lit%'` → `StartsWith`,
  `'%lit'` → `EndsWith`, `'lit'` → `Eq`, `'l_t'` → `Regex`; `ILIKE` is case-insensitive in every
  form; a literal containing regex metacharacters (`'100%'`, `'a.b'`) matches literally.
- Anchor extraction: found for a top-level `AND` chain containing `col = 'literal'` (including
  nested `AND`s), and correctly *absent* for a top-level `OR` — a rule that must still be evaluated
  on every query, and must still deny when it matches.
- **Differential test against DataFusion (the important one).** Every expression in a corpus is
  compiled both to the flat program and to DataFusion's `PhysicalExpr`, and evaluated against a corpus of
  attributions; the two must agree on every pair. Coverage aimed at where a hand-rolled evaluator
  goes wrong: NULL attributes, `NOT` over NULL, `AND`/`OR` with a NULL operand, `%`/`_` patterns,
  regex metacharacters inside a `LIKE` literal (`sql LIKE '100%'` must not become a quantifier),
  `ILIKE` case-insensitivity, and empty-string vs absent attributes.
- **Anchor-index equivalence (property test).** `check` through the index returns exactly what
  evaluating every rule by brute force returns.
- Every example expression in the docs compiles and evaluates as documented.
- `skip_for_admin_recovery` (defined and tested here, in `query_deny_list.rs`, since it only takes a
  token stream and two booleans): an admin statement calling `remove_query_denial` is exempt; the
  same statement from a non-admin is not; an admin (or `admin_principal_possible == false`)
  statement that merely aliases a column `remove_query_denial`, or references it as a quoted
  identifier, without calling it, is **not** exempt — pinning the call-position requirement, since
  both would otherwise pass a bare identifier-token check; with `admin_principal_possible == false`
  (no admin principal
  configured), a non-admin caller's `remove_query_denial` statement is exempt too, matching
  `register_lakehouse_functions`' gate.

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

**`contains_blocked_function` (extend `rust/analytics-web-srv/tests/stream_query_tests.rs`)** — the
existing five cases all use call position and would pass unchanged against either the old or the new
behavior, so cases are added to actually exercise the narrowing:

- a blocked name appearing only inside a string literal, e.g.
  `deny_queries('sql LIKE ''%retire_partitions%''', 'reason')`, is **allowed** (not flagged as a
  destructive call);
- a blocked name in call position is still **blocked**: with no whitespace (`retire_partitions()`),
  with whitespace before the parenthesis (`retire_partitions ()`), and with a comment between the
  name and the parenthesis (`retire_partitions/*x*/()`).

**End-to-end (`python/micromegas/tests/test_query_deny_list.py`, against `local_test_env`)**

- `deny_queries` with a `sql_hash` predicate → the matching query fails with a `ResourceExhausted`
  naming the rule id → `remove_query_denial` → the query succeeds again.
- A non-matching query is unaffected while the rule is in force.
- Each denial lands one `Warn`-level `log_entries` row (`msg LIKE 'query denied%'`) and one
  `query_denied` measure tagged with the rule id — the two dashboard signals from §6, checked
  end-to-end rather than only at the call site. Both rows only appear after the telemetry sink
  flushes and the maintenance role materializes the corresponding view, so both assertions poll via
  `otlp_helpers.assert_eventually` (already used for this in the test suite) with a timeout
  comfortably above `MICROMEGAS_FLUSH_PERIOD` (5 s in `local_test_env`), not a fixed sleep.
- A no-column expression is rejected, as is a syntactically invalid one, each with a message that
  names the problem.
- `list_query_denials()` shows the rule while it stands and drops it after removal; `hit_count`
  reflects the rejections once a refresh tick has flushed.
- A rule matching *everything the test client sends* still leaves `remove_query_denial` callable —
  the escape hatch (§5), and the only recovery path now that rules do not expire.
- Note: `local_test_env` runs with auth disabled, so every caller is an admin there.

**Web app (Vitest)**

- SQL builders: a reason containing `'` is escaped exactly once, and a single-quoted expression
  (e.g. `sql_hash = '9f2c41ab73de0155'`, matching the textarea's natural form) survives
  escape → server-side literal decoding unchanged.
- Page: renders rules, opens the deny dialog, calls the right SQL on confirm, shows the error
  banner on a failed query.

## Open Questions

None outstanding.
