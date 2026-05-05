import json

from micromegas.cli.config import (
    ConnectionConfig,
    DEFAULT_URI,
    load_config,
    resolve_connection,
)


def test_load_config_missing_file(tmp_path):
    missing = tmp_path / "nonexistent.json"
    assert load_config(missing) == {}


def test_load_config_valid(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "issuers": [{"issuer": "https://example.com", "audience": "aud-1"}],
        "uri": "grpc+tls://remote:50051",
        "client_id": "my-client",
    }
    cfg_file.write_text(json.dumps(data))
    result = load_config(cfg_file)
    assert result["uri"] == "grpc+tls://remote:50051"
    assert result["client_id"] == "my-client"
    assert result["issuers"][0]["issuer"] == "https://example.com"


def test_resolve_no_config_no_env(tmp_path, monkeypatch):
    for var in [
        "MICROMEGAS_ANALYTICS_URI",
        "MICROMEGAS_OIDC_ISSUER",
        "MICROMEGAS_OIDC_CLIENT_ID",
        "MICROMEGAS_OIDC_CLIENT_SECRET",
        "MICROMEGAS_OIDC_AUDIENCE",
        "MICROMEGAS_OIDC_SCOPE",
        "MICROMEGAS_TOKEN_FILE",
        "MICROMEGAS_PYTHON_MODULE_WRAPPER",
    ]:
        monkeypatch.delenv(var, raising=False)
    missing = tmp_path / "nonexistent.json"
    cfg = resolve_connection(config_path=missing)
    assert cfg.uri == DEFAULT_URI
    assert cfg.oidc_issuer is None
    assert cfg.oidc_client_id is None


def test_resolve_reads_config_file(tmp_path, monkeypatch):
    for var in [
        "MICROMEGAS_ANALYTICS_URI",
        "MICROMEGAS_OIDC_ISSUER",
        "MICROMEGAS_OIDC_CLIENT_ID",
        "MICROMEGAS_OIDC_CLIENT_SECRET",
        "MICROMEGAS_OIDC_AUDIENCE",
        "MICROMEGAS_OIDC_SCOPE",
        "MICROMEGAS_TOKEN_FILE",
        "MICROMEGAS_PYTHON_MODULE_WRAPPER",
    ]:
        monkeypatch.delenv(var, raising=False)

    cfg_file = tmp_path / "config.json"
    data = {
        "issuers": [{"issuer": "https://idp.example.com", "audience": "aud-123"}],
        "uri": "grpc+tls://analytics.example.com:50051",
        "client_id": "app-client-id",
    }
    cfg_file.write_text(json.dumps(data))

    cfg = resolve_connection(config_path=cfg_file)
    assert cfg.uri == "grpc+tls://analytics.example.com:50051"
    assert cfg.oidc_issuer == "https://idp.example.com"
    assert cfg.oidc_client_id == "app-client-id"
    assert cfg.oidc_audience == "aud-123"


def test_env_vars_override_config(tmp_path, monkeypatch):
    cfg_file = tmp_path / "config.json"
    data = {
        "issuers": [{"issuer": "https://config-issuer.com", "audience": "config-aud"}],
        "uri": "grpc+tls://config-host:50051",
        "client_id": "config-client",
    }
    cfg_file.write_text(json.dumps(data))

    monkeypatch.setenv("MICROMEGAS_ANALYTICS_URI", "grpc://env-host:9999")
    monkeypatch.setenv("MICROMEGAS_OIDC_ISSUER", "https://env-issuer.com")
    monkeypatch.setenv("MICROMEGAS_OIDC_CLIENT_ID", "env-client")
    monkeypatch.setenv("MICROMEGAS_OIDC_AUDIENCE", "env-aud")
    monkeypatch.delenv("MICROMEGAS_OIDC_CLIENT_SECRET", raising=False)
    monkeypatch.delenv("MICROMEGAS_OIDC_SCOPE", raising=False)
    monkeypatch.delenv("MICROMEGAS_TOKEN_FILE", raising=False)
    monkeypatch.delenv("MICROMEGAS_PYTHON_MODULE_WRAPPER", raising=False)

    cfg = resolve_connection(config_path=cfg_file)
    assert cfg.uri == "grpc://env-host:9999"
    assert cfg.oidc_issuer == "https://env-issuer.com"
    assert cfg.oidc_client_id == "env-client"
    assert cfg.oidc_audience == "env-aud"


def test_uri_from_env_without_oidc(tmp_path, monkeypatch):
    for var in [
        "MICROMEGAS_OIDC_ISSUER",
        "MICROMEGAS_OIDC_CLIENT_ID",
        "MICROMEGAS_OIDC_CLIENT_SECRET",
        "MICROMEGAS_OIDC_AUDIENCE",
        "MICROMEGAS_OIDC_SCOPE",
        "MICROMEGAS_TOKEN_FILE",
        "MICROMEGAS_PYTHON_MODULE_WRAPPER",
    ]:
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setenv("MICROMEGAS_ANALYTICS_URI", "grpc://remote:50051")

    missing = tmp_path / "nonexistent.json"
    cfg = resolve_connection(config_path=missing)
    assert cfg.uri == "grpc://remote:50051"
    assert cfg.oidc_issuer is None
    assert cfg.oidc_client_id is None


def test_config_without_issuers(tmp_path, monkeypatch):
    for var in [
        "MICROMEGAS_ANALYTICS_URI",
        "MICROMEGAS_OIDC_ISSUER",
        "MICROMEGAS_OIDC_CLIENT_ID",
        "MICROMEGAS_OIDC_CLIENT_SECRET",
        "MICROMEGAS_OIDC_AUDIENCE",
        "MICROMEGAS_OIDC_SCOPE",
        "MICROMEGAS_TOKEN_FILE",
        "MICROMEGAS_PYTHON_MODULE_WRAPPER",
    ]:
        monkeypatch.delenv(var, raising=False)

    cfg_file = tmp_path / "config.json"
    data = {"uri": "grpc://simple-host:50051"}
    cfg_file.write_text(json.dumps(data))

    cfg = resolve_connection(config_path=cfg_file)
    assert cfg.uri == "grpc://simple-host:50051"
    assert cfg.oidc_issuer is None
    assert cfg.oidc_audience is None
