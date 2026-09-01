"""Static-token authentication provider for pre-minted analytics API keys."""

import re
from pathlib import Path

# A bearer token is sent verbatim as a gRPC metadata value (see
# DynamicAuthMiddleware.sending_headers), which rejects non-ASCII or
# control characters with an opaque transport error at query time rather
# than a catchable Python exception -- so it's validated eagerly here
# instead, matching flightsql/attribution.py's client_entrypoint precedent.
_VALID_TOKEN_RE = re.compile(r"^[\x21-\x7e]+$")


class StaticTokenAuthProvider:
    """Sends a single, unchanging token as a Bearer token on every request.

    For a static analytics API key minted via `POST /api/analytics-api-keys`
    (or the Admin page) -- it travels verbatim as the bearer token, with no
    refresh and no OIDC flow. Pass an instance to `FlightSQLClient`'s or
    `WebClient`'s `auth_provider=` parameter.

    Example:
        >>> from micromegas.auth import StaticTokenAuthProvider
        >>> from micromegas.flightsql.client import FlightSQLClient  # doctest: +SKIP
        >>> auth = StaticTokenAuthProvider.from_file("~/.micromegas/local.key")  # doctest: +SKIP
        >>> client = FlightSQLClient("grpc+tls://analytics.example.com:50051", auth_provider=auth)  # doctest: +SKIP
    """

    def __init__(self, token: str):
        """Store `token` stripped of surrounding whitespace.

        Raises:
            ValueError: If `token` is not a string, is empty after stripping,
                or contains internal whitespace or a non-printable-ASCII
                character.
        """
        if not isinstance(token, str):
            raise ValueError(f"token must be a string, got {type(token).__name__}")
        token = token.strip()
        if not token:
            raise ValueError("token must not be empty")
        if not _VALID_TOKEN_RE.match(token):
            raise ValueError(
                "token must contain only printable ASCII characters with no "
                "internal whitespace"
            )
        # Held privately, and never included in __repr__, so a notebook that
        # echoes a cell's last expression -- and gets saved as a committed
        # .ipynb -- doesn't write a live credential into that output.
        self._token = token

    @classmethod
    def from_file(cls, path) -> "StaticTokenAuthProvider":
        """Read a token from `path`, expanding `~`, and strip it.

        `echo key > file` leaves a trailing newline, so stripping is the
        normal case, not a nicety.

        Raises:
            ValueError: If the file is empty after stripping. The message names `path`.
            OSError: If `path` cannot be read, propagated unchanged.
        """
        path = Path(path).expanduser()
        token = path.read_text(encoding="utf-8").strip()
        if not token:
            raise ValueError(f"token file '{path}' is empty")
        return cls(token)

    def get_token(self) -> str:
        """Return the stored token."""
        return self._token

    def __repr__(self) -> str:
        return f"{type(self).__name__}(token='***')"
