import json

import micromegas.flightsql.client as flightsql_client
from micromegas.cli import config, connection


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
