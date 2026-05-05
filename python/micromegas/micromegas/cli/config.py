import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

CONFIG_PATH = Path.home() / ".micromegas" / "config.json"
DEFAULT_URI = "grpc://localhost:50051"
DEFAULT_TOKEN_FILE = str(Path.home() / ".micromegas" / "tokens.json")


@dataclass(frozen=True, slots=True)
class ConnectionConfig:
    uri: str = DEFAULT_URI
    oidc_issuer: Optional[str] = None
    oidc_client_id: Optional[str] = None
    oidc_client_secret: Optional[str] = None
    oidc_audience: Optional[str] = None
    oidc_scope: Optional[str] = None
    token_file: Optional[str] = None
    python_module_wrapper: Optional[str] = None


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


def resolve_connection(config_path=None) -> ConnectionConfig:
    """Build ConnectionConfig with priority: env vars > config file > defaults."""
    config = load_config(config_path)

    issuers = config.get("issuers") or []
    issuer = issuers[0].get("issuer") if issuers else None
    audience = issuers[0].get("audience") if issuers else None

    return ConnectionConfig(
        uri=_pick("MICROMEGAS_ANALYTICS_URI", config.get("uri"), DEFAULT_URI),
        oidc_issuer=_pick("MICROMEGAS_OIDC_ISSUER", issuer),
        oidc_client_id=_pick("MICROMEGAS_OIDC_CLIENT_ID", config.get("client_id")),
        oidc_client_secret=_pick("MICROMEGAS_OIDC_CLIENT_SECRET"),
        oidc_audience=_pick("MICROMEGAS_OIDC_AUDIENCE", audience),
        oidc_scope=_pick("MICROMEGAS_OIDC_SCOPE"),
        token_file=_pick("MICROMEGAS_TOKEN_FILE", DEFAULT_TOKEN_FILE),
        python_module_wrapper=_pick("MICROMEGAS_PYTHON_MODULE_WRAPPER"),
    )
