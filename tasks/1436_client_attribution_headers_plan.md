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
- `x-client-session` — an opaque id that correlates every query from one client instance, so
  per-task/per-session metrics (queries per task, retries after error, bytes scanned per
  session) become possible for a long-lived script or notebook process that issues several
  queries through the same `FlightSQLClient`. For the CLI, where each `micromegas-query`
  invocation constructs a fresh client, this correlates across invocations only when a known
  agent harness's session env var is present (currently just Claude Code's
  `CLAUDE_CODE_SESSION_ID`); otherwise each invocation gets its own fresh session id — see
  Trade-offs below.

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
  `cli-screens` entrypoint value has no live call site today.
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
import re
import sys
import uuid

_KNOWN_AGENT_HARNESS_ENV_VARS = {
    "CLAUDECODE": "claude-code",
}

# Harness-provided session ids, checked in order, that stay stable across every
# FlightSQLClient/CLI invocation within one agent session -- unlike a fresh
# UUID, these let per-task metrics (queries per task, retries after error)
# correlate for the CLI case described in Trade-offs. Confirmed by direct
# testing: CLAUDE_CODE_SESSION_ID does not vary between a Claude Code parent
# and its subagents, so it identifies "one agent session," which is the
# correlation granularity this header wants.
_KNOWN_AGENT_HARNESS_SESSION_ENV_VARS = (
    "CLAUDE_CODE_SESSION_ID",
)

# gRPC metadata values must be plain ASCII with no control characters (a
# non-ASCII or newline-containing value passed through pyarrow's
# FlightCallOptions aborts the process with a native "Check failed" crash,
# not a catchable Python exception) -- so overrides are validated before use,
# with a silent fall-through to the detected value on rejection.
_MAX_OVERRIDE_LEN = 64
_VALID_OVERRIDE_RE = re.compile(r"[\x20-\x7e]+")


def _sanitize_override(value):
    """Return `value` if it's a safe gRPC header value (printable ASCII,
    bounded length), else None so the caller falls back to detection."""
    if value and len(value) <= _MAX_OVERRIDE_LEN and _VALID_OVERRIDE_RE.fullmatch(value):
        return value
    return None


def resolve_client_agent():
    """Who is driving this client: an explicit override, a known agent harness
    detected from its marker env var, or "none" for a plain human/script."""
    override = _sanitize_override(os.environ.get("MICROMEGAS_CLIENT_AGENT"))
    if override:
        return override
    for env_var, agent_name in _KNOWN_AGENT_HARNESS_ENV_VARS.items():
        if os.environ.get(env_var):
            return agent_name
    return "none"


def resolve_client_entrypoint(explicit=None):
    """How this client was invoked. `explicit` (set by our own CLI main()s,
    but also reachable by any library caller via the public
    `FlightSQLClient(client_entrypoint=...)`/`connect(client_entrypoint=...)`
    parameters) always wins, but -- unlike the env-var override below -- an
    invalid `explicit` value raises `ValueError` instead of silently falling
    through: a caller-supplied argument deserves a catchable error at the
    call site, not a masked native gRPC-metadata crash later on the first
    query. Otherwise: a sanitized env override, then `-c`/jupyter/repl
    detection, then "script". Closed vocabulary only -- never raw
    sys.argv[0]/__main__.__file__."""
    if explicit:
        sanitized = _sanitize_override(explicit)
        if sanitized is None:
            raise ValueError(
                f"invalid client_entrypoint {explicit!r}: must be printable "
                f"ASCII, <= {_MAX_OVERRIDE_LEN} chars"
            )
        return sanitized
    override = _sanitize_override(os.environ.get("MICROMEGAS_CLIENT_ENTRYPOINT"))
    if override:
        return override
    if "ipykernel" in sys.modules:
        return "jupyter"
    if sys.argv and sys.argv[0] == "-c":
        # `python -c "..."` has no `__main__.__file__`, so without this check
        # it would fall through to "repl" below -- mislabeling exactly the
        # invocation mode agent harnesses most often use.
        return "script"
    main_module = sys.modules.get("__main__")
    if sys.flags.interactive or not hasattr(main_module, "__file__"):
        return "repl"
    return "script"


