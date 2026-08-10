"""Hermetic unit tests for micromegas.flightsql.attribution.

These tests live outside tests/cli/, so tests/cli/conftest.py's autouse
MICROMEGAS_* env scrub does not apply here. `CLAUDECODE` in particular is set
in the environment of every subprocess this development harness spawns
(which is how this repo's own `python3 ../../build/python_ci.py` gets run),
so every negative-case test below explicitly scrubs every env var this
module reads before asserting on the no-signal path.
"""

import sys
import uuid as uuid_module

import pytest

from micromegas.flightsql import attribution


def _scrub_all_attribution_env(monkeypatch):
    """Delete every env var attribution.py reads, so a test can assert on the
    no-signal path regardless of the ambient environment it's run in."""
    monkeypatch.delenv("MICROMEGAS_CLIENT_AGENT", raising=False)
    monkeypatch.delenv("MICROMEGAS_CLIENT_ENTRYPOINT", raising=False)
    for env_var in attribution._KNOWN_AGENT_HARNESS_ENV_VARS:
        monkeypatch.delenv(env_var, raising=False)
    for env_var in attribution._KNOWN_AGENT_HARNESS_SESSION_ENV_VARS:
        monkeypatch.delenv(env_var, raising=False)


# ---------------------------------------------------------------------------
# resolve_client_agent
# ---------------------------------------------------------------------------


def test_resolve_client_agent_no_signal_returns_none(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    assert attribution.resolve_client_agent() == "none"


def test_resolve_client_agent_env_override_wins_over_harness_table(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setenv("MICROMEGAS_CLIENT_AGENT", "my-custom-agent")
    monkeypatch.setenv("CLAUDECODE", "1")
    assert attribution.resolve_client_agent() == "my-custom-agent"


def test_resolve_client_agent_claudecode_detected(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setenv("CLAUDECODE", "1")
    assert attribution.resolve_client_agent() == "claude-code"


def test_resolve_client_agent_non_ascii_override_falls_back_to_detection(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setenv("MICROMEGAS_CLIENT_AGENT", "café")
    assert attribution.resolve_client_agent() == "none"


def test_resolve_client_agent_embedded_newline_override_falls_back(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setenv("MICROMEGAS_CLIENT_AGENT", "claude\ncode")
    assert attribution.resolve_client_agent() == "none"


def test_resolve_client_agent_trailing_newline_override_falls_back(monkeypatch):
    # Guards against ^...$-style regexes, where $ matches before a trailing
    # newline and would wrongly accept it.
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setenv("MICROMEGAS_CLIENT_AGENT", "claude-code\n")
    assert attribution.resolve_client_agent() == "none"


def test_resolve_client_agent_over_length_override_falls_back(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setenv("MICROMEGAS_CLIENT_AGENT", "x" * 65)
    assert attribution.resolve_client_agent() == "none"


# ---------------------------------------------------------------------------
# resolve_client_entrypoint
# ---------------------------------------------------------------------------


def test_resolve_client_entrypoint_explicit_wins_over_every_other_signal(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setenv("MICROMEGAS_CLIENT_ENTRYPOINT", "env-value")
    monkeypatch.setitem(sys.modules, "ipykernel", object())
    assert attribution.resolve_client_entrypoint(explicit="cli-query") == "cli-query"


def test_resolve_client_entrypoint_invalid_explicit_raises_value_error(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    with pytest.raises(ValueError):
        attribution.resolve_client_entrypoint(explicit="bad\nvalue")


def test_resolve_client_entrypoint_invalid_explicit_over_length_raises(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    with pytest.raises(ValueError):
        attribution.resolve_client_entrypoint(explicit="x" * 65)


def test_resolve_client_entrypoint_invalid_explicit_non_ascii_raises(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    with pytest.raises(ValueError):
        attribution.resolve_client_entrypoint(explicit="café")


def test_resolve_client_entrypoint_env_override(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setenv("MICROMEGAS_CLIENT_ENTRYPOINT", "my-entrypoint")
    assert attribution.resolve_client_entrypoint() == "my-entrypoint"


def test_resolve_client_entrypoint_dash_c_returns_script_not_repl(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setattr(sys, "argv", ["-c"])
    assert attribution.resolve_client_entrypoint() == "script"


def test_resolve_client_entrypoint_ipykernel_returns_jupyter(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setattr(sys, "argv", ["some_script.py"])
    monkeypatch.setitem(sys.modules, "ipykernel", object())
    assert attribution.resolve_client_entrypoint() == "jupyter"


def test_resolve_client_entrypoint_interactive_returns_repl(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setattr(sys, "argv", ["python"])

    class FakeFlags:
        interactive = 1

    # sys.flags is an immutable structseq -- its attributes can't be set
    # directly, so the whole object is swapped out instead.
    monkeypatch.setattr(sys, "flags", FakeFlags())

    class FakeMainModule:
        __file__ = "not_used_since_interactive_short_circuits.py"

    monkeypatch.setitem(sys.modules, "__main__", FakeMainModule())
    assert attribution.resolve_client_entrypoint() == "repl"


def test_resolve_client_entrypoint_no_main_file_returns_repl(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setattr(sys, "argv", ["python"])

    class FakeMainModule:
        pass

    monkeypatch.setitem(sys.modules, "__main__", FakeMainModule())
    assert attribution.resolve_client_entrypoint() == "repl"


def test_resolve_client_entrypoint_default_script(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setattr(sys, "argv", ["my_script.py"])

    class FakeMainModule:
        __file__ = "my_script.py"

    monkeypatch.setitem(sys.modules, "__main__", FakeMainModule())
    assert attribution.resolve_client_entrypoint() == "script"


# ---------------------------------------------------------------------------
# new_session_id
# ---------------------------------------------------------------------------


def test_new_session_id_no_harness_var_returns_distinct_valid_uuids(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    first = attribution.new_session_id()
    second = attribution.new_session_id()
    # Valid UUID strings.
    uuid_module.UUID(first)
    uuid_module.UUID(second)
    assert first != second


def test_new_session_id_with_claude_code_session_id_returns_it_verbatim(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setenv("CLAUDE_CODE_SESSION_ID", "abc-123-session")
    first = attribution.new_session_id()
    second = attribution.new_session_id()
    assert first == "abc-123-session"
    assert second == "abc-123-session"


def test_new_session_id_unsafe_claude_code_session_id_falls_back_to_uuid(monkeypatch):
    _scrub_all_attribution_env(monkeypatch)
    monkeypatch.setenv("CLAUDE_CODE_SESSION_ID", "bad\nvalue")
    result = attribution.new_session_id()
    uuid_module.UUID(result)
