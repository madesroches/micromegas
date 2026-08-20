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

    def mint_ingestion_api_key(self, name, audience=None):
        """Mint a fresh ingestion API key (AbAC Stage 6, #1374) via
        `POST /api/ingestion-api-keys`.

        Unlike `import_ingestion_api_key`, this generates a brand-new key
        server-side rather than carrying one forward -- the route is no
        longer purely admin-gated: a non-admin caller with a matching
        `mint` grant (or naming a brand-new audience explicitly, which
        lazily claims it) can mint their own key once the deployment has
        `MICROMEGAS_SELF_SERVICE_MINT` enabled. `audience` is omitted from
        the request body when `None`, so the server applies its own
        default/authorization rules rather than receiving an explicit
        `null`.

        Returns the mint response dict, including the one-time cleartext
        `key` -- never retrievable again after this call returns.

        On a `409` with `{"code": "CLAIM_CONTENDED"}` -- transient
        advisory-lock contention with another concurrent claim of the same
        brand-new audience, not a denial (analytics-web-srv's own
        `IngestionKeyError::Conflict`) -- this retries the same POST
        exactly once. `_check_response` (used for every other status,
        including a second `CLAIM_CONTENDED`) discards the response body's
        `code` field and only ever raises a bare `RuntimeError`, so it
        can't tell "retry" apart from a genuine denial (`403 FORBIDDEN`)
        on its own; this method inspects `resp.status_code`/`resp.json()`
        itself, before calling `_check_response`, for exactly that reason.
        """
        payload = {"name": name}
        if audience is not None:
            payload["audience"] = audience

        def post():
            return self.session.post(
                self._api_url("ingestion-api-keys"),
                headers=self._headers(),
                json=payload,
                timeout=self.timeout,
            )

        resp = post()
        if resp.status_code == 409:
            try:
                code = resp.json().get("code")
            except Exception:
                code = None
            if code == "CLAIM_CONTENDED":
                resp = post()
        self._check_response(resp)
        return resp.json()

    def my_audiences(self):
        """List the audiences the caller may mint into today (AbAC Stage 6,
        #1374) via `GET /api/audience-grants/my-audiences`.

        Caller-scoped, so no admin access is required -- this reveals only
        whether *this* caller's own email/groups match a mint selector,
        plus facts about the caller's own identity. Returns
        `{"is_admin", "audiences", "mint_prefix", "email"}`:

        - `is_admin`: whether the caller is an admin (no other route
          reachable with a Bearer token exposes this).
        - `audiences`: the audiences whose `mint` selectors match this
          caller today (meaningless for an admin, whose mint authority
          never depends on a grant row at all -- see `is_admin` instead).
        - `mint_prefix`: the caller-derived namespace prefix a fresh,
          non-admin claim should be minted under, or `None` if the caller
          has no email (and so cannot claim at all).
        - `email`: the caller's own email, or `None`.
        """
        resp = self.session.get(
            self._api_url("audience-grants/my-audiences"),
            headers=self._headers(),
            timeout=self.timeout,
        )
        self._check_response(resp)
        return resp.json()

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
