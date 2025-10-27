"""OIDC authentication provider with automatic token refresh."""

import json
import threading
import time
import webbrowser
from pathlib import Path
from typing import Optional
from urllib.parse import parse_qs

from authlib.integrations.requests_client import OAuth2Session


class OidcAuthProvider:
    """OIDC authentication provider with automatic token refresh.

    Uses authlib for OIDC flows (discovery, PKCE, token refresh).
    Supports browser-based login and token persistence.

    Example:
        >>> # First time login (opens browser)
        >>> auth = OidcAuthProvider.login(
        ...     issuer="https://accounts.google.com",
        ...     client_id="your-app-id.apps.googleusercontent.com"
        ... )
        >>>
        >>> # Get token for API calls
        >>> token = auth.get_token()
        >>>
        >>> # Save tokens for future sessions
        >>> auth.save()
        >>>
        >>> # Later sessions - load from file
        >>> auth = OidcAuthProvider.from_file("~/.micromegas/tokens.json")
    """

    def __init__(
        self,
        issuer: str,
        client_id: str,
        token_file: Optional[str] = None,
        token: Optional[dict] = None,
    ):
        """Initialize OIDC auth provider.

        Args:
            issuer: OIDC issuer URL (e.g., "https://accounts.google.com")
            client_id: Client ID from identity provider
            token_file: Path to save/load tokens (default: ~/.micromegas/tokens.json)
            token: Pre-loaded token dict (for testing or manual token management)
        """
        self.issuer = issuer
        self.client_id = client_id
        self.token_file = token_file or str(Path.home() / ".micromegas" / "tokens.json")
        self._lock = threading.Lock()  # Thread-safe token refresh

        # Create OAuth2Session with OIDC discovery
        self.client = OAuth2Session(
            client_id=client_id,
            scope="openid email profile",
            token=token,
            token_endpoint_auth_method="none",  # Public client (no client secret)
        )

        # Fetch OIDC metadata via discovery
        self.metadata = self.client.fetch_server_metadata(
            f"{issuer}/.well-known/openid-configuration"
        )

        # Set token if provided
        if token:
            self.client.token = token

    @classmethod
    def login(
        cls,
        issuer: str,
        client_id: str,
        token_file: Optional[str] = None,
        redirect_uri: str = "http://localhost:8080/callback",
    ) -> "OidcAuthProvider":
        """Perform browser-based OIDC login flow.

        Args:
            issuer: OIDC issuer URL
            client_id: Client ID from identity provider
            token_file: Where to save tokens after login
            redirect_uri: Local callback URI for OAuth redirect

        Returns:
            OidcAuthProvider with valid tokens

        Example:
            >>> auth = OidcAuthProvider.login(
            ...     issuer="https://accounts.google.com",
            ...     client_id="your-app.apps.googleusercontent.com"
            ... )
        """
        # Create temporary session for login
        temp_client = OAuth2Session(
            client_id=client_id,
            scope="openid email profile",
            redirect_uri=redirect_uri,
            token_endpoint_auth_method="none",
        )

        # Fetch OIDC metadata
        metadata = temp_client.fetch_server_metadata(
            f"{issuer}/.well-known/openid-configuration"
        )

        # Perform authorization code flow with PKCE
        token = cls._perform_auth_flow(temp_client, metadata, redirect_uri)

        # Create provider with token
        provider = cls(issuer, client_id, token_file, token=token)

        # Save tokens if file specified
        if token_file:
            provider.save()

        return provider

    @staticmethod
    def _perform_auth_flow(
        client: OAuth2Session, metadata: dict, redirect_uri: str
    ) -> dict:
        """Perform authorization code flow with PKCE using authlib.

        Args:
            client: Configured OAuth2Session
            metadata: OIDC provider metadata
            redirect_uri: Local callback URI

        Returns:
            Token dict with access_token, id_token, refresh_token, etc.
        """
        import http.server
        import socketserver

        # Generate authorization URL with PKCE (authlib handles code_challenge automatically)
        auth_url, state = client.create_authorization_url(
            metadata["authorization_endpoint"],
            code_challenge_method="S256",  # Use PKCE with S256
        )

        # Start local callback server
        auth_code = None
        callback_port = int(redirect_uri.split(":")[-1].split("/")[0])

        class CallbackHandler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                nonlocal auth_code

                # Parse authorization code from query string
                query = parse_qs(self.path.split("?")[1] if "?" in self.path else "")
                auth_code = query.get("code", [None])[0]

                # Send response to browser
                self.send_response(200)
                self.send_header("Content-type", "text/html")
                self.end_headers()

                if auth_code:
                    self.wfile.write(
                        b"<html><body><h1>Authentication successful!</h1>"
                        b"<p>You can close this window.</p></body></html>"
                    )
                else:
                    self.wfile.write(
                        b"<html><body><h1>Authentication failed</h1>"
                        b"<p>No authorization code received.</p></body></html>"
                    )

            def log_message(self, format, *args):
                pass  # Suppress logging

        # Start callback server
        server = socketserver.TCPServer(("", callback_port), CallbackHandler)
        server_thread = threading.Thread(target=server.handle_request)
        server_thread.daemon = True
        server_thread.start()

        # Open browser for user authentication
        print(f"Opening browser for authentication...")
        webbrowser.open(auth_url)

        # Wait for callback
        server_thread.join(timeout=300)  # 5 minute timeout
        server.server_close()

        if not auth_code:
            raise Exception("Authentication failed - no authorization code received")

        # Exchange authorization code for tokens (authlib handles code_verifier automatically)
        token = client.fetch_token(
            metadata["token_endpoint"],
            authorization_response=f"{redirect_uri}?code={auth_code}&state={state}",
        )

        return token

    def get_token(self) -> str:
        """Get valid ID token, refreshing if necessary.

        This method is called before each query by the FlightSQL client.
        Thread-safe for concurrent queries.

        Returns:
            Valid ID token for Authorization header

        Raises:
            Exception: If no tokens available or refresh fails
        """
        with self._lock:
            if not self.client.token:
                raise Exception("No tokens available. Please call login() first.")

            # Check if token needs refresh (5 min buffer)
            expires_at = self.client.token.get("expires_at", 0)
            if expires_at > time.time() + 300:
                # Token still valid
                return self.client.token["id_token"]

            # Token expired or expiring soon - refresh it
            if self.client.token.get("refresh_token"):
                try:
                    self._refresh_tokens()
                    return self.client.token["id_token"]
                except Exception as e:
                    raise Exception(
                        f"Token refresh failed: {e}. Please re-authenticate."
                    )
            else:
                raise Exception("No refresh token available. Please re-authenticate.")

    def _refresh_tokens(self):
        """Refresh access token using refresh token (authlib handles everything)."""
        # authlib automatically refreshes using refresh_token
        new_token = self.client.fetch_token(
            self.metadata["token_endpoint"],
            grant_type="refresh_token",
            refresh_token=self.client.token["refresh_token"],
        )

        # Update token (authlib updates self.client.token automatically)
        # Save updated tokens to file
        if self.token_file:
            self.save()

    def save(self):
        """Save tokens to file with secure permissions."""
        Path(self.token_file).parent.mkdir(parents=True, exist_ok=True)

        # Save token with metadata
        with open(self.token_file, "w") as f:
            json.dump(
                {
                    "issuer": self.issuer,
                    "client_id": self.client_id,
                    "token": self.client.token,  # authlib's token dict
                },
                f,
                indent=2,
            )

        # Set secure permissions (0600 - owner read/write only)
        Path(self.token_file).chmod(0o600)

    @classmethod
    def from_file(cls, token_file: str) -> "OidcAuthProvider":
        """Load tokens from file.

        Args:
            token_file: Path to token file

        Returns:
            OidcAuthProvider with loaded tokens

        Raises:
            FileNotFoundError: If token file doesn't exist
            Exception: If token refresh fails

        Example:
            >>> auth = OidcAuthProvider.from_file("~/.micromegas/tokens.json")
        """
        token_file = str(Path(token_file).expanduser())

        with open(token_file) as f:
            data = json.load(f)

        return cls(
            issuer=data["issuer"],
            client_id=data["client_id"],
            token_file=token_file,
            token=data["token"],  # authlib token dict
        )
