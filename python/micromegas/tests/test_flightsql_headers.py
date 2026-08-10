"""Tests for micromegas.flightsql.client's header handling.

Covers make_call_headers -- the function on the live query path that
formats `begin`/`end` values for the FlightSQL call headers (see
client.py's query()/query_stream()/query_arrow()); it has no I/O, so it
can be tested directly without a service or mocking -- and
FlightSQLClient's emission of attribution headers on every call site,
exercised via a stubbed `flight.connect` (no real service or network I/O).
"""

import pyarrow

import micromegas.flightsql.client as flightsql_client
from micromegas.flightsql.client import FlightSQLClient, make_call_headers


def test_make_call_headers_z_suffix_begin():
    headers = make_call_headers("2024-01-01T00:00:00Z", None)
    assert (b"query_range_begin", b"2024-01-01T00:00:00+00:00") in headers


def test_make_call_headers_attribution_headers_present_when_passed():
    headers = make_call_headers(
        None,
        None,
        client_agent="claude-code",
        client_entrypoint="cli-query",
        client_session="session-1",
    )
    assert (b"x-client-agent", b"claude-code") in headers
    assert (b"x-client-entrypoint", b"cli-query") in headers
    assert (b"x-client-session", b"session-1") in headers


def test_make_call_headers_attribution_headers_absent_by_default():
    headers = make_call_headers(None, None)
    names = [name for name, _value in headers]
    assert b"x-client-agent" not in names
    assert b"x-client-entrypoint" not in names
    assert b"x-client-session" not in names


class _FakeDoGetResult:
    def __init__(self):
        self.schema = pyarrow.schema([])

    def __iter__(self):
        return iter([])


class _FakeFlightClient:
    """Stub replacing the real pyarrow FlightClient returned by
    flight.connect(...). Records the FlightCallOptions passed to do_get so
    the test can inspect the headers actually sent on the wire.
    """

    def __init__(self):
        self.do_get_calls = []

    def do_get(self, ticket, options=None):
        self.do_get_calls.append(options)
        return _FakeDoGetResult()


def test_flightsqlclient_sends_attribution_headers_on_every_call_site(monkeypatch):
    # tests/cli/conftest.py's autouse env scrub doesn't apply outside
    # tests/cli/, so this test explicitly controls every env var
    # attribution.py reads.
    monkeypatch.setenv("CLAUDECODE", "1")
    monkeypatch.setenv("CLAUDE_CODE_SESSION_ID", "11111111-1111-1111-1111-111111111111")
    monkeypatch.delenv("MICROMEGAS_CLIENT_AGENT", raising=False)
    monkeypatch.delenv("MICROMEGAS_CLIENT_ENTRYPOINT", raising=False)

    fake_client = _FakeFlightClient()

    # flight.connect is Cython code that resolves FlightClient internally --
    # monkeypatching pyarrow.flight.FlightClient does not intercept it, so
    # flight.connect itself (as imported/seen by micromegas.flightsql.client)
    # is monkeypatched instead.
    monkeypatch.setattr(
        flightsql_client.flight, "connect", lambda **kwargs: fake_client
    )

    client = FlightSQLClient("grpc://localhost:50051", client_entrypoint="cli-query")

    client.query("SELECT 1")
    for batch in client.query_stream("SELECT 1"):
        pass
    client.query_arrow("SELECT 1")

    assert len(fake_client.do_get_calls) == 3
    for options in fake_client.do_get_calls:
        headers = dict(options.headers)
        assert headers[b"x-client-agent"] == b"claude-code"
        assert headers[b"x-client-entrypoint"] == b"cli-query"
        assert headers[b"x-client-session"] == b"11111111-1111-1111-1111-111111111111"
