import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

CONFIG_PATH = Path.home() / ".micromegas" / "config.json"
DEFAULT_URI = "grpc://localhost:50051"


class ProfileError(ValueError):
    """Raised by `resolve_active_profile` for profile-selection problems only
    (unknown profile, none selected, or --profile/MICROMEGAS_PROFILE with no
    `profiles` map) — never for downstream connection failures. Subclassing
    `ValueError` keeps it compatible with any existing `except ValueError`
    handling of `load_config`'s JSON-decode error, while letting callers that
    only want profile-selection errors catch `ProfileError` specifically.
    """


def default_token_file(profile: Optional[str] = None) -> str:
    """Return the OIDC token cache path for `profile` (or the plain default when None)."""
    if profile:
        return str(Path.home() / ".micromegas" / f"tokens-{profile}.json")
    return str(Path.home() / ".micromegas" / "tokens.json")


@dataclass(frozen=True, slots=True)
class ConnectionConfig:
    uri: str = DEFAULT_URI
    oidc_issuer: Optional[str] = None
    oidc_client_id: Optional[str] = None
    oidc_client_secret: Optional[str] = None
    oidc_audience: Optional[str] = None
    oidc_scope: Optional[str] = None
    token_file: Optional[str] = None


def load_config(config_path=None):
    """Load ~/.micromegas/config.json, returning empty dict if absent.

    Raises ValueError with the offending path if the file exists but is not valid JSON.
    """
    path = Path(config_path) if config_path else CONFIG_PATH
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        raise ValueError(f"Invalid JSON in config file {path}: {e}") from e


def _pick(env_key: str, *fallbacks: Optional[str]) -> Optional[str]:
    """Return the env var (treating empty as unset), else the first non-empty fallback."""
    return os.environ.get(env_key) or next((v for v in fallbacks if v), None)


def resolve_active_profile(config, profile=None):
    """Resolve the active profile name and its connection dict from `config`.

    The name is picked with precedence: `profile` argument (the --profile
    flag) > `MICROMEGAS_PROFILE` > `default_profile`.

    Returns `(name, active_config)`; `name` is `None` when no `profiles` map
    exists and the flat config is used directly as `active_config`. Raises
    `ProfileError` if `--profile`/`MICROMEGAS_PROFILE` is set but there's no
    `profiles` map, if a `profiles` map exists but no profile is selected,
    or if the resolved name isn't in `profiles`.
    """
    profiles = config.get("profiles")
    if profiles is None:
        if profile or os.environ.get("MICROMEGAS_PROFILE"):
            raise ProfileError(
                "no profiles configured; remove --profile/MICROMEGAS_PROFILE "
                "or add a `profiles` map to the config file"
            )
        return None, config

    if not profiles:
        raise ProfileError("no profiles defined in the `profiles` map")

    name = (
        profile or os.environ.get("MICROMEGAS_PROFILE") or config.get("default_profile")
    )
    if name is None:
        raise ProfileError(
            "no profile selected; pass --profile, set MICROMEGAS_PROFILE, or "
            f"set default_profile (available: {', '.join(sorted(profiles))})"
        )
    if name not in profiles:
        raise ProfileError(
            f"unknown profile '{name}' (available: {', '.join(sorted(profiles))})"
        )
    return name, profiles[name]


def resolve_connection(config_path=None, profile=None) -> ConnectionConfig:
    """Build ConnectionConfig with priority: env vars > active profile > defaults."""
    config = load_config(config_path)
    name, active = resolve_active_profile(config, profile)

    issuers = active.get("issuers") or []
    issuer = issuers[0].get("issuer") if issuers else None
    audience = issuers[0].get("audience") if issuers else None

    return ConnectionConfig(
        uri=_pick("MICROMEGAS_ANALYTICS_URI", active.get("uri"), DEFAULT_URI),
        oidc_issuer=_pick("MICROMEGAS_OIDC_ISSUER", issuer),
        oidc_client_id=_pick("MICROMEGAS_OIDC_CLIENT_ID", active.get("client_id")),
        oidc_client_secret=_pick("MICROMEGAS_OIDC_CLIENT_SECRET"),
        oidc_audience=_pick("MICROMEGAS_OIDC_AUDIENCE", audience),
        oidc_scope=_pick("MICROMEGAS_OIDC_SCOPE"),
        token_file=default_token_file(name),
    )
