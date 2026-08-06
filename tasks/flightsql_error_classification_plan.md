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
non-actionable file:line/path suffix from client-facing messages, keeps that detail (plus a
new query id) in the server log instead, and splits the query-audit log's severity and
`status` field so user errors stop being counted as service failures.

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

`execute_query` (`flight_sql_service_impl.rs:289-504`) is the one path that matters here —
it's the only one that runs arbitrary user SQL. Its error sites, in order:

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
`rust/datafusion-extensions/src/jsonb/format_json.rs:51` (`internal_err!("wrong number of
arguments...")`) and `:57-59` (`DataFusionError::Execution` for a type mismatch, which is
the exact case in the issue's repro). `DataFusionError::Internal` is defined by DataFusion
as "this should not happen — please file a bug" (see
`datafusion-common-54.1.0/src/error.rs:93-109`), so an `internal_err!` on a plain arity
check is a misclassification independent of this issue, and it would defeat the new gRPC
mapping if left alone (every `jsonb_*` call with the wrong number of args would still come
back `Internal`). By contrast, `rust/analytics/src/lakehouse/perfetto_trace_table_function.rs`
already gets this right — it uses `plan_err!` for all four of its argument-parsing checks.

### Confirmed via DataFusion 54.1 source (`datafusion-common-54.1.0/src/error.rs`)

- `DataFusionError::find_root()` (`:436`) walks the `Error::source()` chain and returns the
  innermost `DataFusionError`, unwrapping `Context`, `Diagnostic`, `Collection`, and
  `Shared` uniformly. This is the one place classification needs to happen — call
  `find_root()` once, match on the result.
- `DataFusionError::diagnostic()` (`:609`) returns the outermost `Diagnostic`, which (per
  the issue) carries a `Span` with `start`/`end` `Location { line, column }` into the SQL
  text for many plan-time errors (unknown column, ambiguous reference, type mismatch).
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
| `ResourceExhausted(8)` | `pyarrow._flight.FlightUnavailableError` — gRPC `RESOURCE_EXHAUSTED` maps to Arrow Flight's internal `kUnavailable` transport code (`util_internal.cc:105-107`), which becomes an `IOError`-family exception carrying `FlightStatusDetail::Unavailable`. See **Open Questions** — this reads as "transient, safe to retry," which is exactly wrong for "this query needs too much memory." |

Verified with `python3 -c "import pyarrow.flight as f; ..."` (pyarrow 20.0.0) that no
`FlightInvalidArgument`/`FlightResourceExhausted` classes exist client-side — confirming the
table above is the actual, not merely documented, behavior.

## Design

### 1. Central classifier

Add one function near the top of `flight_sql_service_impl.rs`, replacing the `status!`
macro's blanket `Status::internal` for the five `DataFusionError`-producing sites:

```rust
fn classify_datafusion_error(err: &DataFusionError) -> tonic::Code {
    use datafusion::error::DataFusionError as DFE;
    match err.find_root() {
        DFE::SQL(..) | DFE::Plan(_) | DFE::SchemaError(..) | DFE::Execution(_) => {
            tonic::Code::InvalidArgument
        }
        DFE::ResourcesExhausted(_) => tonic::Code::ResourceExhausted,
        DFE::NotImplemented(_) => tonic::Code::Unimplemented,
        _ => tonic::Code::Internal,
    }
}
```

`find_root` already recurses through `Collection` (returns the classification of the first
collected error, matching `DataFusionError`'s own `error_prefix()`/`message()` convention),
`Context`, `Diagnostic`, and `Shared`. `External` wrapping a non-`DataFusionError` falls
through to the `_ => Internal` arm, per the mapping table in the issue.

### 2. Message construction (drops file:line/path, adds query id, diagnostic span, or plan dump)

Plan-time errors (`SQL`/`Plan`/`SchemaError`) often carry a `Diagnostic` with a `Span` into
the SQL text — a precise line/column the caller can jump to. Execution-time errors (the
`Execution` variant covering the issue's own repro, and most UDF failures once Design §5
lands) never do: `Diagnostic`/`Span` is populated by DataFusion's analyzer/planner, not by a
`ScalarUDFImpl::invoke_with_args` running mid-execution. For that case, the next best
locator is the captured physical plan (`audit_state.plan`, already set at `:460` before
execution starts) — not an exact attribution (nothing tags *which* node raised the error),
but enough for the caller to see the plan shape and spot the operator/expression that lines
up with the message (e.g. a `ProjectionExec` listing the failing `jsonb_format_json(...)`
call). `client_error` takes the plan as an optional extra input and only falls back to it
when there's no span to show:

```rust
const MAX_PLAN_CHARS: usize = 2000;

fn error_or_warn_log(code: tonic::Code, desc: &str, err: &DataFusionError, query_id: &str) {
    let full = format!(
        "{desc}: {} (query_id={query_id})",
        err.find_root() // full detail, backtrace included if enabled — server-side only
    );
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

fn client_error(
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
    if !has_span {
        if let Some(plan) = plan {
            let plan_text = format!("{}", datafusion::physical_plan::displayable(plan.as_ref()).indent(true));
            msg.push_str(&format!("\nphysical plan:\n{}", truncate_plan_text(&plan_text)));
        }
    }
    msg.push_str(&format!(" (query_id={query_id})"));
    error_or_warn_log(code, desc, &err, query_id); // full detail incl. file:line stays server-side
    Status::new(code, msg)
}
```

The full original message (with DataFusion's own backtrace if enabled) plus `desc` and
`query_id` still gets logged server-side at `warn!`/`error!` per the classified code — only
the *client-facing* `Status` message drops the file:line/build-path suffix. No
`--remap-path-prefix` build change is needed since we stop embedding `file!()`/`line!()` in
the client message at all; it can still be added separately for the server-side log if
desired (noted under Open Questions, not required for this plan).

For the `execute_stream`/per-batch site (`:461-466`, `:498`), the value in hand is a
`FlightError`, not a `DataFusionError` — recover it first, forwarding the same optional plan:

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
`http_gateway.rs`) at the top of `execute_query`, store it as `query_id: String` on
`QueryAuditState`, include it in every client-facing `Status` message built by
`client_error`/`classify_flight_error`, and add it to `QueryAuditRecord` so the log line for
a given failure can be found by grepping the id the caller was handed. Always populated
(not `Option`), since it's assigned before any fallible step.

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
  is the single log point for this site; `poll_next` only reads the code, it doesn't log.
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

### Sequence (single query, error path)

```
client                    execute_query                  classify/emit
  │  DoGet(ticket)              │                              │
  │ ───────────────────────────>│                              │
  │                              │  ctx.sql / limit / plan /    │
  │                              │  execute_stream / stream item│
  │                              │  DataFusionError ────────────>│ find_root() + match
  │                              │                              │  -> tonic::Code
  │                              │<───────── Status{code,msg}───│  -> client_error() strips
  │                              │                              │     file:line, adds query_id
  │  Status{InvalidArgument,     │                              │
  │   "...: <msg> (query_id=…)"}│                              │  -> audit_state.emit(
  │<─────────────────────────────│                                    "error", err_class="user")
```

## Implementation Steps

**Phase 1 — core classification (`rust/public/src/servers/`)**
1. `flight_sql_service_impl.rs`: add `classify_datafusion_error`, `client_error`,
   `classify_flight_error`, `error_class`, `error_or_warn_log` helpers near the existing
   `status!` macro. `error_or_warn_log(code, desc, &err, query_id)` maps `error_class(code)`
   to `warn!` (for `"user"`/`"resource"`) or `error!` (for `"internal"`), logging the full
   message — including `desc`, `err.find_root()` (untruncated, with file:line/backtrace
   intact), and `query_id` — server-side only.
2. Replace the five `DataFusionError`-producing error sites in `execute_query` (`:423`,
   `:440`, `:456` via the `status!` macro; `:461-464` via its own hand-rolled
   `Status::internal(...)`; `:498` via the `flight_data_stream` per-batch error path) with
   the new helpers. Leave every other
   `status!` site (ticket decode, header parsing, `make_session_context`, `scoped_runtime`,
   schema encoding, prepared-statement handling) untouched — those aren't
   `DataFusionError`s and stay `Status::internal`.
3. Mint `query_id` in `execute_query`, thread it through `QueryAuditState` and into every
   `client_error`/`classify_flight_error` call at the five sites above (early-setup failures
   before the query_id is available are unaffected — they already have a distinct log
   trail).
4. Wire the physical-plan fallback described in Design §2: clone `plan`/`query_id` before
   the `:461`/`:500` moves, and pass `Some(&plan_for_errors)` into the `client_error`/
   `classify_flight_error` calls at the `execute_stream` (`:461-464`) and per-batch (`:498`)
   sites. The three planning-phase sites (`:423`, `:440`, `:456`) pass `None` — the physical
   plan doesn't exist yet when those can fail.
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
   `status!`, `client_error`, or `classify_flight_error` as appropriate for that site), derive
   `let error_class = error_class(status.code());`, then call
   `audit_state.emit("error", Some(...), Some(error_class))`, then return the `Status`. For
   the four sites that stay on the `status!` macro (`:396`, `:414`, `:432`, `:436` — the
   `anyhow`/non-`DataFusionError` sites, always `Status::internal`), this always yields
   `error_class = "internal"`, but the same build-then-derive-then-emit ordering keeps every
   site consistent and compiling.

**Phase 2 — UDF convention (`rust/datafusion-extensions/src/`)**
7. Convert the arity/type-check `internal_err!` sites listed in Design §5 to `exec_err!`
   across `jsonb/*.rs`, `color/*.rs`, `math/*.rs`, `properties/*.rs`,
   `binning/bin_center.rs`.

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
   pattern. Add two cases specifically for Design §2's fallback: a `Diagnostic`-carrying
   error with `plan: Some(...)` asserts the message contains the span text and *not* a
   `"physical plan:"` section; a plain `Execution` error (no diagnostic) with `plan:
   Some(...)` asserts the reverse — a `"physical plan:"` section containing the fake plan's
   `DisplayAs` output (reuse `query_audit_tests.rs`'s `FakeExec` pattern), truncated at
   `MAX_PLAN_CHARS` when the input exceeds it.
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

## Files to Modify

- `rust/public/src/servers/flight_sql_service_impl.rs` — classifier, message builder,
  query id, five call sites, `CompletionTrackedStream` log-severity split.
- `rust/public/src/servers/query_audit.rs` — `error_class`/`query_id` fields on
  `QueryAuditRecord`.
- `rust/public/tests/query_audit_tests.rs` — fixture updates for the two new fields.
- `rust/public/tests/flight_sql_error_classification_tests.rs` — new.
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
- **Physical-plan fallback is a locator, not an attribution.** Nothing in DataFusion tags
  which `ExecutionPlan` node actually raised a given error, so appending
  `displayable(plan).indent(true)` (capped at `MAX_PLAN_CHARS`, truncated with a marker) only
  gives the caller the plan's shape to eyeball against the error message — e.g. spotting the
  `jsonb_format_json(...)` expression text in a `ProjectionExec` line. It's strictly better
  than nothing for the execution-time (no-`Diagnostic`) case, but it's not as precise as the
  line/column a plan-time `Diagnostic` span gives.
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

## Testing Strategy

1. `cargo test -p micromegas` (package name confirmed in `rust/public/Cargo.toml`) covering
   the new `flight_sql_error_classification_tests.rs` unit tests and the updated
   `query_audit_tests.rs` fixtures.
2. `cargo clippy --workspace -- -D warnings` and `cargo fmt` per `rust/CLAUDE.md`.
3. Manual repro from the issue: start services (`start_services.py`), run
   `micromegas-query "SELECT jsonb_format_json(property_get(properties, 'build-version')) FROM processes LIMIT 1"`
   and confirm the error now reads as an `InvalidArgument`/`ValueError`-shaped failure with
   no absolute build path and no `.rs:` file:line, a `physical plan:` section (this is the
   no-`Diagnostic`, execution-time case Design §2's fallback targets), and that the
   `flightsql_query_audit` log line for that query has `status: "error"`,
   `error_class: "user"`, and a `query_id` that also appears in the client-facing message.
   Also try a plan-time typo (e.g. `SELECT no_such_function(properties) FROM processes`) and
   confirm that one instead gets a line/column span with no `physical plan:` section.
4. Same for a query that exceeds the per-query memory budget (if a low-enough test budget
   can be configured) to confirm `ResourceExhausted`/`error_class: "resource"`.
5. `poetry run pytest` in `python/micromegas/` (per `python/CLAUDE.md`) including the updated
   `test_perfetto_integration.py`, which requires live services per its existing setup.

## Open Questions

1. Should the parsing/attribution `status!` sites that aren't `DataFusionError` (ticket
   decode failures, malformed `query_range_begin`/`query_range_end` headers) also move to
   `Status::invalid_argument`? They're arguably caller mistakes too (malformed request, not
   malformed SQL), but the issue's classification table is scoped to `DataFusionError`
   specifically — flagging as a small, separate follow-up rather than folding it in here.
