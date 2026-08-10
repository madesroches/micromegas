import json

import micromegas.flightsql.client as flightsql_client
from micromegas import oidc_connection
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
    captured_entrypoints = []

    class FakeFlightSQLClient:
        def __init__(self, uri, client_entrypoint=None):
            captured_uris.append(uri)
            captured_entrypoints.append(client_entrypoint)

    monkeypatch.setattr(flightsql_client, "FlightSQLClient", FakeFlightSQLClient)

    client = connection.connect(profile="dev", client_entrypoint="cli-query")
    assert captured_uris == ["grpc://dev-host:50051"]
    assert captured_entrypoints == ["cli-query"]
    assert isinstance(client, FakeFlightSQLClient)


def test_profile_argument_forwards_client_entrypoint_through_oidc_connect(
    tmp_path, monkeypatch
):
    cfg_file = tmp_path / "config.json"
    cfg_file.write_text(
        json.dumps(
            {
                "default_profile": "oidc-profile",
                "profiles": {
                    "oidc-profile": {
                        "uri": "grpc+tls://oidc-host:50051",
                        "client_id": "my-client-id",
                        "issuers": [
                            {
                                "issuer": "https://issuer.example.com",
                                "audience": "my-audience",
                            }
                        ],
                    },
                },
            }
        )
    )
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_file)

    captured_kwargs = {}

    class FakeOidcClient:
        pass

    def fake_oidc_connect(**kwargs):
        captured_kwargs.update(kwargs)
        return FakeOidcClient()

    monkeypatch.setattr(oidc_connection, "connect", fake_oidc_connect)

    client = connection.connect(profile="oidc-profile", client_entrypoint="cli-query")
    assert captured_kwargs["client_entrypoint"] == "cli-query"
    assert isinstance(client, FakeOidcClient)
