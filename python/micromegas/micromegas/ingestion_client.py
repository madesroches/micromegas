"""HTTP client for ingestion's OIDC-admin-gated key-management API (#1411).

A separate, small client from `WebClient`: ingestion is a different service
with a different base path (`/auth/api_keys`, not `/api/...`), so reusing
`WebClient` would be a misnomer. Carries the single operation the
`micromegas-import-keys` CLI tool needs -- importing a pre-existing key
string -- and calls ingestion directly with the operator's own bearer token,
never through `analytics-web-srv`'s proxy (that proxy exists only because the
*browser* can't hold a bearer token; a CLI process has no such restriction).
"""

import requests


class IngestionClient:
    """HTTP client for ingestion's `/auth/api_keys*` routes.

    Uses Bearer token authentication via an OIDC auth provider, same as
    `WebClient`.
    """

    DEFAULT_TIMEOUT = 30

    def __init__(self, base_url, auth_provider=None, timeout=None):
        self.base_url = base_url.rstrip("/")
        self.auth_provider = auth_provider
        self.timeout = timeout or self.DEFAULT_TIMEOUT
        self.session = requests.Session()

    def _headers(self):
        headers = {"Content-Type": "application/json"}
        if self.auth_provider:
            token = self.auth_provider.get_token()
            headers["Authorization"] = f"Bearer {token}"
        return headers

    def _check_response(self, resp):
        if not resp.ok:
            try:
                body = resp.json()
                msg = body.get("message", resp.text)
            except Exception:
                msg = resp.text
            raise RuntimeError(f"HTTP {resp.status_code}: {msg}")

    def import_ingestion_api_key(self, name, key):
        """Import an existing ingestion API key string.

        Hashes and stores `key` verbatim via `POST /auth/api_keys/import`
        rather than minting a fresh one. Response shape mirrors `mint_key`'s
        minus the cleartext: `{"key_id", "name", "created_at", "created_by",
        "revoked_at", "imported"}`.
        """
        resp = self.session.post(
            f"{self.base_url}/auth/api_keys/import",
            headers=self._headers(),
            json={"name": name, "key": key},
            timeout=self.timeout,
        )
        self._check_response(resp)
        return resp.json()