def new_session_id():
    """A per-instance session id. Prefers a stable harness-provided session id
    (see `_KNOWN_AGENT_HARNESS_SESSION_ENV_VARS`) so every FlightSQLClient/CLI
    invocation within one agent session correlates; falls back to a fresh
    UUID when no such signal is present (plain script/notebook process, or an
    unrecognized harness)."""
    for env_var in _KNOWN_AGENT_HARNESS_SESSION_ENV_VARS:
        harness_session = _sanitize_override(os.environ.get(env_var))
        if harness_session:
            return harness_session
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
   `resolve_client_entrypoint(explicit=None)`, `new_session_id()`, a `_sanitize_override()`
   helper guarding `MICROMEGAS_CLIENT_AGENT`/`MICROMEGAS_CLIENT_ENTRYPOINT` (printable ASCII,
   length-bounded, else fall through to detection instead of sending an unsafe gRPC header
   value), the `_KNOWN_AGENT_HARNESS_ENV_VARS` table (just `CLAUDECODE` → `claude-code` for
   now), and the `_KNOWN_AGENT_HARNESS_SESSION_ENV_VARS` table (just `CLAUDE_CODE_SESSION_ID`
   for now) that `new_session_id()` checks before minting a fresh UUID.
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
   update `FakeFlightSQLClient.__init__(self, uri)` (`:24-26`) to
   `def __init__(self, uri, client_entrypoint=None): captured_entrypoints.append(client_entrypoint)`
   (alongside the existing `captured_uris` capture), call
   `connection.connect(profile="dev", client_entrypoint="cli-query")`, and assert
   `captured_entrypoints == ["cli-query"]` — covering the plain (non-OIDC) branch
   (`cli/connection.py:26`). Add a second test covering the OIDC branch
   (`cli/connection.py:14-22`): configure a profile with the config-file keys that make
   `resolve_connection` populate `oidc_issuer`/`oidc_client_id` — i.e. `{"uri": ...,
   "client_id": "...", "issuers": [{"issuer": "https://...", "audience": "..."}]}` (see
   `cli/config.py:135-146`), not the `ConnectionConfig` field names themselves,
   `monkeypatch.setattr(oidc_connection, "connect", fake_oidc_connect)` where
   `fake_oidc_connect(**kwargs)` records `kwargs` and returns a stub client, then assert
   `connection.connect(profile=<oidc-profile>, client_entrypoint="cli-query")` results in
   `kwargs["client_entrypoint"] == "cli-query"` — without driving a real login flow.

