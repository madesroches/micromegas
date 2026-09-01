"""Static-token authentication provider for pre-minted analytics API keys."""

from pathlib import Path


class StaticTokenAuthProvider:
    """Sends a single, unchanging token as a Bearer token on every request.

    For a static analytics API key minted via `POST /api/analytics-api-keys`
    (or the Admin page) -- it travels verbatim as the bearer token, with no
    refresh and no OIDC flow. Pass an instance to `FlightSQLClient`'s or
    `WebClient`'s `auth_provider=` parameter.

    Example:
        >>> from micromegas.auth import StaticTokenAuthProvider
        >>> auth = StaticTokenAuthProvider.from_file("~/.micromegas/local.key")
        >>> client = FlightSQLClient("grpc+tls://analytics.example.com:50051", auth_provider=auth)
    """

    def __init__(self, token: str):
        """Store `token` stripped of surrounding whitespace.

        Raises:
            ValueError: If `token` is not a string, or is empty after stripping.
        """
        if not isinstance(token, str):
            raise ValueError(f"token must be a string, got {type(token).__name__}")
        token = token.strip()
        if not token:
            raise ValueError("token must not be empty")
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
