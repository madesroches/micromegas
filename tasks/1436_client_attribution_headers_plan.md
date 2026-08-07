# FlightSQL Client Attribution Headers Plan

GitHub issue: [#1436](https://github.com/madesroches/micromegas/issues/1436)

## Overview

Every FlightSQL query from the Python client reports `x-client-type: python` and nothing
else, so a human running `micromegas-query` by hand, a cron job, and an LLM agent driving the
same CLI on someone's behalf are indistinguishable in `flightsql_query_audit`. Agent-driven
querying has a materially different profile (more queries per task, more malformed SQL, more
retries, larger scans), and we currently can't measure any of it.

This plan adds three new headers, resolved once per client instance in
`FlightSQLClient.__init__` and sent on every query:

- `x-client-agent` — who is driving the client (`claude-code`, `none`, or an explicit
  override), detected from ambient environment variables.
- `x-client-entrypoint` — how the client was invoked (`cli-query`, `script`, `jupyter`,
  `repl`), from a closed vocabulary.
- `x-client-session` — an opaque per-instance id that correlates every query from one client
  instance, so per-task/per-session metrics (queries per task, retries after error, bytes
  scanned per session) become possible.

The server reads all three, logs them alongside the existing `client=` field, and adds them
to `QueryAuditRecord`. Both are analytics-only signals — never used for auth, quota, or rate
limiting — and trivially spoofable/omittable, same as the existing `x-client-type`.

## Current State

- `python/micromegas/micromegas/flightsql/client.py:64-88` — `make_call_headers(begin, end,
  preserve_dictionary=False)` is a pure formatting function (no I/O) that builds the header
  list for every FlightSQL call. It's called from three sites: `query()` (`:356`),
  `query_stream()` (`:415`), and `query_arrow()` (`:447`). `x-client-type` is hardcoded to
  `"python"` at `:66`.
- `python/micromegas/tests/test_flightsql_headers.py` is a hermetic unit test that relies on
  `make_call_headers` having no I/O — this plan must preserve that property.
- `FlightSQLClient.__init__` (`client.py:172-233`) takes `uri, headers=None,
  preserve_dictionary=False, auth_provider=None`. It stores `self.__preserve_dictionary` and
  is the natural place to resolve and cache per-instance attribution once.
- Three connection paths construct `FlightSQLClient`, all of which need a way to pass through
  an explicit entrypoint label:
  - `micromegas/__init__.py:13-26` — top-level library `connect()`, the documented entry
    point for library/notebook users. Must **not** force an entrypoint — auto-detection
    should run.
  - `micromegas/oidc_connection.py:91-165` — explicit-args OIDC `connect()`, used directly by
    library callers and by `cli/connection.py`. Constructs `FlightSQLClient` at `:163-165`.
  - `micromegas/cli/connection.py:4-26` — the CLI's `connect(profile=None)`, used by
    `micromegas-query`/`micromegas-screens`. Delegates to `oidc_connection.connect()` (OIDC
    configured) or constructs `FlightSQLClient(cfg.uri)` directly (`:26`).
- `micromegas/cli/query.py:main()` (`:61-164`) is the only CLI entry point that actually
  issues FlightSQL queries — it calls `connection.connect(profile=args.profile)` at `:142`,
  then `client.query(sql, begin, end)` at `:145`.
- `micromegas/cli/screens.py` does **not** call FlightSQL at all — it talks to
  `analytics-web-srv`'s REST API via `micromegas/web_client.py`'s `WebClient` (verified: no
  `FlightSQL`/`.query(` references in `screens.py`). So the issue-comment's proposed
  `cli-screens` entrypoint value has no live call site today — see Open Questions.
  `micromegas/cli/logout.py` never queries either (it only deletes token files), so it needs
  no changes.
- Server side, `rust/public/src/servers/flight_sql_service_impl.rs::execute_query`:
  - `query_id` is already minted first thing (`:508`, `Uuid::new_v4()`, landed via #1435) —
    this already satisfies the "per-request correlation id" half of the issue's `x-request-id`
    tangent; that part needs no further work here.
  - `client_type` is read at `:552-555` (`metadata.get("x-client-type")`, default
    `"unknown"`), logged in the start-of-query `info!` at `:559-574`, and stored on
    `QueryAuditState`/`QueryAuditRecord` (`:589-591`, `query_audit.rs:85`).
- `rust/public/src/servers/query_audit.rs:80-119` — `QueryAuditRecord`, the struct serialized
  as the `flightsql_query_audit` JSON log line. `client: String` is always present, `name`/
  `range_begin`/etc. use `#[serde(skip_serializing_if = "Option::is_none")]`.
- `rust/public/src/servers/http_gateway.rs`:
  - `build_origin_metadata` (`:178-214`) augments `x-client-type` with `+gateway` and
    generates `x-request-id` — this machinery is specific to those two headers and is **not**
    where the new headers belong (per the issue's own reasoning: agent/session/entrypoint are
    "who authored the SQL", not "which hops it took", so they should pass through unchanged,
    not get chained).
  - `HeaderForwardingConfig::default()` (`:40-63`) is the allowlist that actually forwards
    caller headers (`X-User-Name`, `X-Request-ID`, etc.) from the incoming HTTP request to the
    downstream FlightSQL gRPC call (`:335-339`) — this is where the three new headers need to
    be added so they survive a gateway hop.
- `grafana/pkg/flightsql/query_data.go:49` sets `x-client-type: grafana` and nothing else —
  out of scope; Grafana isn't an agent, so the server's `"unknown"` default for the two new
  fields (header simply absent) is the correct behavior there, no Go change needed.
- `mkdocs/docs/query-guide/query-audit-log.md` documents every `QueryAuditRecord` field;
  `mkdocs/docs/query-guide/python-api.md` documents `FlightSQLClient(...)`'s constructor
  signature; `mkdocs/docs/gateway/configuration.md` documents the gateway's default header
  allowlist. All three need updating.

## Design

### Detection lives in a new pure-ish module, `make_call_headers` stays pure

`make_call_headers`'s existing docstring/test contract is "no I/O, hermetically testable."
Environment-variable reads and `sys.modules` inspection aren't file/network I/O, but they *are*
hidden global state that would make the function's output depend on the process environment —
worth keeping out of it anyway, so the existing unit test keeps working unchanged. Detection
moves to a new module, `python/micromegas/micromegas/flightsql/attribution.py`:

```python
import os
import sys
import uuid

_KNOWN_AGENT_HARNESS_ENV_VARS = {
    "CLAUDECODE": "claude-code",
}


def resolve_client_agent():
    """Who is driving this client: an explicit override, a known agent harness
    detected from its marker env var, or "none" for a plain human/script."""
    override = os.environ.get("MICROMEGAS_CLIENT_AGENT")
    if override:
        return override
    for env_var, agent_name in _KNOWN_AGENT_HARNESS_ENV_VARS.items():
        if os.environ.get(env_var):
            return agent_name
    return "none"


def resolve_client_entrypoint(explicit=None):
    """How this client was invoked. `explicit` (set by our own CLI main()s)
    always wins; otherwise an env override, then jupyter/repl detection, then
    "script". Closed vocabulary only -- never sys.argv[0]/__main__.__file__."""
    if explicit:
        return explicit
    override = os.environ.get("MICROMEGAS_CLIENT_ENTRYPOINT")
    if override:
        return override
    if "ipykernel" in sys.modules:
        return "jupyter"
    main_module = sys.modules.get("__main__")
    if sys.flags.interactive or not hasattr(main_module, "__file__"):
        return "repl"
    return "script"


def new_session_id():
    return str(uuid.uuid4())
```

`resolve_client_agent`/`resolve_client_entrypoint` are cheap and stateless, so they're called
once in `FlightSQLClient.__init__` and cached — env vars and interpreter mode don't change
mid-process, so there's no benefit to re-resolving per query, and caching keeps
`make_call_headers` a pure formatter of already-resolved strings.

### `FlightSQLClient` resolves once, `make_call_headers` just formats

`FlightSQLClient.__init__` (`client.py:172`) gains one new parameter and three new private
attributes:

```python
def __init__(
    self, uri, headers=None, preserve_dictionary=False, auth_provider=None,
    client_entrypoint=None,
):
    ...
    self.__client_agent = resolve_client_agent()
    self.__client_entrypoint = resolve_client_entrypoint(explicit=client_entrypoint)
    self.__session_id = new_session_id()
```

`make_call_headers` gains three new optional parameters, each appended as a header only when
not `None` (mirroring how `preserve_dictionary` is conditionally appended today), so the
existing hermetic test — which calls it with just `begin`/`end` — is unaffected:

```python
def make_call_headers(
    begin, end, preserve_dictionary=False,
    client_agent=None, client_entrypoint=None, client_session=None,
):
    call_headers = [
        ("x-client-type".encode("utf8"), "python".encode("utf8")),
    ]
    if client_agent is not None:
        call_headers.append(("x-client-agent".encode("utf8"), client_agent.encode("utf8")))
    if client_entrypoint is not None:
        call_headers.append(
            ("x-client-entrypoint".encode("utf8"), client_entrypoint.encode("utf8"))
        )
    if client_session is not None:
        call_headers.append(
            ("x-client-session".encode("utf8"), client_session.encode("utf8"))
        )
    ... # existing begin/end/preserve_dictionary logic, unchanged
```

The three call sites (`query()` `:356`, `query_stream()` `:415`, `query_arrow()` `:447`) pass
the cached attributes:

```python
call_headers = make_call_headers(
    begin, end, self.__preserve_dictionary,
    self.__client_agent, self.__client_entrypoint, self.__session_id,
)
```

### Threading `client_entrypoint` through the connection helpers

Only the explicit-label case (order item 1 in the issue: "explicit label set by our own CLI
`main()`") needs plumbing — everything else is auto-detected inside `attribution.py` with no
caller involvement.

- `oidc_connection.connect(...)` (`oidc_connection.py:91`) gains `client_entrypoint: Optional[str]
  = None`, forwarded to `FlightSQLClient(..., client_entrypoint=client_entrypoint)` at `:163-165`.
- `cli/connection.py::connect(profile=None)` gains `client_entrypoint=None`, forwarded to both
  branches: the `oidc_connection.connect(..., client_entrypoint=client_entrypoint)` call and the
  plain `FlightSQLClient(cfg.uri, client_entrypoint=client_entrypoint)` call.
- `cli/query.py::main()` calls `connection.connect(profile=args.profile,
  client_entrypoint="cli-query")` (`:142`).
- Top-level `micromegas/__init__.py::connect()` is unchanged — it's the documented
  library/notebook entry point, so it must leave `client_entrypoint=None` and let
  auto-detection (jupyter/repl/script) do its job.
- `cli/screens.py` and `cli/logout.py` are unchanged — neither issues a FlightSQL query (see
  Current State), so there is no `cli-screens` value to send yet.

### Server: read, log, and audit the two new fields

`flight_sql_service_impl.rs::execute_query` (`:552-591`) reads two more headers alongside
`client_type`, each defaulting to `"unknown"` when absent (same convention as `client_type`
today — a real, distinguishable value from the Python client's own `"none"`/`"script"`
fallbacks, which mean "the client actively resolved this and found nothing," not "the client
didn't report this axis at all"):

```rust
let client_agent = metadata
    .get("x-client-agent")
    .and_then(|v| v.to_str().ok())
    .unwrap_or("unknown");
let client_entrypoint = metadata
    .get("x-client-entrypoint")
    .and_then(|v| v.to_str().ok())
    .unwrap_or("unknown");
let client_session = metadata
    .get("x-client-session")
    .and_then(|v| v.to_str().ok())
    .map(|s| s.to_string());
```

`client_session` stays `Option<String>` — there's no meaningful default for a caller that
never sent one, so it's omitted rather than defaulted, matching `name`/`range_begin`'s
existing `Option` treatment on `QueryAuditRecord`.

- Both start-of-query `info!` lines (`:561-566`, `:568-573`) gain `agent={client_agent}
  entrypoint={client_entrypoint}` in their format string (session omitted from the free-text
  line — it's already in the structured audit record, and the id itself is only useful for
  correlating audit records, not for skimming logs).
- `QueryAuditState` (`query_audit.rs`-adjacent struct in `flight_sql_service_impl.rs:262-292`)
  gains `agent: String`, `entrypoint: String`, `session: Option<String>`, populated at
  construction (`:589-608`) alongside `client: client_type.to_string()`.
- `QueryAuditState::emit` (`:304-347`) copies the three new fields into `QueryAuditRecord`
  alongside `client: self.client.clone()`.
- `QueryAuditRecord` (`query_audit.rs:80-119`) gains:
  ```rust
  pub agent: String,
  pub entrypoint: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub session: Option<String>,
  ```
  placed immediately after `client`, matching the header/log grouping.

### Gateway: forward, don't augment

Unlike `x-client-type` (hop-chain, gets `+gateway` appended) and `x-request-id` (hop
correlation, generated if absent), the three new headers describe who authored the SQL — a
property of the original caller, unaffected by how many hops the request took. They belong in
`HeaderForwardingConfig::default()`'s plain allowlist (`http_gateway.rs:44-53`), not in
`build_origin_metadata`:

```rust
allowed_headers: vec![
    "Authorization".to_string(),
    "User-Agent".to_string(),
    "X-Client-Type".to_string(),
    "X-Client-Agent".to_string(),      // new
    "X-Client-Entrypoint".to_string(), // new
    "X-Client-Session".to_string(),    // new
    "X-Correlation-ID".to_string(),
    "X-Request-ID".to_string(),
    "X-User-Email".to_string(),
    "X-User-ID".to_string(),
    "X-User-Name".to_string(),
],
```

No change to `build_origin_metadata` or the `handle_query` skip-list (`:328-333`) — those stay
specific to the two headers the gateway itself controls.

## Implementation Steps

**Phase 1 — Python client**
1. New `python/micromegas/micromegas/flightsql/attribution.py`: `resolve_client_agent()`,
   `resolve_client_entrypoint(explicit=None)`, `new_session_id()`, and the
   `_KNOWN_AGENT_HARNESS_ENV_VARS` table (just `CLAUDECODE` → `claude-code` for now).
2. `client.py`: extend `make_call_headers` with `client_agent=None, client_entrypoint=None,
   client_session=None` params (conditionally appended, per Design). Update the three call
   sites (`:356`, `:415`, `:447`) to pass the new cached attributes.
3. `client.py::FlightSQLClient.__init__` (`:172`): add `client_entrypoint=None` parameter;
   resolve and cache `self.__client_agent`, `self.__client_entrypoint`, `self.__session_id`
   via the new `attribution` module. Update the class docstring's `Args`.
4. `oidc_connection.py::connect()` (`:91`): add `client_entrypoint: Optional[str] = None`
   parameter, forward to `FlightSQLClient(...)` at `:163-165`. Update its docstring.
5. `cli/connection.py::connect()` (`:4`): add `client_entrypoint=None` parameter, forward to
   both the `oidc_connection.connect(...)` and `FlightSQLClient(cfg.uri, ...)` branches.
6. `cli/query.py::main()` (`:142`): pass `client_entrypoint="cli-query"`.
7. `python/micromegas/tests/cli/test_connection.py::test_profile_argument_resolves_to_that_profiles_uri`:
   its `FakeFlightSQLClient.__init__(self, uri)` (`:24-26`) only accepts a positional `uri` —
   `connection.connect()` will now always pass `client_entrypoint=` as a keyword, so update the
   fake's signature to `def __init__(self, uri, client_entrypoint=None):` (capturing it isn't
   necessary for that test's assertion, just accepting it so the call doesn't raise).

**Phase 2 — Python tests**
8. New `python/micromegas/tests/test_client_attribution.py`: hermetic unit tests for
   `resolve_client_agent`/`resolve_client_entrypoint`/`new_session_id`, using `monkeypatch` on
   `os.environ` and `sys.modules`/`sys.flags` — covering: no env vars → `"none"`/`"script"`;
   `MICROMEGAS_CLIENT_AGENT` override wins over the harness table; `CLAUDECODE` set → `
   "claude-code"`; `explicit="cli-query"` wins over every other entrypoint signal;
   `MICROMEGAS_CLIENT_ENTRYPOINT` override; `"ipykernel"` in `sys.modules` → `"jupyter"`;
   `sys.flags.interactive` (or no `__main__.__file__`) → `"repl"`; `new_session_id()` returns a
   valid, distinct UUID string on each call.
9. `test_flightsql_headers.py`: add a case asserting the three new headers are present (as
   bytes tuples) when the new params are passed, and absent when they're left at their
   `None` defaults — keeping the "no I/O" hermetic property documented in the file's own
   docstring, since `make_call_headers` itself never reads env/`sys` state.

**Phase 3 — Rust server**
10. `flight_sql_service_impl.rs`: read `x-client-agent`/`x-client-entrypoint` (default
    `"unknown"`) and `x-client-session` (`Option<String>`) alongside `client_type` (`:552`);
    add `agent={client_agent} entrypoint={client_entrypoint}` to both `info!` format strings
    (`:561-573`); add `agent`, `entrypoint`, `session` fields to `QueryAuditState`
    (`:262-292`), populated at construction (`:589-608`); thread them into `QueryAuditState::emit`'s
    `QueryAuditRecord` construction (`:317-342`).
11. `query_audit.rs`: add `pub agent: String`, `pub entrypoint: String`,
    `#[serde(skip_serializing_if = "Option::is_none")] pub session: Option<String>` to
    `QueryAuditRecord` (`:80-119`), placed after `client`.
12. `http_gateway.rs::HeaderForwardingConfig::default()` (`:44-53`): add `"X-Client-Agent"`,
    `"X-Client-Entrypoint"`, `"X-Client-Session"` to `allowed_headers`.

**Phase 4 — Rust tests**
13. `rust/public/tests/query_audit_tests.rs`: add `agent`/`entrypoint`/`session` to the
    full-record fixture and the omits-optionals fixture (asserting `session` is omitted when
    `None`, and that `agent`/`entrypoint` are always present, matching the `client`/`name`
    split already established there).
14. `rust/public/tests/http_gateway_tests.rs`: extend `test_default_config` to assert
    `should_forward("X-Client-Agent")`, `should_forward("X-Client-Entrypoint")`,
    `should_forward("X-Client-Session")` are all `true`.

**Phase 5 — docs**
15. `mkdocs/docs/query-guide/query-audit-log.md`: add `agent`/`entrypoint`/`session` rows to
    the `## Fields` table (`agent`/`entrypoint` always present, default `unknown`; `session`
    present only if the caller sent `x-client-session`); note the three-way distinction
    between "unknown" (client didn't report), "none"/"script" (python client actively found
    nothing), and a detected value.
16. `mkdocs/docs/query-guide/python-api.md`: update the `FlightSQLClient(uri, headers=None,
    preserve_dictionary=False, auth_provider=None)` signature (`:281`) to include
    `client_entrypoint=None`; add a short "Client Attribution" subsection documenting
    `x-client-agent`/`x-client-entrypoint`/`x-client-session`, the `MICROMEGAS_CLIENT_AGENT`/
    `MICROMEGAS_CLIENT_ENTRYPOINT` overrides, and that these are analytics-only (never used for
    auth/quota).
17. `mkdocs/docs/gateway/configuration.md`: add the three new headers to the "Default headers"
    bullet list (`:29`).
18. `CHANGELOG.md`: `## Unreleased` → `**Python:**` (client changes) and `**Analytics:**`
    (server/audit changes) entries. Flag `QueryAuditRecord` as a **minor breaking change**
    again (gains `agent`, `entrypoint`, `session`), matching how the two prior additions to
    this struct were documented (#1435, #1406).

## Files to Modify

- `python/micromegas/micromegas/flightsql/attribution.py` — new.
- `python/micromegas/micromegas/flightsql/client.py` — `make_call_headers`, three call sites,
  `FlightSQLClient.__init__`.
- `python/micromegas/micromegas/oidc_connection.py` — `connect()`.
- `python/micromegas/micromegas/cli/connection.py` — `connect()`.
- `python/micromegas/micromegas/cli/query.py` — `main()`.
- `python/micromegas/tests/cli/test_connection.py` — fake client signature fix.
- `python/micromegas/tests/test_client_attribution.py` — new.
- `python/micromegas/tests/test_flightsql_headers.py` — new-headers cases.
- `rust/public/src/servers/flight_sql_service_impl.rs` — header reads, log lines,
  `QueryAuditState`.
- `rust/public/src/servers/query_audit.rs` — `QueryAuditRecord` fields.
- `rust/public/src/servers/http_gateway.rs` — `HeaderForwardingConfig::default()`.
- `rust/public/tests/query_audit_tests.rs` — fixture updates.
- `rust/public/tests/http_gateway_tests.rs` — allowlist assertions.
- `mkdocs/docs/query-guide/query-audit-log.md` — `## Fields` table.
- `mkdocs/docs/query-guide/python-api.md` — constructor signature + new subsection.
- `mkdocs/docs/gateway/configuration.md` — default headers list.
- `CHANGELOG.md` — `## Unreleased` entries.

## Trade-offs

- **Separate headers/fields, not a composed `python+claude-code` chain.** The issue weighs
  both; separate fields are chosen because `client_type`'s `+gateway` chaining answers "what
  hops did this take," while agent/entrypoint answer "who authored the SQL" — a different
  axis that composing into one string would make `GROUP BY` on painful to split back apart.
- **Detection stays in the Python client only.** The Rust client factory
  (`flightsql_client_factory.rs`) and the Grafana Go plugin both set `x-client-type` themselves
  but are out of scope: neither runs inside an agent harness in any scenario this issue cares
  about (Grafana is a dashboard backend; the Rust factory backs the web app's own
  authenticated-user data-source connections), so there's nothing for them to detect. Server
  defaults (`"unknown"`) already cover their absence correctly.
- **`cli-screens` is not implemented.** The issue comment proposes it, but `screens.py` never
  calls FlightSQL — it manages screen configs over `analytics-web-srv`'s REST API. Adding an
  unreachable enum value would be dead code; it's deferred until (if) `screens.py` gains a
  FlightSQL-querying code path. See Open Questions.
- **No env override for `x-client-session`, and CLI multi-query tasks are not
  session-correlated.** Unlike agent/entrypoint, the issue doesn't propose an override. A
  fresh UUID per `FlightSQLClient` instance matches "one client instantiation = one task" for
  a long-lived script/notebook process that issues several queries, but **not** for the CLI:
  `cli/query.py::main()` constructs exactly one `FlightSQLClient` and issues exactly one query
  per process invocation, so an agent driving `micromegas-query` once per query (the "queries
  per task, retries after error" scenario this issue is motivated by) gets a *different*
  session id for every query in that task — those queries are not correlatable via
  `x-client-session` under this design. Fixing that would mean either the orchestrating agent
  injecting a shared id (needs a `MICROMEGAS_CLIENT_SESSION` override, not proposed by the
  issue) or the CLI persisting a session id across invocations (needs on-disk state, out of
  scope). Left as a known limitation; an override could be added later without breaking
  anything, since it's purely additive.
- **No server-side validation/allowlist on `agent`/`entrypoint` values.** Same as
  `x-client-type` today: the header is caller-controlled and analytics-only, so a malicious or
  buggy client can send anything. Not a new risk this plan introduces.

## Documentation

- `mkdocs/docs/query-guide/query-audit-log.md` — `## Fields` table (new rows).
- `mkdocs/docs/query-guide/python-api.md` — constructor signature, new "Client Attribution"
  subsection.
- `mkdocs/docs/gateway/configuration.md` — default forwarded-headers list.
- `CHANGELOG.md` — `## Unreleased` entries.

## Testing Strategy

1. `poetry run pytest` in `python/micromegas/` (per `python/CLAUDE.md`), covering the new
   `test_client_attribution.py` and the updated `test_flightsql_headers.py`/
   `tests/cli/test_connection.py`.
2. `poetry run black <changed files>` before commit (per `python/CLAUDE.md`).
3. `cargo test -p micromegas --features server` covering the updated
   `query_audit_tests.rs`/`http_gateway_tests.rs`.
4. `cargo fmt` and `cargo clippy --workspace -- -D warnings` (per `rust/CLAUDE.md`).
5. Manual smoke test: start services (`start_services.py`), run
   `CLAUDECODE=1 micromegas-query "SELECT 1" --all` and `tail -f /tmp/analytics.log` (or the
   relevant service log) for that request's `execute_query` line, confirming
   `agent=claude-code entrypoint=cli-query` appears; then query `flightsql_query_audit` (per
   `query-audit-log.md`'s pattern) and confirm the JSON record has `"agent":"claude-code"`,
   `"entrypoint":"cli-query"`, and a `"session"` UUID. Run it a second time and confirm the
   `"session"` UUID differs from the first run (per-invocation, not per-task, per the
   Trade-offs note above). Repeat without `CLAUDECODE` set to confirm `agent=none`. Run the
   same query through a plain Python script (not the CLI) to confirm `entrypoint=script`, and
   through a Jupyter kernel to confirm `entrypoint=jupyter`.

## Open Questions

- **Subagent grouping.** The issue notes `CLAUDE_CODE_CHILD_SESSION` marks a subagent but it's
  unverified whether a subagent's session id differs from its parent's. Not addressed here —
  `x-client-session` is a fresh UUID per `FlightSQLClient` instance regardless, so a subagent
  that constructs its own client already gets its own session; whether that's the desired
  parent/child grouping for analysis is a question for whoever consumes the audit log, not a
  client-side plumbing decision.
