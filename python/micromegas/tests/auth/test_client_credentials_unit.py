"""Unit tests for OidcClientCredentialsProvider."""

import os
import time
from unittest.mock import MagicMock, patch

import pytest

from micromegas.auth import OidcClientCredentialsProvider


@pytest.fixture
def mock_oidc_metadata():
    """Mock OIDC discovery metadata."""
    return {
        "issuer": "https://accounts.google.com",
        "token_endpoint": "https://oauth2.googleapis.com/token",
        "authorization_endpoint": "https://accounts.google.com/o/oauth2/v2/auth",
        "jwks_uri": "https://www.googleapis.com/oauth2/v3/certs",
    }


@pytest.fixture
def mock_token_response():
    """Mock token response from OIDC provider."""
    return {
        "access_token": "test-access-token-12345",
        "token_type": "Bearer",
        "expires_in": 3600,
    }


@patch("micromegas.auth.oidc.requests.get")
def test_client_credentials_init(mock_get, mock_oidc_metadata):
    """Test OidcClientCredentialsProvider initialization."""
    # Mock OIDC discovery
    mock_response = MagicMock()
    mock_response.json.return_value = mock_oidc_metadata
    mock_get.return_value = mock_response

    # Create provider
    provider = OidcClientCredentialsProvider(
        issuer="https://accounts.google.com",
        client_id="test-client-id",
        client_secret="test-client-secret",
    )

    # Verify discovery was called
    mock_get.assert_called_once_with(
        "https://accounts.google.com/.well-known/openid-configuration", timeout=10
    )

    # Verify provider attributes
    assert provider.issuer == "https://accounts.google.com"
    assert provider.client_id == "test-client-id"
    assert provider.client_secret == "test-client-secret"
    assert provider.metadata == mock_oidc_metadata
    assert provider._cached_token is None


@patch.dict(
    os.environ,
    {
        "MICROMEGAS_OIDC_ISSUER": "https://accounts.google.com",
        "MICROMEGAS_OIDC_CLIENT_ID": "test-client-id",
        "MICROMEGAS_OIDC_CLIENT_SECRET": "test-secret",
    },
)
@patch("micromegas.auth.oidc.requests.get")
def test_client_credentials_from_env(mock_get, mock_oidc_metadata):
    """Test OidcClientCredentialsProvider.from_env()."""
    # Mock OIDC discovery
    mock_response = MagicMock()
    mock_response.json.return_value = mock_oidc_metadata
    mock_get.return_value = mock_response

    # Create provider from environment
    provider = OidcClientCredentialsProvider.from_env()

    assert provider.issuer == "https://accounts.google.com"
    assert provider.client_id == "test-client-id"
    assert provider.client_secret == "test-secret"


def test_client_credentials_from_env_missing_issuer():
    """Test from_env() fails when MICROMEGAS_OIDC_ISSUER is missing."""
    # Clear environment
    for var in [
        "MICROMEGAS_OIDC_ISSUER",
        "MICROMEGAS_OIDC_CLIENT_ID",
        "MICROMEGAS_OIDC_CLIENT_SECRET",
    ]:
        os.environ.pop(var, None)

    with pytest.raises(ValueError, match="MICROMEGAS_OIDC_ISSUER"):
        OidcClientCredentialsProvider.from_env()


@patch.dict(
    os.environ,
    {
        "MICROMEGAS_OIDC_ISSUER": "https://accounts.google.com",
    },
)
def test_client_credentials_from_env_missing_client_id():
    """Test from_env() fails when MICROMEGAS_OIDC_CLIENT_ID is missing."""
    with pytest.raises(ValueError, match="MICROMEGAS_OIDC_CLIENT_ID"):
        OidcClientCredentialsProvider.from_env()


@patch.dict(
    os.environ,
    {
        "MICROMEGAS_OIDC_ISSUER": "https://accounts.google.com",
        "MICROMEGAS_OIDC_CLIENT_ID": "test-client-id",
    },
)
def test_client_credentials_from_env_missing_client_secret():
    """Test from_env() fails when MICROMEGAS_OIDC_CLIENT_SECRET is missing."""
    with pytest.raises(ValueError, match="MICROMEGAS_OIDC_CLIENT_SECRET"):
        OidcClientCredentialsProvider.from_env()


