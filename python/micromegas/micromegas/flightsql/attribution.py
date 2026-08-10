"""Client attribution detection for FlightSQL headers.

Resolves who is driving a `FlightSQLClient` instance (`x-client-agent`), how
it was invoked (`x-client-entrypoint`), and an opaque per-instance session id
(`x-client-session`) that correlates every query issued through one client
instance. These are analytics-only signals -- never used for auth, quota, or
rate limiting -- and, same as the existing `x-client-type`, trivially
spoofable/omittable by the caller.

Kept separate from `client.py`'s `make_call_headers` (a pure formatter with
no I/O) because detection here reads environment variables and inspects
`sys.modules`/`sys.argv`/`sys.flags` -- not file/network I/O, but hidden
global state that would otherwise make `make_call_headers`'s output depend on
the process environment.
"""

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
_KNOWN_AGENT_HARNESS_SESSION_ENV_VARS = ("CLAUDE_CODE_SESSION_ID",)

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
    if (
        value
        and len(value) <= _MAX_OVERRIDE_LEN
        and _VALID_OVERRIDE_RE.fullmatch(value)
    ):
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
    `FlightSQLClient(client_entrypoint=...)`/`oidc_connection.connect(client_entrypoint=...)`/
    `cli/connection.connect(client_entrypoint=...)` parameters -- the top-level
    `micromegas.connect()` does not take this parameter) always wins, but -- unlike the
    env-var override below -- an
    invalid `explicit` value raises `ValueError` instead of silently falling
    through: a caller-supplied argument deserves a catchable error at the
    call site, not a masked native gRPC-metadata crash later on the first
    query. Otherwise: a sanitized env override, then `-c`/jupyter/repl
    detection, then "script". `explicit`/the env override are free-form
    (validated only for gRPC-safety, not against a vocabulary) -- the closed
    vocabulary ({script, jupyter, repl}) applies only to the
    auto-detection branch below (cli-query is not auto-detected -- it's the
    explicit label our own CLI passes), which must never return raw
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


def resolve_session_id():
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
