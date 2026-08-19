"""HTTP client for analytics-web-srv REST API."""

import requests


class WebClient:
    """HTTP client for analytics-web-srv REST API.

    Uses Bearer token authentication via an OIDC auth provider.
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

    def _api_url(self, path):
        return f"{self.base_url}/api/{path}"

    def _check_response(self, resp):
        if not resp.ok:
            try:
                body = resp.json()
                msg = body.get("message", resp.text)
            except Exception:
                msg = resp.text
            raise RuntimeError(f"HTTP {resp.status_code}: {msg}")

    def list_screens(self):
        resp = self.session.get(
            self._api_url("screens"), headers=self._headers(), timeout=self.timeout
        )
        self._check_response(resp)
        return resp.json()

    def get_screen(self, name):
        resp = self.session.get(
            self._api_url(f"screens/{requests.utils.quote(name, safe='')}"),
            headers=self._headers(),
            timeout=self.timeout,
        )
        self._check_response(resp)
        return resp.json()

    def create_screen(
        self, name, screen_type, config, managed_by=None, folder_path=None
    ):
        payload = {
            "name": name,
            "screen_type": screen_type,
            "config": config,
        }
        if managed_by is not None:
            payload["managed_by"] = managed_by
        if folder_path is not None:
            payload["folder_path"] = folder_path
        resp = self.session.post(
            self._api_url("screens"),
            headers=self._headers(),
            json=payload,
            timeout=self.timeout,
        )
        self._check_response(resp)
        return resp.json()

    def update_screen(self, name, config, managed_by=None, folder_path=None):
        payload = {"config": config}
        if managed_by is not None:
            payload["managed_by"] = managed_by
        if folder_path is not None:
            payload["folder_path"] = folder_path
        resp = self.session.put(
            self._api_url(f"screens/{requests.utils.quote(name, safe='')}"),
            headers=self._headers(),
            json=payload,
            timeout=self.timeout,
        )
        self._check_response(resp)
        return resp.json()

    def delete_screen(self, name):
        resp = self.session.delete(
            self._api_url(f"screens/{requests.utils.quote(name, safe='')}"),
            headers=self._headers(),
            timeout=self.timeout,
        )
        self._check_response(resp)

    def import_ingestion_api_key(self, name, key, audience=None):
        """Import an existing ingestion API key string (#1458).

        Hashes and stores `key` verbatim via
        `POST /api/ingestion-api-keys/import` rather than minting a fresh one
        -- this is what lets a legacy env-keyring key's own string carry
        forward, since existing clients must keep presenting the same key.
        Mirrors `mint_key`'s response shape minus the cleartext:
        `{"key_id", "name", "created_at", "created_by", "revoked_at",
        "imported", "audience"}`.

        `audience` (#1372, AbAC Stage 4) is omitted from the request body when
        `None`, so the server applies its own default
        (`MICROMEGAS_DEFAULT_KEY_AUDIENCE`, falling back to `public`) rather
        than receiving an explicit `null`.
        """
        payload = {"name": name, "key": key}
        if audience is not None:
            payload["audience"] = audience
        resp = self.session.post(
            self._api_url("ingestion-api-keys/import"),
            headers=self._headers(),
            json=payload,
            timeout=self.timeout,
        )
        self._check_response(resp)
        return resp.json()

    def create_audience_grant(self, audience, axis, selector):
        """Create (or report the pre-existing) audience grant row (#1489, AbAC
        Stage 6a) via `POST /api/audience-grants`.

        `axis` is `"read"` or `"mint"`; `selector` is `"*"`, `"user:<id>"`, or
        `"group:<id>"` -- the server re-validates both. Returns
        `{"audience", "axis", "selector", "created_at", "created_by"}`; the
        response reports the pre-existing row's own fields when the grant
        already existed (the server answers `200` in that case, `201` for a
        fresh create -- `WebClient` doesn't surface the status code itself,
        only the body, matching every other create/import method here).
        """
        resp = self.session.post(
            self._api_url("audience-grants"),
            headers=self._headers(),
            json={"audience": audience, "axis": axis, "selector": selector},
            timeout=self.timeout,
        )
        self._check_response(resp)
        return resp.json()

    def list_audience_grants(self, audience=None, axis=None, limit=None, offset=None):
        """List audience grant rows via `GET /api/audience-grants`, optionally
        filtered by `audience`/`axis` and paginated with `limit`/`offset`.

        Returns a list of `{"audience", "axis", "selector", "created_at",
        "created_by"}` dicts, newest first.
        """
        params = {}
        if audience is not None:
            params["audience"] = audience
        if axis is not None:
            params["axis"] = axis
        if limit is not None:
            params["limit"] = limit
        if offset is not None:
            params["offset"] = offset
        resp = self.session.get(
            self._api_url("audience-grants"),
            headers=self._headers(),
            params=params,
            timeout=self.timeout,
        )
        self._check_response(resp)
        return resp.json()

    def delete_audience_grant(self, audience, axis, selector):
        """Delete one audience grant row via `DELETE /api/audience-grants`,
        keyed by its natural `(audience, axis, selector)` triple passed as
        query parameters -- a `group:<id>` selector isn't restricted enough in
        charset to be a safe raw path segment. Raises `RuntimeError` (via
        `_check_response`) with a 404 if no such row exists.
        """
        resp = self.session.delete(
            self._api_url("audience-grants"),
            headers=self._headers(),
            params={"audience": audience, "axis": axis, "selector": selector},
            timeout=self.timeout,
        )
        self._check_response(resp)

    def import_analytics_api_key(self, name, key):
        """Import an existing analytics API key string (#1411).

        Hashes and stores `key` verbatim via `POST /api/analytics-api-keys/import`
        rather than minting a fresh one -- this is what lets a legacy env-keyring
        key's own string carry forward, since existing clients must keep
        presenting the same key. Mirrors `mint_key`'s response shape minus the
        cleartext: `{"key_id", "name", "created_at", "created_by", "revoked_at",
        "imported"}`.
        """
        resp = self.session.post(
            self._api_url("analytics-api-keys/import"),
            headers=self._headers(),
            json={"name": name, "key": key},
            timeout=self.timeout,
        )
        self._check_response(resp)
        return resp.json()
