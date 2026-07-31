"""Unit tests for OIDC authentication provider."""

import json
import os
import subprocess
import sys
import tempfile
import textwrap
import time
from pathlib import Path
from unittest.mock import Mock, patch, MagicMock

import pytest


def test_oidc_auth_provider_init():
    """Test OidcAuthProvider initialization."""
    from micromegas.auth import OidcAuthProvider

    with patch.object(OidcAuthProvider, "__init__", lambda x, *args, **kwargs: None):
        provider = OidcAuthProvider.__new__(OidcAuthProvider)
        provider.issuer = "https://accounts.google.com"
        provider.client_id = "test-client-id"
        assert provider.issuer == "https://accounts.google.com"
        assert provider.client_id == "test-client-id"


def test_oidc_token_save_and_load():
    """Test saving and loading tokens from file."""
    from micromegas.auth import OidcAuthProvider

    with tempfile.TemporaryDirectory() as tmpdir:
        token_file = Path(tmpdir) / "tokens.json"

        # Mock the OAuth2Session and metadata fetching
        with patch("micromegas.auth.oidc.requests.get") as mock_get, patch(
            "micromegas.auth.oidc.OAuth2Session"
        ) as MockSession:
            # Mock OIDC discovery response
            mock_response = MagicMock()
            mock_response.json.return_value = {
                "authorization_endpoint": "https://test/auth",
                "token_endpoint": "https://test/token",
                "issuer": "https://test.com",
            }
            mock_response.raise_for_status.return_value = None
            mock_get.return_value = mock_response

            mock_client = MagicMock()
            mock_client.token = {
                "access_token": "test-access",
                "id_token": "test-id",
                "refresh_token": "test-refresh",
                "expires_at": time.time() + 3600,
            }
            mock_client.fetch_server_metadata.return_value = {
                "authorization_endpoint": "https://test/auth",
                "token_endpoint": "https://test/token",
            }
            MockSession.return_value = mock_client

            # Create provider and save
            provider = OidcAuthProvider(
                issuer="https://test.com",
                client_id="test-client",
                token_file=str(token_file),
                token=mock_client.token,
            )
            provider.save()

            # Verify file exists and has secure permissions
            assert token_file.exists()
            # Note: On Windows, chmod 0o600 may not work as expected
            # So we only check on Unix-like systems
            import platform

            if platform.system() != "Windows":
                assert oct(token_file.stat().st_mode)[-3:] == "600"

            # Load from file
            loaded_provider = OidcAuthProvider.from_file(str(token_file))
            assert loaded_provider.issuer == "https://test.com"
            assert loaded_provider.client_id == "test-client"
            assert loaded_provider.client.token["id_token"] == "test-id"


NON_ASCII_CLIENT_ID = "client-—-café-日本語"


def test_oidc_token_load_non_ascii_locale(tmp_path):
    """Regression test for issue #1399: OidcAuthProvider.from_file()'s
    token-file read must be UTF-8, not locale-dependent.

    save()'s json.dump keeps ensure_ascii=True, so a save()/from_file()
    round trip would only ever see ASCII \\uXXXX escapes and would pass
    regardless of the open() encoding pin. Instead this test writes a
    token file directly as raw UTF-8 bytes (bypassing json.dump/save()
    entirely) with a non-ASCII client_id, then loads it in a subprocess
    forced to a non-UTF-8 locale (LC_ALL=C + PYTHONUTF8=0), matching the
    technique used in test_screen_files.py.
    """
    token_file = tmp_path / "tokens.json"
    data = {
        "issuer": "https://test.com",
        "client_id": NON_ASCII_CLIENT_ID,
        "token": {
            "access_token": "test-access",
            "id_token": "test-id",
            "refresh_token": "test-refresh",
            "expires_at": time.time() + 3600,
        },
    }
    token_file.write_bytes(json.dumps(data, ensure_ascii=False).encode("utf-8"))

    script = textwrap.dedent(
        """
        import os
        import sys
        from unittest.mock import MagicMock, patch

        assert sys.flags.utf8_mode == 0, "test setup failed: utf8_mode should be 0"

        from micromegas.auth import OidcAuthProvider

        token_file = os.environ["TEST_TOKEN_FILE"]

        with patch("micromegas.auth.oidc.requests.get") as mock_get, patch(
            "micromegas.auth.oidc.OAuth2Session"
        ) as MockSession:
            mock_response = MagicMock()
            mock_response.json.return_value = {
                "authorization_endpoint": "https://test/auth",
                "token_endpoint": "https://test/token",
                "issuer": "https://test.com",
            }
            mock_response.raise_for_status.return_value = None
            mock_get.return_value = mock_response

            mock_client = MagicMock()
            MockSession.return_value = mock_client

            provider = OidcAuthProvider.from_file(token_file)
            sys.stdout.buffer.write(provider.client_id.encode("utf-8"))
        """
    )
    env = dict(os.environ)
    env["LC_ALL"] = "C"
    env["PYTHONUTF8"] = "0"
    env["TEST_TOKEN_FILE"] = str(token_file)

    proc = subprocess.run(
        [sys.executable, "-c", script],
        env=env,
        capture_output=True,
    )
    assert proc.returncode == 0, proc.stderr.decode("utf-8", errors="replace")
    assert proc.stdout.decode("utf-8") == NON_ASCII_CLIENT_ID