**Phase 2 — Python tests**
8. New `python/micromegas/tests/test_client_attribution.py`: hermetic unit tests for
   `resolve_client_agent`/`resolve_client_entrypoint`/`new_session_id`, using `monkeypatch` on
   `os.environ` and `sys.modules`/`sys.flags`/`sys.argv`. The env-scrubbing autouse fixture in
   `tests/cli/conftest.py` is scoped to `tests/cli/` only, so it does not apply here — and
   `CLAUDECODE` in particular is set in the environment of every subprocess this development
   harness spawns (which is how this repo's own `python3 ../../build/python_ci.py` gets run), so
   every negative-case test in this file must explicitly `monkeypatch.delenv("CLAUDECODE",
   raising=False)` (plus every `MICROMEGAS_CLIENT_*` var, `CLAUDE_CODE_SESSION_ID`, and any
   other harness marker var in `_KNOWN_AGENT_HARNESS_ENV_VARS`/
   `_KNOWN_AGENT_HARNESS_SESSION_ENV_VARS`) before asserting on the no-signal path. Covering: no
   env vars/markers present → `"none"`/`"script"`; `MICROMEGAS_CLIENT_AGENT` override wins over
   the harness table; `CLAUDECODE` set → `"claude-code"`; a non-ASCII, embedded-newline, or
   *trailing*-newline (e.g. `"claude-code\n"`) `MICROMEGAS_CLIENT_AGENT`/
   `MICROMEGAS_CLIENT_ENTRYPOINT` override is rejected and falls back to the detected value
   (not sent as-is) — the trailing-newline case guards against `^...$`-style regexes, where `$`
   matches before a trailing newline and would wrongly accept it; an over-length override is
   rejected the same way; `explicit="cli-query"` wins over every other entrypoint signal; an
   invalid `explicit` value (non-ASCII, embedded/trailing newline, or over-length) raises
   `ValueError` instead of falling back to detection, unlike the env-var override path;
   `MICROMEGAS_CLIENT_ENTRYPOINT` override; `sys.argv[0] == "-c"` → `"script"` (not `"repl"`);
   `"ipykernel"` in `sys.modules` → `"jupyter"`; `sys.flags.interactive` (or no
   `__main__.__file__`) → `"repl"`; with no harness session var set, `new_session_id()` returns
   a valid, distinct UUID string on each call; with `CLAUDE_CODE_SESSION_ID` set,
   `new_session_id()` returns that value verbatim (and does so identically on repeated calls,
   unlike the UUID case); an unsafe (non-ASCII/over-length) `CLAUDE_CODE_SESSION_ID` value falls
   back to a fresh UUID via the same `_sanitize_override` path. Also add
   `"tests/test_client_attribution.py"` to `HERMETIC_TEST_ARGS` in `build/python_ci.py` — the
   explicit hermetic file list CI actually invokes — so this new file isn't silently skipped.
9. `test_flightsql_headers.py`: add a case asserting the three new headers are present (as
   bytes tuples) when the new params are passed, and absent when they're left at their
   `None` defaults — keeping the "no I/O" hermetic property documented in the file's own
   docstring, since `make_call_headers` itself never reads env/`sys` state. Also add a test
   that constructs a real `FlightSQLClient(uri, client_entrypoint="cli-query")` and asserts
   that `query()`, `query_stream()`, and `query_arrow()` each issue calls whose
   `FlightCallOptions.headers` (readable in pyarrow 23.0.1) carry `x-client-entrypoint:
   cli-query` plus the env-derived `x-client-agent`/`x-client-session` values — exercising
   `resolve_client_entrypoint(explicit=...)` end-to-end through the real constructor (not by
   hand-setting the private `__client_agent`/`__client_entrypoint`/`__session_id`
   attributes), guarding all three call sites (`client.py:356/415/447`), not just the CLI
   path `tests/cli/test_query.py` covers. This test lives outside `tests/cli/`, so
   `tests/cli/conftest.py`'s autouse env scrub does not apply here either — same as step 8,
   it must explicitly set `monkeypatch.setenv("CLAUDECODE", "1")` and
   `monkeypatch.setenv("CLAUDE_CODE_SESSION_ID", <a test-chosen UUID string>)`, and
   `monkeypatch.delenv("MICROMEGAS_CLIENT_AGENT", raising=False)` /
   `monkeypatch.delenv("MICROMEGAS_CLIENT_ENTRYPOINT", raising=False)` so neither override
   wins over the detected value. Also monkeypatch `pyarrow.flight.FlightClient` (or
   `flight.connect`) to a stub. Assert the concrete expected values:
   `x-client-agent == b"claude-code"` and `x-client-session` equal to the pinned
   `CLAUDE_CODE_SESSION_ID` value the test set, encoded as bytes.
10. New `python/micromegas/tests/cli/test_query.py`: monkeypatch `sys.argv` to a minimal valid
    invocation (e.g. `["micromegas-query", "SELECT 1", "--all"]`) and
    `connection.connect` (as imported in `cli/query.py`) to a fake that records its kwargs and
    returns a stub client whose `.query()` returns an empty `DataFrame`; call `query.main()` and
    assert the captured kwargs include `client_entrypoint="cli-query"` — guarding against
    `query.py:142` dropping that argument.

**Phase 3 — Rust server**
11. `flight_sql_service_impl.rs`: read `x-client-agent`/`x-client-entrypoint` (default
    `"unknown"`) and `x-client-session` (`Option<String>`) alongside `client_type` (`:552`);
    add `agent={client_agent} entrypoint={client_entrypoint}` to both `info!` format strings
    (`:561-573`); add `agent`, `entrypoint`, `session` fields to `QueryAuditState`
    (`:262-292`), populated at construction (`:589-608`); thread them into `QueryAuditState::emit`'s
    `QueryAuditRecord` construction (`:317-342`).
12. `query_audit.rs`: add `pub agent: String`, `pub entrypoint: String`,
    `#[serde(skip_serializing_if = "Option::is_none")] pub session: Option<String>` to
    `QueryAuditRecord` (`:80-119`), placed after `client`.
13. `http_gateway.rs::HeaderForwardingConfig::default()` (`:44-53`): add `"X-Client-Agent"`,
    `"X-Client-Entrypoint"`, `"X-Client-Session"` to `allowed_headers`.

**Phase 4 — Rust tests**
14. `rust/public/tests/query_audit_tests.rs`: add `agent`/`entrypoint`/`session` to the
    full-record fixture and the omits-optionals fixture (asserting `session` is omitted when
    `None`, and that `agent`/`entrypoint` are always present, matching the `client`/`name`
    split already established there).
15. `rust/public/tests/http_gateway_tests.rs`: extend `test_default_config` to assert
    `should_forward("X-Client-Agent")`, `should_forward("X-Client-Entrypoint")`,
    `should_forward("X-Client-Session")` are all `true`.

**Phase 5 — docs**
16. `mkdocs/docs/query-guide/query-audit-log.md`: add `agent`/`entrypoint`/`session` rows to
    the `## Fields` table (`agent`/`entrypoint` always present, default `unknown`; `session`
    present only if the caller sent `x-client-session`); note the three-way distinction
    between "unknown" (client didn't report), "none"/"script" (python client actively found
    nothing), and a detected value. Add a `## Notes` entry (alongside the existing
    `bytes_scanned`/`peak_memory_bytes` caveats) recording that `agent` measures "ran inside a
    known agent harness's environment," not "an LLM wrote this SQL": environment variables are
    inherited by child processes, so a human running the CLI from a shell nested inside an
    agent session is labelled with that agent too.
17. `mkdocs/docs/query-guide/python-api.md`: update the `FlightSQLClient(uri, headers=None,
    preserve_dictionary=False, auth_provider=None)` signature (`:281`) to include
    `client_entrypoint=None`; add a short "Client Attribution" subsection documenting
    `x-client-agent`/`x-client-entrypoint`/`x-client-session`, the `MICROMEGAS_CLIENT_AGENT`/
    `MICROMEGAS_CLIENT_ENTRYPOINT` overrides, and that these are analytics-only (never used for
    auth/quota).
18. `mkdocs/docs/gateway/configuration.md`: add the three new headers to the "Default headers"
    bullet list (`:29`).
19. `CHANGELOG.md`: `## Unreleased` → `**Python:**` (client changes) and `**Analytics:**`
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
- `python/micromegas/tests/cli/test_connection.py` — fake client captures/asserts
  `client_entrypoint` on both branches.
- `python/micromegas/tests/cli/test_query.py` — new; asserts `main()` passes
  `client_entrypoint="cli-query"`.
- `python/micromegas/tests/test_client_attribution.py` — new.
- `python/micromegas/tests/test_flightsql_headers.py` — new-headers cases.
- `build/python_ci.py` — add `"tests/test_client_attribution.py"` to `HERMETIC_TEST_ARGS`.
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
- **`x-client-agent` measures "ran inside a known agent harness's environment," not "an LLM
  authored this SQL."** Environment variables are inherited by child processes, so a human
  running the CLI from a shell nested inside an agent session (e.g. a terminal opened from
  within Claude Code) is labelled `claude-code` even though a person, not the agent, typed the
  query. Acceptable, but the field name and docs (`query-audit-log.md`'s Notes, per step 16)
  must be explicit about what it actually measures — environment provenance, not authorship.
- **Detection stays in the Python client only.** The Rust client factory
  (`flightsql_client_factory.rs`) and the Grafana Go plugin both set `x-client-type` themselves
  but are out of scope: neither runs inside an agent harness in any scenario this issue cares
  about (Grafana is a dashboard backend; the Rust factory backs the web app's own
  authenticated-user data-source connections), so there's nothing for them to detect. Server
  defaults (`"unknown"`) already cover their absence correctly.
- **`cli-screens` is not implemented.** The issue comment proposes it, but `screens.py` never
  calls FlightSQL — it manages screen configs over `analytics-web-srv`'s REST API. Adding an
  unreachable enum value would be dead code; it's deferred until (if) `screens.py` gains a
  FlightSQL-querying code path.
- **`x-client-session` correlates CLI multi-query tasks only for known agent harnesses, not
  in general.** A bare fresh UUID per `FlightSQLClient` instance would match "one client
  instantiation = one task" for a long-lived script/notebook process, but **not** for the CLI:
  `cli/query.py::main()` constructs exactly one `FlightSQLClient` and issues exactly one query
  per process invocation, so an agent driving `micromegas-query` once per query (the "queries
  per task, retries after error" scenario this issue is motivated by) would get a *different*
  session id for every query in that task under a naive UUID-only scheme. `new_session_id()`
  closes this for the harness this issue is motivated by: it checks
  `_KNOWN_AGENT_HARNESS_SESSION_ENV_VARS` (`CLAUDE_CODE_SESSION_ID`) before minting a UUID, and
  **confirmed by direct testing**, that variable does not vary between a Claude Code parent and
  its subagents — spawning a subagent and inspecting its environment shows the identical
  session id — so every `micromegas-query` invocation within one Claude Code session shares a
  session id, without any orchestrator cooperation or on-disk state. This is deliberately
  narrower than a general fix: a plain script/notebook process, a human's shell, or an
  unrecognized agent harness still gets a fresh UUID per instantiation (the "one client
  instantiation = one task" case above), and there's no env override for callers that want to
  force a shared id outside a known harness — left as a known limitation, addable later without
  breaking anything since it's purely additive. `CLAUDE_CODE_CHILD_SESSION=1` was considered as
  an alternate signal but rejected: it's set for *any* subprocess the CLI's tool layer spawns
  (including the top-level session's own shell commands), not only subagent-issued ones, so it
  doesn't identify a session the way `CLAUDE_CODE_SESSION_ID` does.
- **No server-side validation/allowlist on `agent`/`entrypoint` values.** Same as
  `x-client-type` today: the header is caller-controlled and analytics-only, so a malicious or
  buggy client can send anything. Not a new risk this plan introduces. Client-side, though,
  `attribution.py`'s `_sanitize_override` does validate `MICROMEGAS_CLIENT_AGENT`/
  `MICROMEGAS_CLIENT_ENTRYPOINT` (printable ASCII, bounded length) before they reach gRPC
  metadata, since an unvalidated non-ASCII or newline-containing value there crashes the
  process rather than raising a catchable exception — that's a client-side safety check, not
  a server-side trust boundary, so the risk described above is unchanged. The same validation
  also guards the public `client_entrypoint` argument on `FlightSQLClient.__init__`/
  `connect(...)` (a more likely source of an unsafe value than the env vars, since it's a
  documented API parameter): `resolve_client_entrypoint` runs it through `_sanitize_override`
  too, but raises `ValueError` on rejection instead of silently falling back, since an explicit
  caller argument warrants a catchable error rather than a masked mislabeling.

## Documentation

- `mkdocs/docs/query-guide/query-audit-log.md` — `## Fields` table (new rows).
- `mkdocs/docs/query-guide/python-api.md` — constructor signature, new "Client Attribution"
  subsection.
- `mkdocs/docs/gateway/configuration.md` — default forwarded-headers list.
- `CHANGELOG.md` — `## Unreleased` entries.

## Testing Strategy

1. `python3 ../../build/python_ci.py` from `python/micromegas/` (per `python/CLAUDE.md`'s CI
   line) — **not** a bare `poetry run pytest`, which would also collect the integration suite
   under `tests/` (e.g. `tests/auth/test_oidc_integration.py`), which needs live services. This
   runs the hermetic `HERMETIC_TEST_ARGS` list, covering the new `test_client_attribution.py`
   and `tests/cli/test_query.py`, and the updated `test_flightsql_headers.py`/
   `tests/cli/test_connection.py`, once `HERMETIC_TEST_ARGS` is updated per step 8 above.
2. `poetry run black <changed files>` before commit (per `python/CLAUDE.md`).
3. `cargo test -p micromegas --features server` covering the updated
   `query_audit_tests.rs`/`http_gateway_tests.rs`.
4. `cargo fmt` and `cargo clippy --workspace -- -D warnings` (per `rust/CLAUDE.md`).
5. Manual smoke test: start services (`start_services.py`), run
   `CLAUDECODE=1 micromegas-query "SELECT 1" --all` and `tail -f /tmp/analytics.log` (or the
   relevant service log) for that request's `execute_query` line, confirming
   `agent=claude-code entrypoint=cli-query` appears; then query `flightsql_query_audit` (per
   `query-audit-log.md`'s pattern) and confirm the JSON record has `"agent":"claude-code"`,
   `"entrypoint":"cli-query"`, and a `"session"` value. If `CLAUDE_CODE_SESSION_ID` is set in
   the shell (as it will be when this test is run from inside a Claude Code session, alongside
   `CLAUDECODE`), confirm `"session"` equals `$CLAUDE_CODE_SESSION_ID` and run the command a
   second time to confirm `"session"` is identical across both runs — stable per harness
   session, per the Trade-offs note above, not a fresh UUID. Separately, in a shell with both
   `CLAUDECODE` and `CLAUDE_CODE_SESSION_ID` unset, run the same command twice and confirm
   `"session"` is a valid UUID that differs between the two runs (fresh per invocation, since no
   known agent-harness session var is present) and that `agent=none`. Run the same query through
   a plain Python script (not the CLI) to confirm `entrypoint=script`, and through a Jupyter
   kernel to confirm `entrypoint=jupyter`.
