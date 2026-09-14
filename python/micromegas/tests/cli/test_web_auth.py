import json

import pytest

import micromegas.auth.oidc as oidc_module
import micromegas.oidc_connection as oidc_connection_module
from micromegas.cli.config import ProfileError
from micromegas.cli.web_auth import resolve_web_auth


def _write_config(tmp_path, data):
    cfg_file = tmp_path / "config.json"
    cfg_file.write_text(json.dumps(data))
    return cfg_file


def _patch_client_credentials(monkeypatch, sentinel="cc-provider"):
    monkeypatch.setattr(
        oidc_module.OidcClientCredentialsProvider,
        "from_env",
        classmethod(lambda cls: sentinel),
    )


def _patch_load_or_login(monkeypatch, sentinel="oidc-provider"):
    calls = {}

    def fake_load_or_login(**kwargs):
        calls.update(kwargs)
        return sentinel

    monkeypatch.setattr(oidc_connection_module, "load_or_login", fake_load_or_login)
    return calls


def test_api_key_only_profile_returns_diagnostic_without_opening_file(
    tmp_path, monkeypatch
):
    missing_key_file = tmp_path / "does-not-exist.key"
    cfg_file = _write_config(
        tmp_path,
        {
            "default_profile": "prod",
            "profiles": {
                "prod": {
                    "uri": "grpc+tls://prod-host:50051",
                    "api_key_file": str(missing_key_file),
                }
            },
        },
    )

    provider, diagnostic = resolve_web_auth(config_path=cfg_file)
    assert provider is None
    assert "api_key_file" in diagnostic
    assert "OIDC tokens only" in diagnostic
    assert "profile 'prod'" in diagnostic


def test_flat_config_api_key_only_uses_config_file_subject(tmp_path, monkeypatch):
    missing_key_file = tmp_path / "does-not-exist.key"
    cfg_file = _write_config(
        tmp_path,
        {"uri": "grpc+tls://host:50051", "api_key_file": str(missing_key_file)},
    )

    provider, diagnostic = resolve_web_auth(config_path=cfg_file)
    assert provider is None
    assert "api_key_file" in diagnostic
    assert "config file" in diagnostic
    assert "profile 'None'" not in diagnostic


def test_profile_oidc_pair_calls_load_or_login_with_profile_token_file(
    tmp_path, monkeypatch
):
    calls = _patch_load_or_login(monkeypatch)
    cfg_file = _write_config(
        tmp_path,
        {
            "default_profile": "prod",
            "profiles": {
                "prod": {
                    "uri": "grpc+tls://prod-host:50051",
                    "client_id": "prod-client",
                    "issuers": [
                        {"issuer": "https://issuer.example.com", "audience": "aud-1"}
                    ],
                }
            },
        },
    )

    provider, diagnostic = resolve_web_auth(config_path=cfg_file)
    assert diagnostic is None
    assert provider == "oidc-provider"
    assert calls["issuer"] == "https://issuer.example.com"
    assert calls["client_id"] == "prod-client"
    assert calls["audience"] == "aud-1"
    assert calls["token_file"].endswith("tokens-prod.json")


def test_full_env_triple_uses_client_credentials_not_load_or_login(
    tmp_path, monkeypatch
):
    _patch_client_credentials(monkeypatch)
    load_or_login_calls = _patch_load_or_login(monkeypatch)
    monkeypatch.setenv("MICROMEGAS_OIDC_ISSUER", "https://env-issuer.example.com")
    monkeypatch.setenv("MICROMEGAS_OIDC_CLIENT_ID", "env-client")
    monkeypatch.setenv("MICROMEGAS_OIDC_CLIENT_SECRET", "env-secret")
    missing = tmp_path / "nonexistent.json"

    provider, diagnostic = resolve_web_auth(config_path=missing)
    assert diagnostic is None
    assert provider == "cc-provider"
    assert load_or_login_calls == {}