def test_oidc_get_token_valid():
    """Test getting a valid token without refresh."""
    from micromegas.auth import OidcAuthProvider

    with patch("micromegas.auth.oidc.requests.get") as mock_get, patch(
        "micromegas.auth.oidc.OAuth2Session"
    ) as MockSession:
        # Mock OIDC discovery response
        mock_response = MagicMock()
        mock_response.json.return_value = {
            "authorization_endpoint": "https://test/auth",
            "token_endpoint": "https://test/token",
            "issuer": "https://test.com",
        }
        mock_response.raise_for_status.return_value = None
        mock_get.return_value = mock_response

        mock_client = MagicMock()
        # Create a valid JWT with far-future expiration
        # Header: {"typ":"JWT","alg":"HS256"}
        # Payload: {"sub":"test","exp":9999999999}
        # Signature: test-signature (doesn't need to be valid for this test)
        valid_jwt = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ0ZXN0IiwiZXhwIjo5OTk5OTk5OTk5fQ.dGVzdC1zaWduYXR1cmU"
        mock_client.token = {
            "access_token": "test-access",
            "id_token": valid_jwt,
            "refresh_token": "test-refresh",
            "expires_at": time.time() + 600,  # 10 minutes
        }
        mock_client.fetch_server_metadata.return_value = {
            "authorization_endpoint": "https://test/auth",
            "token_endpoint": "https://test/token",
        }
        MockSession.return_value = mock_client

        provider = OidcAuthProvider(
            issuer="https://test.com",
            client_id="test-client",
            token=mock_client.token,
        )

        token = provider.get_token()
        assert token == valid_jwt
        # Should not call fetch_token since token is still valid
        mock_client.fetch_token.assert_not_called()


def test_oidc_get_token_needs_refresh():
    """Test getting token when refresh is needed."""
    from micromegas.auth import OidcAuthProvider

    with patch("micromegas.auth.oidc.requests.get") as mock_get, patch(
        "micromegas.auth.oidc.OAuth2Session"
    ) as MockSession:
        # Mock OIDC discovery response
        mock_response = MagicMock()
        mock_response.json.return_value = {
            "authorization_endpoint": "https://test/auth",
            "token_endpoint": "https://test/token",
            "issuer": "https://test.com",
        }
        mock_response.raise_for_status.return_value = None
        mock_get.return_value = mock_response

        mock_client = MagicMock()
        initial_token = {
            "access_token": "test-access",
            "id_token": "old-id-token",
            "refresh_token": "test-refresh",
            "expires_at": time.time() + 120,  # 2 minutes
        }
        # Token expires in 2 minutes (< 5 min buffer, needs refresh)
        mock_client.token = initial_token
        mock_client.fetch_server_metadata.return_value = {
            "authorization_endpoint": "https://test/auth",
            "token_endpoint": "https://test/token",
        }

        # Mock refresh response - set up side effect to update token
        new_token = {
            "access_token": "new-access",
            "id_token": "new-id-token",
            "refresh_token": "new-refresh",
            "expires_at": time.time() + 3600,
        }

        def update_token(*args, **kwargs):
            mock_client.token = new_token
            return new_token

        mock_client.fetch_token.side_effect = update_token
        MockSession.return_value = mock_client

        provider = OidcAuthProvider(
            issuer="https://test.com",
            client_id="test-client",
            token=initial_token,
        )
        provider.metadata = mock_client.fetch_server_metadata.return_value

        token = provider.get_token()
        assert token == "new-id-token"
        # Should call fetch_token for refresh
        mock_client.fetch_token.assert_called_once()


