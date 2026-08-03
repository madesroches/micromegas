import json
import sys
import types

import micromegas.flightsql.client as flightsql_client
from micromegas.cli import config, connection


def test_wrapper_bypasses_profile_resolution(tmp_path, monkeypatch):
    """MICROMEGAS_PYTHON_MODULE_WRAPPER must short-circuit connect() before
    resolve_connection() ever runs, so a `profiles` map with no
    default_profile selected never raises ProfileError for wrapper users."""
    cfg_file = tmp_path / "config.json"
    cfg_file.write_text(
        json.dumps(
            {
                "profiles": {
                    "prod": {"uri": "grpc+tls://prod-host:50051"},
                    "dev": {"uri": "grpc://dev-host:50051"},
                }
            }
        )
    )
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_file)

    sentinel_client = object()
    stub_module = types.ModuleType("fake_micromegas_wrapper")
    stub_module.connect = lambda: sentinel_client
    monkeypatch.setitem(sys.modules, "fake_micromegas_wrapper", stub_module)
    monkeypatch.setenv("MICROMEGAS_PYTHON_MODULE_WRAPPER", "fake_micromegas_wrapper")

    assert connection.connect() is sentinel_client


def test_profile_argument_resolves_to_that_profiles_uri(tmp_path, monkeypatch):
    cfg_file = tmp_path / "config.json"
    cfg_file.write_text(
        json.dumps(
            {
                "default_profile": "prod",
                "profiles": {
                    "prod": {"uri": "grpc+tls://prod-host:50051"},
                    "dev": {"uri": "grpc://dev-host:50051"},
                },
            }
        )
    )
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_file)

    captured_uris = []

    class FakeFlightSQLClient:
        def __init__(self, uri):
            captured_uris.append(uri)

    monkeypatch.setattr(flightsql_client, "FlightSQLClient", FakeFlightSQLClient)

    client = connection.connect(profile="dev")
    assert captured_uris == ["grpc://dev-host:50051"]
    assert isinstance(client, FakeFlightSQLClient)
