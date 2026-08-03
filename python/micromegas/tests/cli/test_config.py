import json
from pathlib import Path

import pytest

from micromegas.cli.config import (
    ConnectionConfig,
    DEFAULT_URI,
    ProfileError,
    default_token_file,
    load_config,
    resolve_active_profile,
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


def test_resolve_no_config_no_env(tmp_path):
    missing = tmp_path / "nonexistent.json"
    cfg = resolve_connection(config_path=missing)
    assert cfg.uri == DEFAULT_URI
    assert cfg.oidc_issuer is None
    assert cfg.oidc_client_id is None


def test_resolve_reads_config_file(tmp_path):
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

    cfg = resolve_connection(config_path=cfg_file)
    assert cfg.uri == "grpc://env-host:9999"
    assert cfg.oidc_issuer == "https://env-issuer.com"
    assert cfg.oidc_client_id == "env-client"
    assert cfg.oidc_audience == "env-aud"


def test_uri_from_env_without_oidc(tmp_path, monkeypatch):
    monkeypatch.setenv("MICROMEGAS_ANALYTICS_URI", "grpc://remote:50051")

    missing = tmp_path / "nonexistent.json"
    cfg = resolve_connection(config_path=missing)
    assert cfg.uri == "grpc://remote:50051"
    assert cfg.oidc_issuer is None
    assert cfg.oidc_client_id is None


def test_config_without_issuers(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {"uri": "grpc://simple-host:50051"}
    cfg_file.write_text(json.dumps(data))

    cfg = resolve_connection(config_path=cfg_file)
    assert cfg.uri == "grpc://simple-host:50051"
    assert cfg.oidc_issuer is None
    assert cfg.oidc_audience is None


# --- Named profiles ---


def test_flat_config_behaves_as_before(tmp_path):
    """A config with no `profiles` key is a regression check: it resolves
    exactly like before profiles existed, with no profile name involved."""
    cfg_file = tmp_path / "config.json"
    data = {
        "uri": "grpc+tls://flat-host:50051",
        "client_id": "flat-client",
        "issuers": [{"issuer": "https://flat-issuer.com", "audience": "flat-aud"}],
    }
    cfg_file.write_text(json.dumps(data))

    config = load_config(cfg_file)
    name, active = resolve_active_profile(config)
    assert name is None
    assert active is config

    cfg = resolve_connection(config_path=cfg_file)
    assert cfg.uri == "grpc+tls://flat-host:50051"
    assert cfg.oidc_client_id == "flat-client"
    assert cfg.oidc_issuer == "https://flat-issuer.com"
    assert cfg.token_file == default_token_file(None)


def test_profiles_map_uses_default_profile(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": "prod",
        "profiles": {
            "prod": {"uri": "grpc+tls://prod-host:50051"},
            "dev": {"uri": "grpc://dev-host:50051"},
        },
    }
    cfg_file.write_text(json.dumps(data))

    cfg = resolve_connection(config_path=cfg_file)
    assert cfg.uri == "grpc+tls://prod-host:50051"


def test_profile_argument_overrides_default_profile(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": "prod",
        "profiles": {
            "prod": {"uri": "grpc+tls://prod-host:50051"},
            "dev": {"uri": "grpc://dev-host:50051"},
        },
    }
    cfg_file.write_text(json.dumps(data))

    cfg = resolve_connection(config_path=cfg_file, profile="dev")
    assert cfg.uri == "grpc://dev-host:50051"


def test_env_profile_overrides_default_but_loses_to_argument(tmp_path, monkeypatch):
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": "prod",
        "profiles": {
            "prod": {"uri": "grpc+tls://prod-host:50051"},
            "dev": {"uri": "grpc://dev-host:50051"},
            "local": {"uri": "grpc://localhost:50051"},
        },
    }
    cfg_file.write_text(json.dumps(data))

    monkeypatch.setenv("MICROMEGAS_PROFILE", "dev")
    cfg = resolve_connection(config_path=cfg_file)
    assert cfg.uri == "grpc://dev-host:50051"

    # An explicit --profile argument still wins over MICROMEGAS_PROFILE.
    cfg = resolve_connection(config_path=cfg_file, profile="local")
    assert cfg.uri == "grpc://localhost:50051"


def test_unknown_profile_raises_profile_error(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": "prod",
        "profiles": {"prod": {"uri": "grpc+tls://prod-host:50051"}},
    }
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError) as e:
        resolve_connection(config_path=cfg_file, profile="nope")
    assert "nope" in str(e.value)
    assert "prod" in str(e.value)


def test_no_profile_selected_raises_profile_error(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "profiles": {
            "prod": {"uri": "grpc+tls://prod-host:50051"},
            "dev": {"uri": "grpc://dev-host:50051"},
        },
    }
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError) as e:
        resolve_connection(config_path=cfg_file)
    assert "dev" in str(e.value)
    assert "prod" in str(e.value)