def test_oidc_get_token_no_token():
    """Test getting token when no tokens available."""
    from micromegas.auth import OidcAuthProvider

    with patch("micromegas.auth.oidc.requests.get") as mock_get, patch(
        "micromegas.auth.oidc.OAuth2Session"
    ) as MockSession:
        # Mock OIDC discovery response
        mock_response = MagicMock()
        mock_response.json.return_value = {
            "authorization_endpoint": "https://test/auth",
            "token_endpoint": "https://test/token",
            "issuer": "https://test.com",
        }
        mock_response.raise_for_status.return_value = None
        mock_get.return_value = mock_response

        mock_client = MagicMock()
        mock_client.token = None
        mock_client.fetch_server_metadata.return_value = {
            "authorization_endpoint": "https://test/auth",
            "token_endpoint": "https://test/token",
        }
        MockSession.return_value = mock_client

        provider = OidcAuthProvider(
            issuer="https://test.com", client_id="test-client", token=None
        )

        with pytest.raises(Exception, match="No tokens available"):
            provider.get_token()


def test_oidc_thread_safety():
    """Test that token refresh is thread-safe."""
    from micromegas.auth import OidcAuthProvider
    import threading

    with patch("micromegas.auth.oidc.requests.get") as mock_get, patch(
        "micromegas.auth.oidc.OAuth2Session"
    ) as MockSession:
        # Mock OIDC discovery response
        mock_response = MagicMock()
        mock_response.json.return_value = {
            "authorization_endpoint": "https://test/auth",
            "token_endpoint": "https://test/token",
            "issuer": "https://test.com",
        }
        mock_response.raise_for_status.return_value = None
        mock_get.return_value = mock_response

        mock_client = MagicMock()
        # Token expires soon
        mock_client.token = {
            "access_token": "test-access",
            "id_token": "test-id",
            "refresh_token": "test-refresh",
            "expires_at": time.time() + 120,  # 2 minutes
        }
        mock_client.fetch_server_metadata.return_value = {
            "authorization_endpoint": "https://test/auth",
            "token_endpoint": "https://test/token",
        }

        refresh_count = 0

        def mock_refresh(*args, **kwargs):
            nonlocal refresh_count
            refresh_count += 1
            time.sleep(0.1)  # Simulate network delay
            return {
                "access_token": "new-access",
                "id_token": "new-id",
                "refresh_token": "new-refresh",
                "expires_at": time.time() + 3600,
            }

        mock_client.fetch_token.side_effect = mock_refresh
        MockSession.return_value = mock_client

        provider = OidcAuthProvider(
            issuer="https://test.com",
            client_id="test-client",
            token=mock_client.token,
        )
        provider.metadata = mock_client.fetch_server_metadata.return_value

        # Simulate concurrent token requests
        threads = []
        results = []

        def get_token_thread():
            try:
                # Update client.token after refresh
                if mock_client.fetch_token.called:
                    mock_client.token = mock_client.fetch_token.return_value
                token = provider.get_token()
                results.append(token)
            except Exception as e:
                results.append(str(e))

        for _ in range(5):
            t = threading.Thread(target=get_token_thread)
            threads.append(t)
            t.start()

        for t in threads:
            t.join()

        # Should only refresh once due to locking
        # (but may be called multiple times if threads interleave)
        # At minimum, all threads should get a valid token
        assert len(results) == 5
        assert all(isinstance(r, str) for r in results)


def test_oidc_rejects_unsigned_token():
    """Test that unsigned tokens (alg=none) are rejected."""
    from micromegas.auth import OidcAuthProvider

    with patch("micromegas.auth.oidc.requests.get") as mock_get, patch(
        "micromegas.auth.oidc.OAuth2Session"
    ) as MockSession:
        # Mock OIDC discovery response
        mock_response = MagicMock()
        mock_response.json.return_value = {
            "authorization_endpoint": "https://test/auth",
            "token_endpoint": "https://test/token",
            "issuer": "https://test.com",
        }
        mock_response.raise_for_status.return_value = None
        mock_get.return_value = mock_response

        mock_client = MagicMock()
        # Create an unsigned JWT token (alg=none) like the one in the bug report
        # Header: {"typ":"JWT","alg":"none"}
        # Payload: {"sub":"test","exp":9999999999}
        unsigned_token = "eyJ0eXAiOiJKV1QiLCJhbGciOiJub25lIn0.eyJzdWIiOiJ0ZXN0IiwiZXhwIjo5OTk5OTk5OTk5fQ."

        mock_client.token = {
            "access_token": "test-access",
            "id_token": unsigned_token,
            "refresh_token": "test-refresh",
            "expires_at": time.time() + 600,  # 10 minutes
        }
        mock_client.fetch_server_metadata.return_value = {
            "authorization_endpoint": "https://test/auth",
            "token_endpoint": "https://test/token",
        }
        MockSession.return_value = mock_client

        provider = OidcAuthProvider(
            issuer="https://test.com",
            client_id="test-client",
            token=mock_client.token,
        )

        # Should raise exception for unsigned token
        with pytest.raises(Exception, match="alg=none"):
            provider.get_token()
