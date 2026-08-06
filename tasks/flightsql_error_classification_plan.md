# FlightSQL Query Error Classification Plan

GitHub issue: [#1435](https://github.com/madesroches/micromegas/issues/1435)

## Overview

Every error the FlightSQL service returns — a typo'd function name, a query that blows the
per-query memory budget, or an actual server bug — comes back to the caller as gRPC
`Internal(13)`, with a message that embeds the build machine's absolute path and the
file:line of the error-conversion macro. This makes it impossible for a client (or an
alerting rule) to tell "you wrote a bad query" from "the server broke," and gives the
caller nothing actionable to fix their query with.

This plan classifies `DataFusionError`s into the right gRPC status code, strips the
non-actionable file:line/path suffix from client-facing messages — that suffix is simply no
longer generated, since it only ever came from the `status!` macro's own
`file!()/line!()` expansion, which these sites stop using — logs the full error server-side
alongside a new query id, and splits the query-audit log's severity and `status` field so
user errors stop being counted as service failures.

## Current State

All 21 error-conversion sites in `rust/public/src/servers/flight_sql_service_impl.rs` go
through one macro:

```rust
macro_rules! status {
    ($desc:expr, $err:expr) => {
        Status::internal(format!("{}: {} at {}:{}", $desc, $err, file!(), line!()))
    };
}
```

`execute_query` (`flight_sql_service_impl.rs:289-504`) is the path that matters most here —
it's the one that runs arbitrary user SQL *and executes it*. (`do_action_create_prepared_statement`,
`:867-918`, also calls `ctx.sql(&query.query)` on caller SQL and returns a `DataFusionError`
at `:889`, but only plans it — see step 2's note on that site.) `execute_query`'s error sites,
in order:

1. `ctx.sql(sql).await` (`:423`) — planning. Returns `DataFusionError`.
2. `df.limit(0, Some(parsed_limit))` (`:440`) — planning. Returns `DataFusionError`.
3. `df.create_physical_plan().await` (`:456`) — physical planning. Returns `DataFusionError`.
4. `execute_stream(plan, task_ctx)` (`:461`) — its own immediate `Result`, returns
   `DataFusionError`. Unlike the other sites, this one isn't built through the `status!`
   macro — it's a hand-rolled `Status::internal(format!("Error executing plan: {e:?}"))`
   (`:463-464`), so it also needs replacing directly with `client_error(...)`.
5. `flight_data_stream` per-batch errors (`:498`) — by this point the error has been
   re-wrapped once at `:466` as `FlightError::ExternalError(Box::new(e))` where `e` was the
   `DataFusionError` yielded by the stream, so recovering the variant means matching
   `FlightError::ExternalError` and `downcast_ref::<DataFusionError>()` on its payload.

Two other setup-phase sites (`scoped_runtime` at `:395`, `make_session_context` at `:404`)
return `anyhow::Error`, not `DataFusionError` — they represent genuine server-side setup
failures (bad config, storage unreachable), not something the caller's SQL can trigger, so
they're out of scope for reclassification and stay `Status::internal`.

`CompletionTrackedStream::poll_next` (`:204-238`) logs every stream error at `error!` and
calls `audit_state.emit("error", Some(err.to_string()))` regardless of cause.
`QueryAuditRecord` (`query_audit.rs:80-110`) has a `status: &'static str` field
(`"ok" | "error" | "incomplete"`) but nothing that distinguishes *why* it errored.

Separately, `rust/datafusion-extensions/src/**` (the `jsonb_*`, `property_get`, `lerp_color`,
`color_scale`, etc. UDFs) almost universally use `internal_err!` for what are actually
caller mistakes — wrong argument count and unsupported input type — e.g.
`rust/datafusion-extensions/src/jsonb/format_json.rs:54` (`internal_err!("wrong number of
arguments...")`) and `:59-61` (`DataFusionError::Execution` for a type mismatch, which is
the exact case in the issue's repro). `DataFusionError::Internal` is defined by DataFusion
as "this should not happen — please file a bug" (see
`datafusion-common-54.1.0/src/error.rs:93-109`), so an `internal_err!` on a plain arity
check is a misclassification independent of this issue, and it would defeat the new gRPC
mapping if left alone (every `jsonb_*` call with the wrong number of args would still come
back `Internal`). By contrast, `rust/analytics/src/lakehouse/perfetto_trace_table_function.rs`
already gets this right — it uses `plan_err!` at all five of its argument-checking sites (one
per argument, plus one validating the parsed `span_types` value).

### Confirmed via DataFusion 54.1 source (`datafusion-common-54.1.0/src/error.rs`)

- `DataFusionError::find_root()` (`:436`) walks the `Error::source()` chain and returns the
  innermost `DataFusionError`, unwrapping `Context`, `Diagnostic`, `Collection`, and
  `Shared` uniformly. This is the one place classification needs to happen — call
  `find_root()` once, match on the result.
- `DataFusionError::diagnostic()` (`:609`) returns the outermost `Diagnostic`, which (per
  the issue) carries a `Span` with `start`/`end` `Location { line, column }` into the SQL
  text for many plan-time errors (unknown column, ambiguous reference, type mismatch) —
  but only once `datafusion.sql_parser.collect_spans` is enabled. It defaults to `false`
  (`datafusion-common-54.1.0/src/config.rs:300`), and `datafusion-sql-54.1.0/src/expr/identifier.rs:68/84/107`
  only attaches a `Span` to a planned `Column` when that option is set, which the
  unknown-column/ambiguous-reference diagnostics read back
  (`datafusion-common-54.1.0/src/column.rs:273-286`). `make_session_context`
  (`rust/analytics/src/lakehouse/query.rs:216-219`) never sets it today, so Design §2 adds
  `.set_bool("datafusion.sql_parser.collect_spans", true)` to that function's `SessionConfig`
  (see Files to Modify) — without it, only diagnostics whose span comes straight off the
  sqlparser AST (e.g. "Invalid function") would carry one.
- `DataFusionError::strip_backtrace()` (`:466`) strips the `RUST_BACKTRACE` suffix DataFusion
  may append, independent of anything this plan adds.

### Confirmed via Arrow Flight C++ source (`apache/arrow` `cpp/src/arrow/flight/transport/grpc/util_internal.cc` and `transport.cc`)

Since our server is a plain tonic service (it never sets Arrow's optional
`x-arrow-status`/`grpc-status-details-bin` headers), pyarrow's Flight client falls back to
its plain grpc-status-code mapping (`transport.cc:272-320`). This determines exactly what
exception type `micromegas-query`/notebook callers will see after this change:

| gRPC code we return | pyarrow client-side result |
|---|---|
| `INTERNAL(13)` (today, always) | `pyarrow._flight.FlightInternalError` (message prefixed `"Flight returned internal error, with message: "` — this literally matches the repro in the issue) |
| `InvalidArgument(3)` | `pyarrow.lib.ArrowInvalid`, a **`ValueError` subclass** — no Flight-specific exception class exists for this code, it goes through generic Arrow status mapping. This is a good outcome: a bad query becomes a `ValueError` in Python. |
| `Unimplemented(12)` | `pyarrow.lib.ArrowNotImplementedError`, a `NotImplementedError` subclass |
| `ResourceExhausted(8)` | `pyarrow._flight.FlightUnavailableError` — gRPC `RESOURCE_EXHAUSTED` maps to Arrow Flight's internal `kUnavailable` transport code (`util_internal.cc:105-107`), which becomes an `IOError`-family exception carrying `FlightStatusDetail::Unavailable`. See **Trade-offs** — this reads as "transient, safe to retry," which is exactly wrong for "this query needs too much memory." |

Verified with `python3 -c "import pyarrow.flight as f; ..."` (pyarrow 20.0.0) that no
`FlightInvalidArgument`/`FlightResourceExhausted` classes exist client-side — confirming the
table above is the actual, not merely documented, behavior.

## Design

### 1. Central classifier

Add one function near the top of `flight_sql_service_impl.rs`, replacing the `status!`
macro's blanket `Status::internal` for the five `DataFusionError`-producing sites:

`classify_datafusion_error` is `pub` (not private) so it can be unit-tested directly from
`rust/public/tests/flight_sql_error_classification_tests.rs`, the same way `query_audit.rs`
exposes its testable helpers to that external test crate:

```rust
pub fn classify_datafusion_error(err: &DataFusionError) -> tonic::Code {
    use datafusion::error::DataFusionError as DFE;
    match err.find_root() {
        DFE::SQL(..) | DFE::Plan(_) | DFE::SchemaError(..) | DFE::Execution(_)
        | DFE::Configuration(_) => tonic::Code::InvalidArgument,
        DFE::ResourcesExhausted(_) => tonic::Code::ResourceExhausted,
        DFE::NotImplemented(_) => tonic::Code::Unimplemented,
        _ => tonic::Code::Internal,
    }
}
```

`find_root` already recurses through `Collection` (returns the classification of the first
collected error, matching `DataFusionError`'s own `error_prefix()`/`message()` convention),
`Context`, `Diagnostic`, and `Shared`. `External` wrapping a non-`DataFusionError` falls
through to the `_ => Internal` arm, per the mapping table in the issue. `Configuration` is
included alongside `SQL`/`Plan`/`SchemaError`/`Execution` because it's reachable from a
caller's own `SET datafusion.…` statement. `ArrowError` deliberately stays in the `_ =>
Internal` bucket: `DataFusionError::ArrowError`'s `source()` is the `ArrowError` itself (not a
`DataFusionError`), so `find_root()` returns it directly with no further variant to
distinguish "caller-triggered" (e.g. a cast failure or divide-by-zero on user data) from a
genuine internal Arrow-kernel bug — see Trade-offs.

### 2. Message construction (drops file:line/path, adds query id, diagnostic span, or plan dump)

Plan-time errors (`SQL`/`Plan`/`SchemaError`) can carry a `Diagnostic` with a `Span` into
the SQL text — a precise line/column the caller can jump to — but only once
`datafusion.sql_parser.collect_spans` is enabled (see "Confirmed via DataFusion 54.1 source"
above); add `.set_bool("datafusion.sql_parser.collect_spans", true)` to the `SessionConfig`
built in `make_session_context` (`rust/analytics/src/lakehouse/query.rs:216-219`) so the
span actually gets populated. Execution-time errors (the
`Execution` variant covering the issue's own repro, and most UDF failures once Design §5
lands) never do: `Diagnostic`/`Span` is populated by DataFusion's analyzer/planner, not by a
`ScalarUDFImpl::invoke_with_args` running mid-execution. For that case, the next best
locator is the captured physical plan (`audit_state.plan`, already set at `:460` before
execution starts) — not an exact attribution (nothing tags *which* node raised the error),
but enough for whoever is debugging the failure to see the plan shape and spot the
operator/expression that lines up with the message (e.g. a `ProjectionExec` listing the
failing `jsonb_format_json(...)` call). That plan text is **server-log-only** — it is never
appended to the client-facing `Status` message. `displayable(plan).indent(true)` renders
`FileScanConfig::fmt_as`'s `file_groups=` section, which prints each scanned file's
`object_meta.location` (`datafusion-datasource-54.1.0/src/file_scan_config/mod.rs:666-674`,
`src/display.rs:32-72`) — exactly the shape of the `DataSourceExec`/`ParquetSource` plans
Micromegas view scans build (`rust/analytics/src/lakehouse/partitioned_execution_plan.rs:342-357`),
so sending it to the caller would leak internal lakehouse object-store partition paths on
every execution-time error, including genuine server bugs classified `Internal`. Keeping it
server-side avoids that entirely, at the cost of the caller not seeing the plan shape
themselves — they get the query id instead, which the server-log line also carries, so anyone
who needs the plan can grep for it. `client_error` takes the plan as an optional extra input
and only logs it (never returns it to the client) when there's no diagnostic span to show:

```rust
const MAX_PLAN_CHARS: usize = 2000;

// Pure and `pub` (like `classify_datafusion_error`/`client_error`) so step 9's tests can
// assert its content — including the truncation marker — without capturing log output.
pub fn build_log_line(
    desc: &str,
    err: &DataFusionError,
    query_id: &str,
    plan: Option<&Arc<dyn ExecutionPlan>>,
) -> String {
    // Log `err` itself, not `err.find_root()` — `find_root()` is for classification (it's fine
    // to lose the outer `Context` chain when matching a gRPC code on the innermost variant),
    // but the server log should keep that outer context. `DataFusionError`'s `Display` never
    // contains a source location, so this is simply the error text plus its backtrace (if
    // enabled) — file:line is no longer emitted anywhere, since it only ever came from the
    // `status!` macro's own `file!()/line!()`, which this site stops using.
    let mut full = format!("{desc}: {err} (query_id={query_id})");
    if let Some(plan) = plan {
        let plan_text = format!(
            "{}",
            datafusion::physical_plan::displayable(plan.as_ref()).indent(true)
        );
        full.push_str(&format!("\nphysical plan:\n{}", truncate_plan_text(&plan_text)));
    }
    full
}

fn error_or_warn_log(
    code: tonic::Code,
    desc: &str,
    err: &DataFusionError,
    query_id: &str,
    plan: Option<&Arc<dyn ExecutionPlan>>,
) {
    let full = build_log_line(desc, err, query_id, plan);
    match error_class(code) {
        "internal" => error!("{full}"),
        _ => warn!("{full}"),
    }
}

fn truncate_plan_text(text: &str) -> String {
    if text.len() <= MAX_PLAN_CHARS {
        text.to_string()
    } else {
        // Byte-offset slicing (`&text[..MAX_PLAN_CHARS]`) panics if the offset lands
        // mid-character — plan text can embed non-ASCII string literals from query
        // predicates via `ScalarValue`'s `Display` impl. Slice on a char boundary instead.
        let end = text
            .char_indices()
            .nth(MAX_PLAN_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        format!("{}... (truncated)", &text[..end])
    }
}

pub fn client_error(
    desc: &str,
    err: DataFusionError,
    query_id: &str,
    plan: Option<&Arc<dyn ExecutionPlan>>,
) -> Status {
    let code = classify_datafusion_error(&err);
    let mut msg = format!("{desc}: {}", err.find_root().strip_backtrace());
    let mut has_span = false;
    if let Some(diag) = err.diagnostic() {
        if let Some(span) = diag.span {
            has_span = true;
            msg.push_str(&format!(
                " (at line {}, column {})",
                span.start.line, span.start.column
            ));
        }
        for note in &diag.notes {
            msg.push_str(&format!("\nnote: {}", note.message));
        }
        for help in &diag.helps {
            msg.push_str(&format!("\nhelp: {}", help.message));
        }
    }
    // `msg` at this point — desc, root-error text, optional span/notes/helps — is exactly
    // what both the client and the audit record get; the physical plan never joins it (see
    // below). Append `query_id` last so it's the fixed, greppable suffix on every message.
    msg.push_str(&format!(" (query_id={query_id})"));
    // The plan (when there's no span to show instead) goes only to the server log — never
    // into `msg` — since `displayable(plan)` can render object-store partition paths (see
    // the leak concern above). Passing `None` here for the span case also means a
    // plan-time error with a diagnostic span never bothers rendering the plan at all.
    error_or_warn_log(code, desc, &err, query_id, if has_span { None } else { plan });
    Status::new(code, msg)
}
```

The full `err` (with DataFusion's own backtrace if enabled, and — unlike `find_root()` — any
outer `Context` chain intact) plus `desc`, `query_id`, and (when there's no diagnostic span)
the physical plan text all get logged server-side at `warn!`/`error!` per the classified code.
The *client-facing* `Status` message never had a file:line/build-path suffix to drop in the
first place once these sites stop going through `status!` — there's simply nothing left to
strip. No `--remap-path-prefix` build change is needed since `file!()`/`line!()` are no longer
embedded anywhere in this path; a source-location tag could still be added to the server-side
log later, but that's not required for this plan.

The immediate `execute_stream(plan, task_ctx)` call failure (`:461-464`) is a plain
`DataFusionError` — same as the other four sites — and goes straight to `client_error(...)`.
Only the per-batch site (`:498`), downstream of the `FlightError::ExternalError(Box::new(e))`
re-wrap at `:466`, deals with a `FlightError`; recover the `DataFusionError` from it first,
forwarding the same optional plan:

```rust
fn classify_flight_error(
    err: FlightError,
    query_id: &str,
    plan: Option<&Arc<dyn ExecutionPlan>>,
) -> Status {
    match err {
        FlightError::ExternalError(inner) => match inner.downcast::<DataFusionError>() {
            Ok(df_err) => client_error("error building data stream", *df_err, query_id, plan),
            Err(inner) => {
                error!("error building data stream: {inner} (query_id={query_id})");
                Status::internal(format!(
                    "error building data stream: {inner} (query_id={query_id})"
                ))
            }
        },
        other => {
            error!("error building data stream: {other} (query_id={query_id})");
            Status::internal(format!(
                "error building data stream: {other} (query_id={query_id})"
            ))
        }
    }
}
```

Every branch logs exactly once before returning — the `Ok(df_err)` branch via `client_error`'s
own `error_or_warn_log` call, the other two branches directly — so `classify_flight_error`
is a single, unconditional log point for this site regardless of which branch is taken.

**Ownership note for wiring this in:** by the time `plan` (the local in `execute_query`) is
moved into `execute_stream(plan, task_ctx)` at `:461`, it's gone — and `audit_state` itself
is later moved into `CompletionTrackedStream::new` at `:500`, so the `:498` closure can't
borrow `audit_state.plan` either. Clone what's needed into independent bindings *before*
those moves: `let plan_for_errors = plan.clone();` right after `audit_state.plan =
Some(plan.clone())` at `:460`, and a `let query_id_for_stream = query_id.clone();` — then
both closures (`:461-464` and `:498`) capture their own clones instead of borrowing `plan`
or `audit_state`.

### 3. Query id

Mint one `Uuid::new_v4()` (already a workspace dependency, used elsewhere e.g.
`http_gateway.rs`) as `let query_id = Uuid::new_v4().to_string();`, the very first statement
in `execute_query` — before the UTF-8 parse of the SQL (`:296`), the two range-header
datetime parses, and attribution resolution, all of which precede `audit_state`/
`QueryAuditState` construction (`:370-388`) where `query_id` is stored. Placed there,
`query_id` is available before every fallible step in `execute_query`, so it's Always
populated (not `Option`) on `QueryAuditState`, include it in every client-facing `Status`
message built by `client_error`/`classify_flight_error`, and add it to `QueryAuditRecord` so
the log line for a given failure can be found by grepping the id the caller was handed.

### 4. Audit record / log severity split

Add `error_class: Option<&'static str>` (`"user" | "resource" | "internal"`, with
`#[serde(skip_serializing_if = "Option::is_none")]` so it's omitted when `status == "ok"`,
matching the existing `name`/`range_begin`/`error` pattern in `query_audit.rs`) to
`QueryAuditRecord`, derived once from the `tonic::Code` the same way in both places that need
it:

```rust
fn error_class(code: tonic::Code) -> &'static str {
    match code {
        tonic::Code::InvalidArgument | tonic::Code::Unimplemented => "user",
        tonic::Code::ResourceExhausted => "resource",
        _ => "internal",
    }
}
```

- In `CompletionTrackedStream::poll_next` (`:204-238`), read `err.code()` off the `Status`
  already flowing through the stream to derive the class for `QueryAuditState::emit`, but
  **drop `poll_next`'s own `error!("stream error occurred: ...")` log statement**. Every
  `Status` reaching `poll_next` was already logged once by `classify_flight_error` at `:498`
  (the sole `map_err` feeding this stream) when it built that `Status` — keeping both log
  statements would print two lines per execution-time/per-batch error. `classify_flight_error`
  is the single log point for this site; `poll_next` only reads the code, it doesn't log. Also
  change its existing `audit_state.emit("error", Some(err.to_string()), ...)` call to
  `Some(err.message().to_string())` — `err` here is the `Status` itself, and `Status`'s
  `Display` wraps the message in its own `code: ..., message: ...` framing, whereas
  `.message()` is the same short string `client_error` built (no plan text, per Design §2),
  matching what Step 6's other four `emit` sites pass.
- In every early-setup failure `map_err` closure (`:395-463`), the `Status` was just built by
  `client_error`, so `status.code()` gives the same answer for free.

`QueryAuditState::emit` (`query_audit.rs` / `flight_sql_service_impl.rs:120-161`) gains an
`error_class: Option<&'static str>` parameter, threaded into the new `QueryAuditRecord`
field.

### 5. UDF error convention (datafusion-extensions)

Fix the specific misclassifications that would otherwise make the new mapping useless for
UDF-triggered errors — convert `internal_err!` to `exec_err!` (still `DataFusionError`, just
the `Execution` variant instead of `Internal`) at the sites that are unambiguously caller
mistakes:

- Every `"wrong number of arguments to X()"` arity check (16 sites across
  `jsonb/*.rs`, `color/*.rs`, `math/*.rs`, `properties/*.rs`, `binning/bin_center.rs`).
- Every `"unsupported input type"` / `"unsupported dictionary value type"` type check (the
  `get.rs`/`array_length.rs`/`cast.rs`/`keys.rs`/`parse.rs` input-type guards, and
  `properties_udf.rs`/`property_get.rs` equivalents), plus five caller-triggered sites in
  `properties_udf.rs` that fall outside the arity/type-check framing above:
  `PropertiesToArray::return_type`'s "expects a Dictionary input type" (`:86`),
  `PropertiesToArray::invoke_with_args`'s "expects exactly one argument" (`:93`) and "does not
  support scalar inputs" (`:119`), and `PropertiesLength::invoke_with_args`'s "expects exactly
  one argument" (`:161`) and "does not support scalar inputs" (`:344`).

Leave `"arrays of different lengths in X()"` and `"Dictionary key index out of bounds"` as
`internal_err!` — those indicate an invariant violation within a single Arrow batch (all
columns of a batch must have equal length; a dictionary index must be in range for its own
values array), not something a caller's SQL text can directly cause.

### 6. Request-metadata parsing sites

Of the non-`DataFusionError` `status!` sites in `execute_query`, six are unambiguously
caller-supplied input with no plausible server-side origin: the `query_range_begin`/
`query_range_end` header parses (`:316`, `:318`, `:322`, `:324`) and the `limit` header
parses (`:432` `to_str()`/`ToStrError`, `:436` `usize::from_str`/`ParseIntError`) — all six
parse gRPC metadata values the caller set directly, and none is a `DataFusionError` or an
`anyhow::Error`. Convert these six to `Status::invalid_argument` via a small macro
paralleling `status!`:

```rust
macro_rules! client_input_error {
    ($desc:expr, $err:expr) => {
        Status::invalid_argument(format!("{}: {}", $desc, $err))
    };
}
```

The ticket-decode site (`TicketStatementQuery::decode`, `:527-528`) and the SQL-statement-handle
UTF-8 parse it feeds (`:296-297`) stay on `status!`/`Status::internal`, unchanged. Both operate on
a ticket this server itself produced and encoded (`get_flight_info_statement`, `:542-546`) — the
caller is expected to round-trip it unmodified, not construct it. A decode failure here is at
least as likely to come from a version-skew between pods during a rolling deployment (same
server code, different build encoding/decoding the ticket) as from a caller corrupting or
forging one — there isn't enough certainty to classify it as the caller's mistake, so it's not
reclassified.

### Sequence (single query, error path)

```
client                    execute_query                  classify/emit
  │  DoGet(ticket)              │                              │
  │ ───────────────────────────>│                              │
  │                              │  ctx.sql / limit / plan /    │
  │                              │  execute_stream / stream item│
  │                              │  DataFusionError ────────────>│ find_root() + match
  │                              │                              │  -> tonic::Code
  │                              │<───────── Status{code,msg}───│  -> client_error() adds
  │                              │                              │     query_id (no plan text)
  │  Status{InvalidArgument,     │                              │
  │   "...: <msg> (query_id=…)"}│                              │  -> audit_state.emit(
  │<─────────────────────────────│                                    "error", err_class="user")
```

## Implementation Steps

**Phase 1 — core classification (`rust/public/src/servers/`)**
0. `rust/analytics/src/lakehouse/query.rs`: add
   `.set_bool("datafusion.sql_parser.collect_spans", true)` to `make_session_context`'s
   `SessionConfig` so plan-time `Diagnostic`s actually carry a `Span` (see "Confirmed via
   DataFusion 54.1 source" and Design §2).
1. `flight_sql_service_impl.rs`: add `classify_datafusion_error`, `client_error`,
   `classify_flight_error`, `error_class`, `build_log_line`, `error_or_warn_log`, and
   `client_input_error!` helpers near the existing `status!` macro. `classify_datafusion_error`,
   `client_error`, and `build_log_line` are `pub` so step 9's external integration tests can
   call them directly. `error_or_warn_log(code, desc, &err, query_id, plan)`
   maps `error_class(code)` to `warn!` (for `"user"`/`"resource"`) or `error!` (for
   `"internal"`), logging `desc`, `err` itself (not `err.find_root()` — this keeps any outer
   `Context` chain, untruncated, with backtrace intact if enabled; there is no file:line to
   keep, since it was never part of `DataFusionError`'s `Display` — it only ever came from the
   `status!` macro's own `file!()/line!()`, which these sites stop using), `query_id`, and —
   when `plan` is `Some` — the truncated physical plan text, all server-side only.
   `client_input_error!` is Design §6's new macro, returning `Status::invalid_argument`
   directly (no `DataFusionError`/`error_class`/audit-log involvement — these sites run before
   `query_id`/`audit_state` exist).
2. Replace the five `DataFusionError`-producing error sites in `execute_query` (`:423`,
   `:440`, `:456` via the `status!` macro; `:461-464` via its own hand-rolled
   `Status::internal(...)`; `:498` via the `flight_data_stream` per-batch error path) with
   the new helpers. Also convert the six caller-input header-parsing sites — `query_range_begin`/
   `query_range_end` (`:316`, `:318`, `:322`, `:324`) and `limit` (`:432`, `:436`) — from
   `status!` to `client_input_error!`, per Design §6.
   Leave every other
   `status!` site (ticket decode `:527-528`, the SQL-statement-handle UTF-8 parse it feeds
   `:296-297`, `make_session_context`, `scoped_runtime`,
   schema encoding, prepared-statement handling) untouched and stay `Status::internal`. Most
   of these aren't `DataFusionError`s (or, per Design §6, aren't confidently classifiable as
   caller mistakes). The one exception is `do_action_create_prepared_statement`'s
   `ctx.sql(&query.query)` call (`:889`), which *is* a `DataFusionError` from caller SQL —
   it's left out of scope not because of its error type but because prepared-statement
   *execution* (`do_get_prepared_statement`) is unimplemented (`api_entry_not_implemented!`),
   so this plan's audit-log/query-id machinery has nothing to attach a classification to on
   that path; reclassifying just that one call is a follow-up, not bundled here.
3. Mint `query_id` as the first statement of `execute_query` (before the UTF-8 parse of the
   SQL), thread it through `QueryAuditState` and into every `client_error`/
   `classify_flight_error` call at the five sites above. It is therefore available for every
   failure inside `execute_query`; the one failure genuinely unaffected is the ticket-decode
   `status!` site in the calling `do_get_fallback` (`:527-528`), which happens before
   `execute_query` is even entered and already has a distinct log trail.
4. Wire the physical-plan fallback described in Design §2: clone `plan`/`query_id` before
   the `:461`/`:500` moves, and pass `Some(&plan_for_errors)` into the `client_error`/
   `classify_flight_error` calls at the `execute_stream` (`:461-464`) and per-batch (`:498`)
   sites. The three planning-phase sites (`:423`, `:440`, `:456`) pass `None` — the physical
   plan doesn't exist yet when those can fail. This plan text only ever reaches the
   server-side log (via `error_or_warn_log`, and only when there's no diagnostic span) — it
   is never appended to the `Status` message `client_error` returns, so it never reaches the
   caller regardless of `error_class`.
5. Add `error_class` to `QueryAuditState`/`QueryAuditRecord` (`query_audit.rs`); update
   `CompletionTrackedStream::poll_next` to derive the class from `err.code()` and pass it to
   `emit`, and remove its own `error!("stream error occurred: ...")` log statement — per
   Design §4, `classify_flight_error` is now the sole log point for this site, so `poll_next`
   reads the code without logging again. Two more `emit` call sites take the new parameter
   with a plain `None` (no error to classify): `poll_next`'s success branch
   (`state.emit("ok", None, ...)` at `:231`) and `CompletionTrackedStream::Drop`'s
   `state.emit("incomplete", None, ...)` at `:193`.
6. `QueryAuditState::emit` gaining a non-optional `error_class` parameter means every
   `audit_state.emit(...)` call site must be updated to pass one — there are 8 inside
   `execute_query`'s early-failure `map_err` closures (`:396`, `:414`, `:424`, `:432`, `:436`,
   `:441-444`, `:457`, `:463`). Today every one of these calls `emit(...)` *before* building
   the `Status`/hand-rolled error it returns, so deriving the class "for free" from
   `status.code()` isn't possible as-is. Reorder each closure: build the `Status` first (via
   `status!`, `client_input_error!`, `client_error`, or `classify_flight_error` as appropriate
   for that site), derive `let error_class = error_class(status.code());`, then call
   `audit_state.emit("error", Some(...), Some(error_class))`, then return the `Status`. For
   the two sites that stay on the `status!` macro (`:396`, `:414` — the `anyhow`,
   non-`DataFusionError` setup-failure sites, always `Status::internal`), this always yields
   `error_class = "internal"`. The two sites that move to `client_input_error!` per Design §6
   (`:432`, `:436`) yield `error_class = "user"` (`Status::invalid_argument`) the same way.
   All four cases share the same build-then-derive-then-emit ordering, keeping every site
   consistent and compiling. For the four sites that switch to `client_error`/
   `classify_flight_error` (`:424`, `:441-444`, `:457`, `:463`), `err`/`e` is moved into that
   call (both take `DataFusionError`/`FlightError` by value, and `DataFusionError` isn't
   `Clone`), so it isn't available afterward to build a separate message the way today's code
   does. Pass `Some(status.message().to_string())` — the already-built client-facing
   message — as `emit`'s error argument for these four sites instead of re-deriving a fresh
   string from the moved value. This is safe to put in `QueryAuditRecord.error` precisely
   because `client_error` never appends the physical plan to `status.message()` (Design §2) —
   the message here is desc + root-error text + optional span/notes/helps + `query_id`, not
   the multi-kilobyte plan dump, which stays confined to the server log.

**Phase 2 — UDF convention (`rust/datafusion-extensions/src/`)**
7. Convert the arity/type-check `internal_err!` sites listed in Design §5 to `exec_err!`
   across `jsonb/*.rs`, `color/*.rs`, `math/*.rs`, `properties/*.rs`,
   `binning/bin_center.rs`. Each touched file imports the macro explicitly (e.g.
   `use datafusion::common::{Result, internal_err};` in `jsonb/format_json.rs:4`,
   `jsonb/get.rs:5`, `color/rgba.rs:3`, `math/lerp.rs:3`, `properties/properties_udf.rs:5`,
   `binning/bin_center.rs:3`, …) — add `exec_err` to every one of those import lists. Two
   files have no remaining `internal_err!` use after the conversion (`jsonb/format_json.rs`,
   whose only site is `:54`, and `jsonb/path_query.rs`, whose only site is `:27`); drop
   `internal_err` from their imports so `cargo clippy --workspace -- -D warnings` (Testing
   Strategy #2) doesn't fail on an unused import.

**Phase 3 — tests**
8. `rust/public/tests/query_audit_tests.rs`: add `error_class`/`query_id` to the
   `full_record`/omits-optionals fixtures; add a case asserting `error_class` is omitted (or
   `None`) for `status: "ok"`.
9. New `rust/public/tests/flight_sql_error_classification_tests.rs` (or extend an existing
   integration test if one already drives `FlightSqlServiceImpl` end-to-end — none currently
   does, per `find rust -iname "*flight_sql*test*"`): unit-test `classify_datafusion_error`
   and `client_error` directly against constructed `DataFusionError` values for each
   branch of the table (`SQL`, `Plan`, `SchemaError`, `Execution`, `ResourcesExhausted`,
   `NotImplemented`, `Internal`, wrapped in `Context`/`Diagnostic`/`Collection`), asserting
   both the resulting `tonic::Code` and that the message contains no `.rs:` file:line
   pattern. Add a case specifically for Design §2's span handling: a `Diagnostic`-carrying
   error with `plan: Some(...)` asserts the message contains the span text and *not* a
   `"physical plan:"` section — `client_error`'s returned `Status` message never contains a
   `"physical plan:"` section for *any* input, since that text is server-log-only (Design §2);
   assert this for a plain `Execution` error with `plan: Some(...)` too. Cover the plan-text
   path itself with separate tests against `build_log_line` (the pure helper
   `error_or_warn_log` delegates to): one asserting its output contains a `"physical plan:"`
   section with the fake plan's `DisplayAs` output when `plan: Some(...)`, one asserting it's
   absent when `plan: None`, and one feeding a fake plan whose `DisplayAs` emits >
   `MAX_PLAN_CHARS` of text to assert truncation at that boundary. These tests define their
   own fake `ExecutionPlan` with a real `DisplayAs` impl (plus a variant that emits the
   over-limit text) rather than reusing `query_audit_tests.rs`'s `FakeExec`, whose
   `DisplayAs::fmt_as` is `unimplemented!()` — it was never written to be displayed. Register
   the new file in `rust/public/Cargo.toml`
   with a matching `[[test]]` block (same pattern as `query_audit_tests`):
   ```toml
   [[test]]
   name = "flight_sql_error_classification_tests"
   path = "tests/flight_sql_error_classification_tests.rs"
   required-features = ["server"]
   ```
10. `python/micromegas/tests/test_perfetto_integration.py::test_perfetto_trace_chunks_error_handling`:
   all three cases (invalid span type, missing arguments — both `plan_err!` — and
   non-existent process, an `Execution` error per
   `perfetto_trace_execution_plan.rs:189-208`) currently assert
   `pytest.raises(flight.FlightInternalError)`. After Phase 1 these become
   `InvalidArgument`, which pyarrow surfaces as `pyarrow.lib.ArrowInvalid`. Add `import pyarrow`
   alongside the existing `import pyarrow._flight as flight` (the latter alone doesn't bind
   the `pyarrow` name), then update all three assertions to
   `pytest.raises(pyarrow.lib.ArrowInvalid)` (the message-content assertions are unaffected —
   same text, just no file:line/query_id noise to strip in the test).

**Phase 4 — docs**
11. `mkdocs/docs/query-guide/query-audit-log.md`: add `error_class`/`query_id` rows to the
    `## Fields` table (`query_id` always present; `error_class` present on error, matching the
    existing `error` row's "on error" convention); add a "queries grouped by `error_class`"
    example matching the doc's existing style (`jsonb_get(j, 'error_class')`).
12. `mkdocs/docs/gateway/index.md`: the gateway (`rust/public/src/servers/http_gateway.rs:346-362`)
    downcasts the FlightSQL client error to `tonic::Status` and maps `Code::InvalidArgument`
    to `GatewayError::BadRequest` (HTTP 400) — since the gateway's query path
    (`client.query(...)` → `do_get_fallback` → `execute_query`) runs through the same
    reclassification, a bad SQL query or other `InvalidArgument`-mapped error now surfaces as
    400 instead of 500. Update the `### Error Handling` table (`:159-166`): change the
    "500 Internal Error" row's "When" column from "SQL syntax error, execution failure" to
    just "server-side execution failure" and add a "400 Bad Request" row's "When" cell to
    also cover "invalid/unsupported SQL (syntax error, unknown column/function, etc.)"
    alongside the existing "Empty SQL, query too large (>1MB)" case.

## Files to Modify

- `rust/public/src/servers/flight_sql_service_impl.rs` — classifier, message builder,
  query id, five call sites, `CompletionTrackedStream` log-severity split.
- `rust/analytics/src/lakehouse/query.rs` — enable `datafusion.sql_parser.collect_spans` in
  `make_session_context`'s `SessionConfig` so plan-time `Diagnostic`s carry a `Span`.
- `rust/public/src/servers/query_audit.rs` — `error_class`/`query_id` fields on
  `QueryAuditRecord`.
- `rust/public/tests/query_audit_tests.rs` — fixture updates for the two new fields.
- `rust/public/tests/flight_sql_error_classification_tests.rs` — new.
- `rust/public/Cargo.toml` — register the new test file with a `[[test]]` block
  (`required-features = ["server"]`), matching the existing `query_audit_tests` entry.
- `rust/datafusion-extensions/src/jsonb/{format_json,get,array_length,cast,keys,parse,path_query}.rs`,
  `rust/datafusion-extensions/src/color/{color_scale,lerp_color,rgba}.rs`,
  `rust/datafusion-extensions/src/math/{lerp,unlerp}.rs`,
  `rust/datafusion-extensions/src/properties/{properties_udf,property_get}.rs`,
  `rust/datafusion-extensions/src/binning/bin_center.rs` — `internal_err!` → `exec_err!` for
  arity/type-check sites only.
- `python/micromegas/tests/test_perfetto_integration.py` — updated exception type in three
  assertions.
- `mkdocs/docs/query-guide/query-audit-log.md` — add `error_class`/`query_id` rows to the
  `## Fields` table; add a "queries grouped by `error_class`" example.
- `mkdocs/docs/gateway/index.md` — update the `### Error Handling` table: bad/unsupported SQL
  now maps to 400, not 500.

## Trade-offs

- **Where to classify.** Classifying once in `flight_sql_service_impl.rs` (rather than
  pushing gRPC-code knowledge into `datafusion-extensions`, which has no tonic dependency
  and shouldn't need one) keeps the gRPC-facing concern at the gRPC-facing boundary. The
  UDF-side change (Phase 2) is deliberately narrow: fix the `DataFusionError` *variant*
  chosen at the source, not teach the UDF crate about gRPC codes.
- **`Execution` defaults to `InvalidArgument`.** As the issue notes, `Execution` is a
  grab-bag that DataFusion itself also uses for some internal-ish failures. Given the
  UDF-convention fix in Phase 2 and that `perfetto_trace_execution_plan.rs` already reserves
  `Execution` for caller-triggered failures (bad process id, bad span type reaching
  execution), defaulting to `InvalidArgument` is right in practice; a genuine internal bug
  that happens to surface as `Execution` will now read as `InvalidArgument` instead of
  `Internal`, which is the same trade-off the issue explicitly accepts.
- **`ResourcesExhausted` → gRPC `ResourceExhausted(8)`.** This is the semantically correct
  gRPC code, but per the confirmed pyarrow mapping it surfaces client-side as
  `FlightUnavailableError`, which reads as "transient, retry-safe" — the opposite of "this
  query needs a smaller scan/limit." No gRPC code maps to something better in pyarrow's
  client (there is no Flight-specific "resource exhausted" exception). Documented under Open
  Questions rather than worked around, since a workaround (e.g. reusing `InvalidArgument`
  for this case) would lose the distinct `error_class: "resource"` signal in the audit log
  for no real gain client-side today. A future typed-exception wrapper (issue item 6) that
  inspects the audit log or a structured detail payload rather than the bare gRPC code could
  revisit this.
- **Physical-plan fallback is server-log-only, and a locator rather than an attribution.**
  `displayable(plan).indent(true)` renders `FileScanConfig::fmt_as`'s `file_groups=` section,
  which prints each scanned file's `object_meta.location` — Micromegas view scans are exactly
  this shape (`DataSourceExec`/`ParquetSource` plans built in
  `rust/analytics/src/lakehouse/partitioned_execution_plan.rs`), so the plan text can reveal
  internal lakehouse object-store partition paths. `client_error` therefore never puts it in
  the `Status` message it returns; it only reaches `error_or_warn_log` (capped at
  `MAX_PLAN_CHARS`, truncated with a marker), so it's visible to whoever can read the server
  log, keyed by `query_id`, never to the caller. Even server-side, it's a locator, not an
  attribution: nothing in DataFusion tags which `ExecutionPlan` node actually raised a given
  error, so the plan only gives the plan's shape to eyeball against the error message — e.g.
  spotting the `jsonb_format_json(...)` expression text in a `ProjectionExec` line. It's
  strictly better than nothing for the execution-time (no-`Diagnostic`) case, but it's not as
  precise as the line/column a plan-time `Diagnostic` span gives, and it costs the caller the
  plan shape they'd otherwise see directly — they get the `query_id` to hand to whoever has
  server-log access instead.
- **`Configuration` → `InvalidArgument`; `ArrowError` stays `Internal`.** `Configuration` is
  reachable from a caller's own `SET datafusion.…` statement, so it joins
  `SQL`/`Plan`/`SchemaError`/`Execution` under `InvalidArgument`. `ArrowError` is left in the
  `_ => Internal` bucket even though some `ArrowError`s are caller-triggered (e.g. a cast
  failure or divide-by-zero on user data): `DataFusionError::ArrowError`'s `source()` is the
  `ArrowError` itself, not a `DataFusionError`, so `find_root()` returns it directly with no
  further variant available to separate a caller mistake from a genuine internal Arrow-kernel
  bug. Splitting that further would mean matching on `arrow::error::ArrowError`'s own variants,
  which is out of scope for this pass.
- **Not touching `scoped_runtime`/`make_session_context` errors.** These are `anyhow::Error`
  setup failures (bad config, storage unreachable), not something caller SQL can trigger;
  reclassifying them is out of scope and they correctly stay `Internal`.
- **Not adding `Status::with_details`/structured error payload.** The issue marks this
  ("item 6") as a follow-up pending verification that pyarrow surfaces gRPC status details on
  this code path at all — worth a spike before committing engineering time, not bundled
  here.
- **Not building the Python typed-exception hierarchy.** Same follow-up item; the `client_error`/`error_class`
  work here is what such a hierarchy would need to key off of (gRPC code +
  `error_class`), but wrapping `pyarrow._flight.Flight*Error`/`ArrowInvalid` in
  micromegas-specific exception types is a separate, python-side change.
- **Not implementing the `EXPLAIN`/`LIMIT 0` cheap-validation option.** The issue calls this
  out as "separate from reporting" — a client-side/API-shape feature, not a reporting fix.
  Left for a follow-up plan.
- **HTTP gateway status codes change too, not just the gRPC path.** The gateway
  (`http_gateway.rs:346-362`) maps FlightSQL `Code::InvalidArgument` to `GatewayError::BadRequest`
  (HTTP 400) already; since the gateway's query path runs through `execute_query`, every error
  reclassified from `Internal` to `InvalidArgument` by this plan now comes back as HTTP 400
  instead of 500 for gateway/web-app callers, not just for direct FlightSQL clients. This is
  the same "you wrote a bad query" vs. "the server broke" distinction the issue asks for,
  applied for free at the HTTP boundary — treated here as a welcome side effect, not a
  separate feature, but it does mean `mkdocs/docs/gateway/index.md`'s error table needs
  updating (Phase 4, step 12) since it documents the old 500-for-everything behavior.
  `analytics-web-app/src/lib/arrow-stream.ts:172-194` surfaces any non-401/403 status
  generically, so the web app's behavior doesn't break, but its displayed status code changes.
- **Header parsing gets `InvalidArgument`; ticket decode doesn't.** Both are non-`DataFusionError`
  `status!` sites, but they differ in how certain the caller-mistake attribution is. The
  `query_range_begin`/`query_range_end` and `limit` header values are set by the caller
  directly, so a parse failure is unambiguously theirs. The ticket, by contrast, is an opaque token this
  server itself issues and expects back unmodified — a decode failure is at least as
  plausibly a version-skew bug across pods in a rolling deployment as a caller-corrupted
  ticket. Classifying it `InvalidArgument` anyway would blame the caller in cases that
  could be a server-side bug, so it stays `Internal` until there's a way to tell the two
  apart with more confidence (see Design §6).

## Testing Strategy

1. `cargo test -p micromegas --features server` (package name confirmed in
   `rust/public/Cargo.toml`; the crate's `default = []` and every `[[test]]` block —
   including the one step 9 adds — is `required-features = ["server"]`, so plain
   `cargo test -p micromegas` silently skips all of them) covering the new
   `flight_sql_error_classification_tests.rs` unit tests and the updated
   `query_audit_tests.rs` fixtures.
2. `cargo clippy --workspace -- -D warnings` and `cargo fmt` per `rust/CLAUDE.md`.
3. Manual repro from the issue: start services (`start_services.py`), run
   `micromegas-query "SELECT jsonb_format_json(property_get(properties, 'build-version')) FROM processes LIMIT 1"`
   and confirm the error now reads as an `InvalidArgument`/`ValueError`-shaped failure with
   no absolute build path, no `.rs:` file:line, and no `physical plan:` section in what the
   CLI prints; then `tail /tmp/analytics.log` (or the relevant service log) for that query's
   `query_id` and confirm the server-side log line for it *does* carry a `physical plan:`
   section (this is the no-`Diagnostic`, execution-time case Design §2's fallback targets) —
   and that the `flightsql_query_audit` log line for that query has `status: "error"`,
   `error_class: "user"`, and the same `query_id`, with no `physical plan:` text in its
   `error` field. Also try a plan-time typo (e.g. `SELECT no_such_function(properties) FROM
   processes`) and confirm that one instead gets a line/column span, with no `physical plan:`
   section anywhere — client, server log, or audit record.
4. Same for a query that exceeds the per-query memory budget (if a low-enough test budget
   can be configured) to confirm `ResourceExhausted`/`error_class: "resource"`.
5. `poetry run pytest` in `python/micromegas/` (per `python/CLAUDE.md`) including the updated
   `test_perfetto_integration.py`, which requires live services per its existing setup.

## Open Questions

None remaining. The one open question from earlier drafts — whether non-`DataFusionError`
`status!` sites should also move to `Status::invalid_argument` — is resolved in Design §6: the
header-parsing sites do (unambiguously caller-supplied input), the ticket-decode site doesn't
(not confidently attributable to the caller; see the Trade-offs entry).