def test_env_vars_still_win_over_active_profile(tmp_path, monkeypatch):
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": "prod",
        "profiles": {
            "prod": {
                "uri": "grpc+tls://prod-host:50051",
                "client_id": "prod-client",
                "issuers": [
                    {"issuer": "https://prod-issuer.com", "audience": "prod-aud"}
                ],
            },
        },
    }
    cfg_file.write_text(json.dumps(data))

    monkeypatch.setenv("MICROMEGAS_ANALYTICS_URI", "grpc://env-host:9999")
    cfg = resolve_connection(config_path=cfg_file)
    assert cfg.uri == "grpc://env-host:9999"
    assert cfg.oidc_client_id == "prod-client"
    assert cfg.oidc_issuer == "https://prod-issuer.com"


def test_default_token_file_no_profile_returns_plain_default():
    assert Path(default_token_file(None)) == Path.home() / ".micromegas" / "tokens.json"
    assert "tokens-" not in default_token_file(None)


def test_default_token_file_with_profile_returns_suffixed_path():
    path = default_token_file("prod")
    assert Path(path) == Path.home() / ".micromegas" / "tokens-prod.json"


def test_profile_argument_against_flat_config_raises_profile_error(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {"uri": "grpc://simple-host:50051"}
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file, profile="prod")


def test_env_profile_against_flat_config_raises_profile_error(tmp_path, monkeypatch):
    cfg_file = tmp_path / "config.json"
    data = {"uri": "grpc://simple-host:50051"}
    cfg_file.write_text(json.dumps(data))

    monkeypatch.setenv("MICROMEGAS_PROFILE", "prod")
    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file)


def test_profiles_not_a_map_raises_profile_error(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {"profiles": "prod"}
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file)


def test_profiles_list_raises_profile_error(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {"profiles": ["prod", "dev"]}
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file)


def test_profile_entry_not_a_map_raises_profile_error(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": "prod",
        "profiles": {"prod": "grpc://h:1"},
    }
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file)


def test_profile_entry_list_raises_profile_error(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": "prod",
        "profiles": {"prod": ["grpc://h:1"]},
    }
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file)


def test_default_profile_list_raises_profile_error(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": [],
        "profiles": {"prod": {"uri": "grpc://h:1"}},
    }
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file)


def test_default_profile_dict_raises_profile_error(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": {},
        "profiles": {"prod": {"uri": "grpc://h:1"}},
    }
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file)


def test_resolve_connection_uses_per_profile_token_file(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": "dev",
        "profiles": {"dev": {"uri": "grpc://dev-host:50051"}},
    }
    cfg_file.write_text(json.dumps(data))

    cfg = resolve_connection(config_path=cfg_file)
    assert cfg.token_file == default_token_file("dev")
    assert cfg.token_file != default_token_file(None)


# --- Profile name validation ---


def test_default_token_file_rejects_empty_string():
    with pytest.raises(ProfileError):
        default_token_file("")


def test_default_token_file_rejects_path_separator():
    with pytest.raises(ProfileError):
        default_token_file("team/prod")


def test_default_token_file_rejects_path_traversal():
    with pytest.raises(ProfileError):
        default_token_file("../../../evil")


def test_default_token_file_rejects_dot_and_dotdot():
    with pytest.raises(ProfileError):
        default_token_file(".")
    with pytest.raises(ProfileError):
        default_token_file("..")


def test_profile_argument_empty_string_raises_profile_error(tmp_path):
    """An explicit but empty --profile must error, not silently fall back to
    MICROMEGAS_PROFILE/default_profile (regression guard for the falsy-empty
    bug)."""
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": "prod",
        "profiles": {"prod": {"uri": "grpc+tls://prod-host:50051"}},
    }
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file, profile="")


def test_profile_argument_empty_string_raises_profile_error_flat_config(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {"uri": "grpc://simple-host:50051"}
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file, profile="")


def test_profile_argument_with_path_separator_raises_profile_error(tmp_path):
    cfg_file = tmp_path / "config.json"
    data = {
        "profiles": {"team/prod": {"uri": "grpc+tls://prod-host:50051"}},
    }
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file, profile="team/prod")


def test_default_profile_with_path_traversal_raises_profile_error(tmp_path):
    """Even a name coming from `default_profile` (not just --profile) must be
    validated, since it also gets interpolated into the token file path."""
    cfg_file = tmp_path / "config.json"
    data = {
        "default_profile": "../../../evil",
        "profiles": {"../../../evil": {"uri": "grpc://h:1"}},
    }
    cfg_file.write_text(json.dumps(data))

    with pytest.raises(ProfileError):
        resolve_connection(config_path=cfg_file)


def test_token_file_env_var_has_no_effect(tmp_path, monkeypatch):
    """MICROMEGAS_TOKEN_FILE was removed; setting it must not change
    resolve_connection()'s output (regression guard for the removal)."""
    monkeypatch.setenv("MICROMEGAS_TOKEN_FILE", "/tmp/should-be-ignored.json")

    missing = tmp_path / "nonexistent.json"
    cfg = resolve_connection(config_path=missing)
    assert cfg.token_file == default_token_file(None)
    assert cfg.token_file != "/tmp/should-be-ignored.json"