def test_env_secret_alone_uses_load_or_login_not_client_credentials(
    tmp_path, monkeypatch
):
    """Pins the Decisions entry: the client-credentials branch is keyed on
    all three MICROMEGAS_OIDC_* env vars being set, not on the resolved
    client_secret -- issuer/client_id coming from the profile with only the
    secret from the env must still take the load_or_login branch."""
    _patch_client_credentials(monkeypatch, sentinel="cc-provider")
    calls = _patch_load_or_login(monkeypatch)
    monkeypatch.setenv("MICROMEGAS_OIDC_CLIENT_SECRET", "env-secret")
    cfg_file = _write_config(
        tmp_path,
        {
            "uri": "grpc+tls://host:50051",
            "client_id": "profile-client",
            "issuers": [{"issuer": "https://profile-issuer.example.com"}],
        },
    )

    provider, diagnostic = resolve_web_auth(config_path=cfg_file)
    assert diagnostic is None
    assert provider == "oidc-provider"
    assert calls["issuer"] == "https://profile-issuer.example.com"
    assert calls["client_id"] == "profile-client"
    assert calls["client_secret"] == "env-secret"


def test_env_issuer_overrides_profile_issuer_in_load_or_login_call(
    tmp_path, monkeypatch
):
    calls = _patch_load_or_login(monkeypatch)
    monkeypatch.setenv("MICROMEGAS_OIDC_ISSUER", "https://env-issuer.example.com")
    cfg_file = _write_config(
        tmp_path,
        {
            "uri": "grpc+tls://host:50051",
            "client_id": "profile-client",
            "issuers": [{"issuer": "https://profile-issuer.example.com"}],
        },
    )

    provider, diagnostic = resolve_web_auth(config_path=cfg_file)
    assert diagnostic is None
    assert calls["issuer"] == "https://env-issuer.example.com"


def test_two_auth_mechanisms_raises_profile_error(tmp_path, monkeypatch):
    key_file = tmp_path / "prod.key"
    key_file.write_text("mmk_secret\n", encoding="utf-8")
    cfg_file = _write_config(
        tmp_path,
        {
            "default_profile": "prod",
            "profiles": {
                "prod": {
                    "uri": "grpc+tls://prod-host:50051",
                    "client_id": "prod-client",
                    "issuers": [{"issuer": "https://issuer.example.com"}],
                    "api_key_file": str(key_file),
                }
            },
        },
    )

    with pytest.raises(ProfileError):
        resolve_web_auth(config_path=cfg_file)


def test_full_env_triple_with_unselected_profile_still_uses_client_credentials(
    tmp_path, monkeypatch
):
    """Pins step 1's ordering: the env-triple check runs before
    `resolve_connection`, so a `profiles` map with no profile selected never
    raises `ProfileError` when the full env triple is present -- this is
    what keeps grants/groups/import-keys behavior-compatible."""
    _patch_client_credentials(monkeypatch)
    cfg_file = _write_config(
        tmp_path,
        {
            "profiles": {
                "prod": {"uri": "grpc+tls://prod-host:50051"},
                "dev": {"uri": "grpc://dev-host:50051"},
            }
        },
    )
    monkeypatch.setenv("MICROMEGAS_OIDC_ISSUER", "https://env-issuer.example.com")
    monkeypatch.setenv("MICROMEGAS_OIDC_CLIENT_ID", "env-client")
    monkeypatch.setenv("MICROMEGAS_OIDC_CLIENT_SECRET", "env-secret")

    provider, diagnostic = resolve_web_auth(config_path=cfg_file)
    assert diagnostic is None
    assert provider == "cc-provider"


def test_nothing_configured_returns_generic_diagnostic(tmp_path):
    """No config file and no `profiles` map: the diagnostic names both the
    flat config-file keys and the `--profile`/`profiles` map escape hatch --
    it can't point at MICROMEGAS_OIDC_ISSUER/_CLIENT_ID as "the fix" here,
    since env vars are already checked ahead of this path and evidently
    weren't set."""
    missing = tmp_path / "nonexistent.json"

    provider, diagnostic = resolve_web_auth(config_path=missing)
    assert provider is None
    assert "no auth mechanism" in diagnostic
    assert "~/.micromegas/config.json" in diagnostic
    assert "client_id" in diagnostic
    assert "issuers[0].issuer" in diagnostic
    assert "profiles" in diagnostic
    assert "--profile" in diagnostic


def test_profile_resolving_neither_mechanism_names_profile_in_diagnostic(tmp_path):
    cfg_file = _write_config(
        tmp_path,
        {
            "default_profile": "staging",
            "profiles": {
                "staging": {"uri": "grpc+tls://staging-host:50051"},
            },
        },
    )

    provider, diagnostic = resolve_web_auth(config_path=cfg_file)
    assert provider is None
    assert "profile 'staging'" in diagnostic
