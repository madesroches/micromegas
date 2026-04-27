import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

CONFIG_PATH = Path.home() / ".micromegas_oidc_config.json"
DEFAULT_URI = "grpc://localhost:50051"


@dataclass
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
    """Load ~/.micromegas_oidc_config.json, returning empty dict if absent."""
    path = config_path or CONFIG_PATH
    if Path(path).exists():
        with open(path) as f:
            return json.load(f)
    return {}


def resolve_connection(config_path=None) -> ConnectionConfig:
    """Build ConnectionConfig with priority: env vars > config file > defaults."""
    config = load_config(config_path)

    issuer = None
    audience = None
    issuers = config.get("issuers", [])
    if issuers:
        issuer = issuers[0].get("issuer")
        audience = issuers[0].get("audience")

    token_file_default = str(Path.home() / ".micromegas" / "tokens.json")

    return ConnectionConfig(
        uri=os.environ.get("MICROMEGAS_ANALYTICS_URI")
        or config.get("uri")
        or DEFAULT_URI,
        oidc_issuer=os.environ.get("MICROMEGAS_OIDC_ISSUER") or issuer,
        oidc_client_id=os.environ.get("MICROMEGAS_OIDC_CLIENT_ID")
        or config.get("client_id"),
        oidc_client_secret=os.environ.get("MICROMEGAS_OIDC_CLIENT_SECRET"),
        oidc_audience=os.environ.get("MICROMEGAS_OIDC_AUDIENCE"),
        oidc_scope=os.environ.get("MICROMEGAS_OIDC_SCOPE"),
        token_file=os.environ.get("MICROMEGAS_TOKEN_FILE") or token_file_default,
        python_module_wrapper=os.environ.get("MICROMEGAS_PYTHON_MODULE_WRAPPER"),
    )
