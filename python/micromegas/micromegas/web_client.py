"""HTTP client for analytics-web-srv REST API."""

import requests


class WebClient:
    """HTTP client for analytics-web-srv REST API.

    Uses Bearer token authentication via an OIDC auth provider.
    """

    def __init__(self, base_url, auth_provider=None):
        self.base_url = base_url.rstrip("/")
        self.auth_provider = auth_provider
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
        resp = self.session.get(self._api_url("screens"), headers=self._headers())
        self._check_response(resp)
        return resp.json()

    def get_screen(self, name):
        resp = self.session.get(
            self._api_url(f"screens/{requests.utils.quote(name, safe='')}"),
            headers=self._headers(),
        )
        self._check_response(resp)
        return resp.json()

    def create_screen(self, name, screen_type, config, managed_by=None):
        payload = {
            "name": name,
            "screen_type": screen_type,
            "config": config,
        }
        if managed_by is not None:
            payload["managed_by"] = managed_by
        resp = self.session.post(
            self._api_url("screens"),
            headers=self._headers(),
            json=payload,
        )
        self._check_response(resp)
        return resp.json()

    def update_screen(self, name, config, managed_by=None):
        payload = {"config": config}
        if managed_by is not None:
            payload["managed_by"] = managed_by
        resp = self.session.put(
            self._api_url(f"screens/{requests.utils.quote(name, safe='')}"),
            headers=self._headers(),
            json=payload,
        )
        self._check_response(resp)
        return resp.json()

    def delete_screen(self, name):
        resp = self.session.delete(
            self._api_url(f"screens/{requests.utils.quote(name, safe='')}"),
            headers=self._headers(),
        )
        self._check_response(resp)
