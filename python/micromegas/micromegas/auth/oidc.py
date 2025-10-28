"""OIDC authentication provider with automatic token refresh."""

import json
import os
import threading
import time
import webbrowser
from pathlib import Path
from typing import Optional
from urllib.parse import parse_qs

import requests
from authlib.integrations.requests_client import OAuth2Session


class OidcAuthProvider:
    """OIDC authentication provider with automatic token refresh.

    Uses authlib for OIDC flows (discovery, PKCE, token refresh).
    Supports browser-based login and token persistence.

    Supports two OAuth client types:
    - Desktop app: No client_secret, uses PKCE only (for CLI tools)
    - Web application: With client_secret, uses PKCE + secret (for web apps)

    Example (Desktop app - CLI/local):
        >>> # First time login (opens browser)
        >>> auth = OidcAuthProvider.login(
        ...     issuer="https://accounts.google.com",
        ...     client_id="your-desktop-app-id.apps.googleusercontent.com"
        ... )

    Example (Web app - server-side):
        >>> # Login with client_secret for web app
        >>> auth = OidcAuthProvider.login(
        ...     issuer="https://accounts.google.com",
        ...     client_id="your-web-app-id.apps.googleusercontent.com",
        ...     client_secret="your-client-secret"  # Store securely on server
        ... )
    """

    def __init__(
        self,
        issuer: str,
        client_id: str,
        client_secret: Optional[str] = None,
        token_file: Optional[str] = None,
        token: Optional[dict] = None,
    ):
        """Initialize OIDC auth provider.

        Args:
            issuer: OIDC issuer URL (e.g., "https://accounts.google.com")
            client_id: Client ID from identity provider
            client_secret: Client secret (optional, only for Web application clients)
            token_file: Path to save/load tokens (default: ~/.micromegas/tokens.json)
            token: Pre-loaded token dict (for testing or manual token management)
        """
        self.issuer = issuer
        self.client_id = client_id
        self.client_secret = client_secret
        self.token_file = token_file or str(Path.home() / ".micromegas" / "tokens.json")
        self._lock = threading.Lock()  # Thread-safe token refresh

        # Fetch OIDC metadata via discovery
        self.metadata = self._fetch_oidc_metadata(issuer)

        # Create OAuth2Session with discovered endpoints
        # Use appropriate auth method based on whether client_secret is provided
        auth_method = "client_secret_post" if client_secret else "none"
        # Include offline_access for Azure AD refresh tokens
        self.client = OAuth2Session(
            client_id=client_id,
            client_secret=client_secret,
            scope="openid email profile offline_access",
            token=token,
            token_endpoint_auth_method=auth_method,
        )

        # Set token if provided
        if token:
            self.client.token = token

    @staticmethod
    def _fetch_oidc_metadata(issuer: str) -> dict:
        """Fetch OIDC discovery metadata from issuer.

        Args:
            issuer: OIDC issuer URL

        Returns:
            Dictionary containing OIDC metadata (endpoints, etc.)
        """
        discovery_url = f"{issuer}/.well-known/openid-configuration"
        response = requests.get(discovery_url, timeout=10)
        response.raise_for_status()
        return response.json()

    @classmethod
    def login(
        cls,
        issuer: str,
        client_id: str,
        client_secret: Optional[str] = None,
        token_file: Optional[str] = None,
        redirect_uri: str = "http://localhost:48080/callback",
    ) -> "OidcAuthProvider":
        """Perform browser-based OIDC login flow.

        Args:
            issuer: OIDC issuer URL
            client_id: Client ID from identity provider
            client_secret: Client secret (optional, for Web application clients)
            token_file: Where to save tokens after login
            redirect_uri: Local callback URI for OAuth redirect

        Returns:
            OidcAuthProvider with valid tokens

        Example (Desktop app):
            >>> auth = OidcAuthProvider.login(
            ...     issuer="https://accounts.google.com",
            ...     client_id="desktop-app-id.apps.googleusercontent.com"
            ... )

        Example (Web app):
            >>> auth = OidcAuthProvider.login(
            ...     issuer="https://accounts.google.com",
            ...     client_id="web-app-id.apps.googleusercontent.com",
            ...     client_secret="your-secret-here"  # Store securely
            ... )
        """
        # Fetch OIDC metadata
        metadata = cls._fetch_oidc_metadata(issuer)

        # Create temporary session for login
        auth_method = "client_secret_post" if client_secret else "none"
        # Include offline_access for Azure AD refresh tokens
        temp_client = OAuth2Session(
            client_id=client_id,
            client_secret=client_secret,
            scope="openid email profile offline_access",
            redirect_uri=redirect_uri,
            token_endpoint_auth_method=auth_method,
        )

        # Perform authorization code flow with PKCE
        token = cls._perform_auth_flow(
            temp_client, metadata, redirect_uri, client_secret
        )

        # Create provider with token
        provider = cls(issuer, client_id, client_secret, token_file, token=token)

        # Save tokens if file specified
        if token_file:
            provider.save()

        return provider

    @staticmethod
    def _perform_auth_flow(
        client: OAuth2Session,
        metadata: dict,
        redirect_uri: str,
        client_secret: Optional[str] = None,
    ) -> dict:
        """Perform authorization code flow with PKCE using authlib.

        Args:
            client: Configured OAuth2Session
            metadata: OIDC provider metadata
            redirect_uri: Local callback URI
            client_secret: Optional client secret for Web application clients

        Returns:
            Token dict with access_token, id_token, refresh_token, etc.
        """
        import http.server
        import socketserver

        # Generate authorization URL with PKCE
        # For Azure AD compatibility, we need to ensure PKCE parameters are always included
        from authlib.common.security import generate_token
        from authlib.oauth2.rfc7636 import create_s256_code_challenge

        # Generate PKCE code_verifier (43-128 character random string)
        code_verifier = generate_token(48)
        code_challenge = create_s256_code_challenge(code_verifier)

        # Create authorization URL with explicit PKCE parameters
        auth_url, state = client.create_authorization_url(
            metadata["authorization_endpoint"],
            code_verifier=code_verifier,
            code_challenge=code_challenge,
            code_challenge_method="S256",
        )

        # Start local callback server
        auth_code = None
        callback_port = int(redirect_uri.split(":")[-1].split("/")[0])

        class CallbackHandler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                nonlocal auth_code

                # Parse authorization code from query string
                query = parse_qs(self.path.split("?")[1] if "?" in self.path else "")
                received_state = query.get("state", [None])[0]

                # Validate state parameter to prevent CSRF attacks
                if received_state != state:
                    self.send_response(400)
                    self.send_header("Content-type", "text/html; charset=utf-8")
                    self.send_header("X-Content-Type-Options", "nosniff")
                    self.send_header("X-Frame-Options", "DENY")
                    self.send_header("Content-Security-Policy", "default-src 'none'")
                    self.end_headers()
                    self.wfile.write(
                        b"<html><body><h1>Invalid state parameter</h1>"
                        b"<p>Authentication failed due to invalid state. This may indicate a CSRF attack.</p></body></html>"
                    )
                    return

                # Only extract code after state validation succeeds
                auth_code = query.get("code", [None])[0]

                # Send response to browser
                self.send_response(200)
                self.send_header("Content-type", "text/html; charset=utf-8")
                self.send_header("X-Content-Type-Options", "nosniff")
                self.send_header("X-Frame-Options", "DENY")
                self.send_header("Content-Security-Policy", "default-src 'none'")
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
        server = None
        try:
            server = socketserver.TCPServer(("", callback_port), CallbackHandler)
            # Allow reusing the address to avoid "Address already in use" errors
            server.allow_reuse_address = True

            server_thread = threading.Thread(target=server.handle_request)
            server_thread.daemon = True
            server_thread.start()

            # Open browser for user authentication
            print(f"Opening browser for authentication...")
            webbrowser.open(auth_url)

            # Wait for callback
            server_thread.join(timeout=300)  # 5 minute timeout

            if not auth_code:
                raise Exception(
                    "Authentication failed - no authorization code received"
                )

            # Exchange authorization code for tokens with PKCE code_verifier
            # Note: PKCE works with both Desktop app (no secret) and Web app (with secret)
            token = client.fetch_token(
                metadata["token_endpoint"],
                authorization_response=f"{redirect_uri}?code={auth_code}&state={state}",
                code_verifier=code_verifier,
            )
        finally:
            # Always close the server to release the port
            if server:
                try:
                    server.server_close()
                except Exception:
                    pass  # Best effort cleanup

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
        """Save tokens to file with secure permissions.

        Note: client_secret is NOT saved (for security).
        When loading from file for web apps, provide client_secret separately.
        """
        # Create parent directory with secure permissions (0700)
        parent_dir = Path(self.token_file).parent
        parent_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        # Ensure permissions even if directory already exists
        parent_dir.chmod(0o700)

        # Create file with secure permissions atomically (0600)
        # This prevents race condition where file could be world-readable
        fd = os.open(
            self.token_file,
            os.O_CREAT | os.O_WRONLY | os.O_TRUNC,
            0o600,
        )

        # Save token with metadata (but NOT client_secret for security)
        with os.fdopen(fd, "w") as f:
            json.dump(
                {
                    "issuer": self.issuer,
                    "client_id": self.client_id,
                    "token": self.client.token,  # authlib's token dict
                    # client_secret intentionally not saved for security
                },
                f,
                indent=2,
            )

    @classmethod
    def from_file(
        cls, token_file: str, client_secret: Optional[str] = None
    ) -> "OidcAuthProvider":
        """Load tokens from file.

        Args:
            token_file: Path to token file
            client_secret: Optional client secret (for Web application clients)
                          Must be provided separately for security (not saved in file)

        Returns:
            OidcAuthProvider with loaded tokens

        Raises:
            FileNotFoundError: If token file doesn't exist
            Exception: If token refresh fails

        Example (Desktop app):
            >>> auth = OidcAuthProvider.from_file("~/.micromegas/tokens.json")

        Example (Web app):
            >>> # client_secret from environment or config, not from token file
            >>> client_secret = os.environ["GOOGLE_CLIENT_SECRET"]
            >>> auth = OidcAuthProvider.from_file(
            ...     "~/.micromegas/tokens.json",
            ...     client_secret=client_secret
            ... )
        """
        token_file = str(Path(token_file).expanduser())

        with open(token_file) as f:
            data = json.load(f)

        return cls(
            issuer=data["issuer"],
            client_id=data["client_id"],
            client_secret=client_secret,  # Provided separately for security
            token_file=token_file,
            token=data["token"],  # authlib token dict
        )