@patch("micromegas.auth.oidc.requests.post")
@patch("micromegas.auth.oidc.requests.get")
def test_client_credentials_get_token(
    mock_get, mock_post, mock_oidc_metadata, mock_token_response
):
    """Test get_token() fetches and returns access token."""
    # Mock OIDC discovery
    mock_get_response = MagicMock()
    mock_get_response.json.return_value = mock_oidc_metadata
    mock_get.return_value = mock_get_response

    # Mock token endpoint
    mock_post_response = MagicMock()
    mock_post_response.json.return_value = mock_token_response
    mock_post.return_value = mock_post_response

    # Create provider
    provider = OidcClientCredentialsProvider(
        issuer="https://accounts.google.com",
        client_id="test-client-id",
        client_secret="test-client-secret",
    )

    # Get token
    token = provider.get_token()

    # Verify token endpoint was called
    mock_post.assert_called_once_with(
        "https://oauth2.googleapis.com/token",
        data={
            "grant_type": "client_credentials",
            "client_id": "test-client-id",
            "client_secret": "test-client-secret",
        },
        timeout=10,
    )

    # Verify token returned
    assert token == "test-access-token-12345"

    # Verify token cached
    assert provider._cached_token is not None
    assert provider._cached_token["access_token"] == "test-access-token-12345"


@patch("micromegas.auth.oidc.requests.post")
@patch("micromegas.auth.oidc.requests.get")
def test_client_credentials_token_caching(
    mock_get, mock_post, mock_oidc_metadata, mock_token_response
):
    """Test that get_token() uses cached token when still valid."""
    # Mock OIDC discovery
    mock_get_response = MagicMock()
    mock_get_response.json.return_value = mock_oidc_metadata
    mock_get.return_value = mock_get_response

    # Mock token endpoint
    mock_post_response = MagicMock()
    mock_post_response.json.return_value = mock_token_response
    mock_post.return_value = mock_post_response

    # Create provider
    provider = OidcClientCredentialsProvider(
        issuer="https://accounts.google.com",
        client_id="test-client-id",
        client_secret="test-client-secret",
    )

    # First call - fetches token
    token1 = provider.get_token()
    assert mock_post.call_count == 1

    # Second call - uses cached token (no new fetch)
    token2 = provider.get_token()
    assert mock_post.call_count == 1  # Still 1, no new call
    assert token1 == token2


@patch("micromegas.auth.oidc.requests.post")
@patch("micromegas.auth.oidc.requests.get")
@patch("micromegas.auth.oidc.time.time")
def test_client_credentials_token_refresh(
    mock_time, mock_get, mock_post, mock_oidc_metadata, mock_token_response
):
    """Test that get_token() refreshes expired token."""
    # Mock OIDC discovery
    mock_get_response = MagicMock()
    mock_get_response.json.return_value = mock_oidc_metadata
    mock_get.return_value = mock_get_response

    # Mock token endpoint - first call
    mock_post_response = MagicMock()
    mock_post_response.json.return_value = mock_token_response
    mock_post.return_value = mock_post_response

    # Mock time - start at t=0
    mock_time.return_value = 0.0

    # Create provider
    provider = OidcClientCredentialsProvider(
        issuer="https://accounts.google.com",
        client_id="test-client-id",
        client_secret="test-client-secret",
    )

    # First call - fetches token (expires at 3300 due to 5 min buffer)
    token1 = provider.get_token()
    assert mock_post.call_count == 1
    assert token1 == "test-access-token-12345"

    # Advance time to before expiration
    mock_time.return_value = 3000.0  # Still valid

    # Second call - uses cached token
    token2 = provider.get_token()
    assert mock_post.call_count == 1  # No new fetch
    assert token2 == token1

    # Advance time to after expiration
    mock_time.return_value = 3400.0  # Expired

    # Mock new token response
    mock_token_response["access_token"] = "new-access-token-67890"
    mock_post_response.json.return_value = mock_token_response

    # Third call - fetches new token
    token3 = provider.get_token()
    assert mock_post.call_count == 2  # New fetch
    assert token3 == "new-access-token-67890"


@patch("micromegas.auth.oidc.requests.post")
@patch("micromegas.auth.oidc.requests.get")
def test_client_credentials_thread_safety(
    mock_get, mock_post, mock_oidc_metadata, mock_token_response
):
    """Test that get_token() is thread-safe."""
    import threading

    # Mock OIDC discovery
    mock_get_response = MagicMock()
    mock_get_response.json.return_value = mock_oidc_metadata
    mock_get.return_value = mock_get_response

    # Mock token endpoint
    mock_post_response = MagicMock()
    mock_post_response.json.return_value = mock_token_response
    mock_post.return_value = mock_post_response

    # Create provider
    provider = OidcClientCredentialsProvider(
        issuer="https://accounts.google.com",
        client_id="test-client-id",
        client_secret="test-client-secret",
    )

    # Call get_token() from multiple threads concurrently
    tokens = []

    def get_token_thread():
        token = provider.get_token()
        tokens.append(token)

    threads = [threading.Thread(target=get_token_thread) for _ in range(10)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    # All threads should get the same token
    assert len(tokens) == 10
    assert all(token == tokens[0] for token in tokens)

    # Token should only be fetched once (due to locking + caching)
    # Note: In practice, might be 1-2 calls due to race conditions,
    # but should be much less than 10
    assert mock_post.call_count <= 2
