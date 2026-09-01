import json

import pytest

import micromegas
import micromegas.flightsql.client as flightsql_client
from micromegas import oidc_connection
from micromegas.cli import config, connection
from micromegas.cli.config import ProfileError


class FakeFlightSQLClient:
    def __init__(
        self, uri, client_entrypoint=None, preserve_dictionary=False, auth_provider=None
    ):
        self.uri = uri
        self.client_entrypoint = client_entrypoint
        self.preserve_dictionary = preserve_dictionary
        self.auth_provider = auth_provider


def _write_config(tmp_path, monkeypatch, data):
    cfg_file = tmp_path / "config.json"
    cfg_file.write_text(json.dumps(data))
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_file)
    return cfg_file


def test_profile_argument_resolves_to_that_profiles_uri(tmp_path, monkeypatch):
    _write_config(
        tmp_path,
        monkeypatch,
        {
            "default_profile": "prod",
            "profiles": {
                "prod": {"uri": "grpc+tls://prod-host:50051"},
                "dev": {"uri": "grpc://dev-host:50051"},
            },
        },
    )

    monkeypatch.setattr(flightsql_client, "FlightSQLClient", FakeFlightSQLClient)

    client = connection.connect(profile="dev", client_entrypoint="cli-query")
    assert client.uri == "grpc://dev-host:50051"
    assert client.client_entrypoint == "cli-query"
    assert client.preserve_dictionary is False
    assert client.auth_provider is None
    assert isinstance(client, FakeFlightSQLClient)


def test_profile_argument_forwards_client_entrypoint_through_oidc_connect(
    tmp_path, monkeypatch
):
    _write_config(
        tmp_path,
        monkeypatch,
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
        },
    )

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


def test_static_key_profile_builds_client_with_token_and_entrypoint(
    tmp_path, monkeypatch
):
    key_file = tmp_path / "prod.key"
    key_file.write_text("mmk_prod-secret-key\n", encoding="utf-8")

    _write_config(
        tmp_path,
        monkeypatch,
        {
            "default_profile": "prod",
            "profiles": {
                "prod": {
                    "uri": "grpc+tls://prod-host:50051",
                    "api_key_file": str(key_file),
                },
            },
        },
    )

    monkeypatch.setattr(flightsql_client, "FlightSQLClient", FakeFlightSQLClient)

    client = connection.connect(profile="prod", client_entrypoint="cli-query")
    assert isinstance(client, FakeFlightSQLClient)
    assert client.uri == "grpc+tls://prod-host:50051"
    assert client.client_entrypoint == "cli-query"
    assert client.auth_provider.get_token() == "mmk_prod-secret-key"


def test_preserve_dictionary_forwarded_on_no_auth_branch(tmp_path, monkeypatch):
    _write_config(
        tmp_path,
        monkeypatch,
        {"uri": "grpc://simple-host:50051"},
    )

    monkeypatch.setattr(flightsql_client, "FlightSQLClient", FakeFlightSQLClient)

    client = connection.connect(preserve_dictionary=True)
    assert client.preserve_dictionary is True


def test_preserve_dictionary_forwarded_on_static_key_branch(tmp_path, monkeypatch):
    key_file = tmp_path / "prod.key"
    key_file.write_text("mmk_prod-secret-key\n", encoding="utf-8")

    _write_config(
        tmp_path,
        monkeypatch,
        {"uri": "grpc+tls://prod-host:50051", "api_key_file": str(key_file)},
    )

    monkeypatch.setattr(flightsql_client, "FlightSQLClient", FakeFlightSQLClient)

    client = connection.connect(preserve_dictionary=True)
    assert client.preserve_dictionary is True


def test_preserve_dictionary_forwarded_on_oidc_branch(tmp_path, monkeypatch):
    _write_config(
        tmp_path,
        monkeypatch,
        {
            "uri": "grpc+tls://oidc-host:50051",
            "client_id": "my-client-id",
            "issuers": [{"issuer": "https://issuer.example.com"}],
        },
    )

    captured_kwargs = {}

    class FakeOidcClient:
        pass

    def fake_oidc_connect(**kwargs):
        captured_kwargs.update(kwargs)
        return FakeOidcClient()

    monkeypatch.setattr(oidc_connection, "connect", fake_oidc_connect)

    connection.connect(preserve_dictionary=True)
    assert captured_kwargs["preserve_dictionary"] is True


def test_connect_with_profile_is_cli_connection_connect_alias():
    assert micromegas.connect_with_profile is connection.connect


def test_missing_api_key_file_raises_profile_error(tmp_path, monkeypatch):
    _write_config(
        tmp_path,
        monkeypatch,
        {
            "uri": "grpc+tls://prod-host:50051",
            "api_key_file": str(tmp_path / "nonexistent.key"),
        },
    )

    with pytest.raises(ProfileError):
        connection.connect()


def test_empty_api_key_file_raises_profile_error(tmp_path, monkeypatch):
    key_file = tmp_path / "empty.key"
    key_file.write_text("", encoding="utf-8")

    _write_config(
        tmp_path,
        monkeypatch,
        {"uri": "grpc+tls://prod-host:50051", "api_key_file": str(key_file)},
    )

    with pytest.raises(ProfileError):
        connection.connect()
