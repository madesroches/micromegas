package flightsql

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/grafana/grafana-plugin-sdk-go/backend"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// TestDiscoverTokenEndpoint_Success tests successful OIDC discovery
func TestDiscoverTokenEndpoint_Success(t *testing.T) {
	// Create mock OIDC discovery server
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		assert.Equal(t, "/.well-known/openid-configuration", r.URL.Path)

		discovery := map[string]interface{}{
			"issuer":         "https://example.com",
			"token_endpoint": "https://example.com/oauth/token",
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	// Test discovery
	tokenEndpoint, err := discoverTokenEndpoint(discoveryServer.URL)

	require.NoError(t, err)
	assert.Equal(t, "https://example.com/oauth/token", tokenEndpoint)
}

// TestDiscoverTokenEndpoint_WithTrailingSlash tests discovery URL with trailing slash
func TestDiscoverTokenEndpoint_WithTrailingSlash(t *testing.T) {
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		assert.Equal(t, "/.well-known/openid-configuration", r.URL.Path)

		discovery := map[string]interface{}{
			"token_endpoint": "https://example.com/oauth/token",
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	// Test with trailing slash - should be removed
	tokenEndpoint, err := discoverTokenEndpoint(discoveryServer.URL + "/")

	require.NoError(t, err)
	assert.Equal(t, "https://example.com/oauth/token", tokenEndpoint)
}

// TestDiscoverTokenEndpoint_NotFound tests 404 error
func TestDiscoverTokenEndpoint_NotFound(t *testing.T) {
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer discoveryServer.Close()

	_, err := discoverTokenEndpoint(discoveryServer.URL)

	require.Error(t, err)
	assert.Contains(t, err.Error(), "discovery failed with status: 404")
}

// TestDiscoverTokenEndpoint_InvalidJSON tests malformed JSON response
func TestDiscoverTokenEndpoint_InvalidJSON(t *testing.T) {
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte("invalid json {"))
	}))
	defer discoveryServer.Close()

	_, err := discoverTokenEndpoint(discoveryServer.URL)

	require.Error(t, err)
	assert.Contains(t, err.Error(), "failed to parse discovery response")
}

// TestDiscoverTokenEndpoint_MissingTokenEndpoint tests missing token_endpoint field
func TestDiscoverTokenEndpoint_MissingTokenEndpoint(t *testing.T) {
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		discovery := map[string]interface{}{
			"issuer": "https://example.com",
			// token_endpoint is missing
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	_, err := discoverTokenEndpoint(discoveryServer.URL)

	require.Error(t, err)
	assert.Contains(t, err.Error(), "token_endpoint not found in discovery document")
}

// TestDiscoverTokenEndpoint_Timeout tests HTTP timeout
func TestDiscoverTokenEndpoint_Timeout(t *testing.T) {
	// Create server that hangs for longer than timeout
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(15 * time.Second) // Longer than 10 second timeout
	}))
	defer discoveryServer.Close()

	_, err := discoverTokenEndpoint(discoveryServer.URL)

	require.Error(t, err)
	assert.Contains(t, err.Error(), "discovery request failed")
	// Should fail due to timeout, not hang indefinitely
}

// TestDiscoverTokenEndpoint_NetworkError tests network failure
func TestDiscoverTokenEndpoint_NetworkError(t *testing.T) {
	// Use invalid URL that will fail to connect
	_, err := discoverTokenEndpoint("http://invalid-host-that-does-not-exist.example.com")

	require.Error(t, err)
	assert.Contains(t, err.Error(), "discovery request failed")
}

// TestNewOAuthTokenManager_Success tests successful token manager creation
func TestNewOAuthTokenManager_Success(t *testing.T) {
	// Mock OIDC discovery server
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		discovery := map[string]interface{}{
			"token_endpoint": "https://example.com/oauth/token",
		}
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	mgr, err := NewOAuthTokenManager(
		discoveryServer.URL,
		"test-client-id",
		"test-client-secret",
		"",
	)

	require.NoError(t, err)
	require.NotNil(t, mgr)
	assert.NotNil(t, mgr.tokenSource)
	assert.NotNil(t, mgr.config)
	assert.Equal(t, "test-client-id", mgr.config.ClientID)
	assert.Equal(t, "test-client-secret", mgr.config.ClientSecret)
	assert.Equal(t, "https://example.com/oauth/token", mgr.config.TokenURL)
}

// TestNewOAuthTokenManager_WithAudience tests token manager with audience parameter
func TestNewOAuthTokenManager_WithAudience(t *testing.T) {
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		discovery := map[string]interface{}{
			"token_endpoint": "https://example.com/oauth/token",
		}
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	mgr, err := NewOAuthTokenManager(
		discoveryServer.URL,
		"test-client-id",
		"test-client-secret",
		"https://api.example.com",
	)

	require.NoError(t, err)
	require.NotNil(t, mgr)
	assert.Contains(t, mgr.config.EndpointParams, "audience")
	assert.Equal(t, []string{"https://api.example.com"}, mgr.config.EndpointParams["audience"])
}

// TestNewOAuthTokenManager_DiscoveryFailure tests token manager creation with discovery failure
func TestNewOAuthTokenManager_DiscoveryFailure(t *testing.T) {
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer discoveryServer.Close()

	_, err := NewOAuthTokenManager(
		discoveryServer.URL,
		"test-client-id",
		"test-client-secret",
		"",
	)

	require.Error(t, err)
	assert.Contains(t, err.Error(), "OIDC discovery failed")
}

// TestGetToken_Success tests successful token retrieval
func TestGetToken_Success(t *testing.T) {
	// Mock token server
	tokenServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		assert.Equal(t, "/oauth/token", r.URL.Path)
		assert.Equal(t, "POST", r.Method)

		// Verify client credentials are sent
		err := r.ParseForm()
		require.NoError(t, err)
		assert.Equal(t, "client_credentials", r.FormValue("grant_type"))

		// Return valid token
		token := map[string]interface{}{
			"access_token": "test-access-token-12345",
			"token_type":   "Bearer",
			"expires_in":   3600,
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(token)
	}))
	defer tokenServer.Close()

	// Mock discovery server
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		discovery := map[string]interface{}{
			"token_endpoint": tokenServer.URL + "/oauth/token",
		}
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	mgr, err := NewOAuthTokenManager(
		discoveryServer.URL,
		"test-client-id",
		"test-client-secret",
		"",
	)
	require.NoError(t, err)

	// Get token
	ctx := context.Background()
	token, err := mgr.GetToken(ctx)

	require.NoError(t, err)
	assert.Equal(t, "test-access-token-12345", token)
}

// TestGetToken_WithAudience tests token retrieval with audience parameter
func TestGetToken_WithAudience(t *testing.T) {
	audienceReceived := ""

	// Mock token server
	tokenServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		err := r.ParseForm()
		require.NoError(t, err)
		audienceReceived = r.FormValue("audience")

		w.Header().Set("Content-Type", "application/json")
		token := map[string]interface{}{
			"access_token": "test-token",
			"token_type":   "Bearer",
			"expires_in":   3600,
		}
		json.NewEncoder(w).Encode(token)
	}))
	defer tokenServer.Close()

	// Mock discovery server
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		discovery := map[string]interface{}{
			"token_endpoint": tokenServer.URL + "/oauth/token",
		}
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	mgr, err := NewOAuthTokenManager(
		discoveryServer.URL,
		"test-client-id",
		"test-client-secret",
		"https://api.example.com",
	)
	require.NoError(t, err)

	ctx := context.Background()
	_, err = mgr.GetToken(ctx)

	require.NoError(t, err)
	assert.Equal(t, "https://api.example.com", audienceReceived)
}

// TestGetToken_TokenCaching tests that tokens are cached
func TestGetToken_TokenCaching(t *testing.T) {
	requestCount := 0

	// Mock token server
	tokenServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requestCount++
		w.Header().Set("Content-Type", "application/json")
		token := map[string]interface{}{
			"access_token": fmt.Sprintf("token-%d", requestCount),
			"token_type":   "Bearer",
			"expires_in":   3600, // 1 hour - should be cached
		}
		json.NewEncoder(w).Encode(token)
	}))
	defer tokenServer.Close()

	// Mock discovery server
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		discovery := map[string]interface{}{
			"token_endpoint": tokenServer.URL + "/oauth/token",
		}
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	mgr, err := NewOAuthTokenManager(
		discoveryServer.URL,
		"test-client-id",
		"test-client-secret",
		"",
	)
	require.NoError(t, err)

	ctx := context.Background()

	// First token fetch - should hit server
	token1, err := mgr.GetToken(ctx)
	require.NoError(t, err)
	assert.Equal(t, "token-1", token1)
	assert.Equal(t, 1, requestCount)

	// Second fetch - should use cached token (no new request)
	token2, err := mgr.GetToken(ctx)
	require.NoError(t, err)
	assert.Equal(t, "token-1", token2) // Same token
	assert.Equal(t, 1, requestCount)   // No additional request

	// Third fetch - should still use cache
	token3, err := mgr.GetToken(ctx)
	require.NoError(t, err)
	assert.Equal(t, "token-1", token3)
	assert.Equal(t, 1, requestCount)
}

// TestGetToken_InvalidCredentials tests token fetch with invalid credentials
func TestGetToken_InvalidCredentials(t *testing.T) {
	// Mock token server that rejects credentials
	tokenServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
		json.NewEncoder(w).Encode(map[string]interface{}{
			"error":             "invalid_client",
			"error_description": "Client authentication failed",
		})
	}))
	defer tokenServer.Close()

	// Mock discovery server
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		discovery := map[string]interface{}{
			"token_endpoint": tokenServer.URL + "/oauth/token",
		}
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	mgr, err := NewOAuthTokenManager(
		discoveryServer.URL,
		"invalid-client-id",
		"invalid-secret",
		"",
	)
	require.NoError(t, err)

	ctx := context.Background()
	_, err = mgr.GetToken(ctx)

	require.Error(t, err)
	assert.Contains(t, err.Error(), "failed to get OAuth token")
}

// TestGetToken_ServerError tests token fetch with server error
func TestGetToken_ServerError(t *testing.T) {
	// Mock token server that returns error
	tokenServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer tokenServer.Close()

	// Mock discovery server
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		discovery := map[string]interface{}{
			"token_endpoint": tokenServer.URL + "/oauth/token",
		}
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	mgr, err := NewOAuthTokenManager(
		discoveryServer.URL,
		"test-client-id",
		"test-client-secret",
		"",
	)
	require.NoError(t, err)

	ctx := context.Background()
	_, err = mgr.GetToken(ctx)

	require.Error(t, err)
	assert.Contains(t, err.Error(), "failed to get OAuth token")
}

// TestConfigValidation_OAuth tests config validation with OAuth credentials
func TestConfigValidation_OAuth(t *testing.T) {
	tests := []struct {
		name      string
		config    config
		wantError bool
	}{
		{
			name: "valid OAuth config",
			config: config{
				Addr:              "localhost:50051",
				Secure:            true,
				OAuthIssuer:       "https://example.com",
				OAuthClientId:     "client-id",
				OAuthClientSecret: "client-secret",
			},
			wantError: false,
		},
		{
			name: "OAuth without issuer",
			config: config{
				Addr:              "localhost:50051",
				Secure:            true,
				OAuthClientId:     "client-id",
				OAuthClientSecret: "client-secret",
			},
			wantError: true,
		},
		{
			name: "OAuth without client ID",
			config: config{
				Addr:              "localhost:50051",
				Secure:            true,
				OAuthIssuer:       "https://example.com",
				OAuthClientSecret: "client-secret",
			},
			wantError: true,
		},
		{
			name: "OAuth without client secret",
			config: config{
				Addr:          "localhost:50051",
				Secure:        true,
				OAuthIssuer:   "https://example.com",
				OAuthClientId: "client-id",
			},
			wantError: true,
		},
		{
			name: "insecure without OAuth is valid",
			config: config{
				Addr:   "localhost:50051",
				Secure: false,
			},
			wantError: false,
		},
		{
			name: "secure with token instead of OAuth",
			config: config{
				Addr:   "localhost:50051",
				Secure: true,
				Token:  "api-token",
			},
			wantError: false,
		},
		{
			name: "secure with username/password instead of OAuth",
			config: config{
				Addr:     "localhost:50051",
				Secure:   true,
				Username: "user",
				Password: "pass",
			},
			wantError: false,
		},
		{
			name: "invalid address format",
			config: config{
				Addr:              "localhost",
				Secure:            true,
				OAuthIssuer:       "https://example.com",
				OAuthClientId:     "client-id",
				OAuthClientSecret: "client-secret",
			},
			wantError: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := tt.config.validate()
			if tt.wantError {
				assert.Error(t, err)
			} else {
				assert.NoError(t, err)
			}
		})
	}
}

// TestNewDatasource_OAuth tests datasource creation with OAuth
func TestNewDatasource_OAuth(t *testing.T) {
	// This is a unit test that doesn't require a real FlightSQL server
	// We test that OAuth configuration is properly loaded from settings

	// Mock OIDC discovery server
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		discovery := map[string]interface{}{
			"token_endpoint": "https://example.com/oauth/token",
		}
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	// Note: This test will fail at FlightSQL client creation since we don't have a real server
	// But it validates OAuth config loading
	cfg := config{
		Addr:              "localhost:50051",
		Secure:            false,
		OAuthIssuer:       discoveryServer.URL,
		OAuthClientId:     "test-client-id",
		OAuthClientSecret: "test-client-secret",
		OAuthAudience:     "https://api.example.com",
	}

	cfgJSON, err := json.Marshal(cfg)
	require.NoError(t, err)

	settings := backend.DataSourceInstanceSettings{
		JSONData: cfgJSON,
		DecryptedSecureJSONData: map[string]string{
			"oauthClientSecret": "test-client-secret",
		},
	}

	// Attempt to create datasource
	// This will fail at FlightSQL client creation, but we can verify config parsing
	_, err = NewDatasource(context.Background(), settings)

	// We expect an error because there's no FlightSQL server
	// But it should NOT be a config error - it should fail later
	require.Error(t, err)
	// Error should be about connection, not config
	assert.NotContains(t, strings.ToLower(err.Error()), "config validation")
}

// TestNewDatasource_BackwardCompatibility tests that existing auth methods still work
func TestNewDatasource_BackwardCompatibility(t *testing.T) {
	tests := []struct {
		name        string
		config      config
		expectError bool // whether we expect an error due to connection failure
	}{
		{
			name: "token auth (API key)",
			config: config{
				Addr:   "localhost:50051",
				Secure: false,
				Token:  "api-token-12345",
			},
			expectError: false, // token auth doesn't attempt connection during creation
		},
		{
			name: "username/password auth",
			config: config{
				Addr:     "localhost:50051",
				Secure:   false,
				Username: "testuser",
				Password: "testpass",
			},
			expectError: true, // will fail to connect
		},
		{
			name: "no auth",
			config: config{
				Addr:   "localhost:50051",
				Secure: false,
			},
			expectError: false, // no auth required, no connection attempted at creation
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfgJSON, err := json.Marshal(tt.config)
			require.NoError(t, err)

			settings := backend.DataSourceInstanceSettings{
				JSONData: cfgJSON,
				DecryptedSecureJSONData: map[string]string{
					"token":    tt.config.Token,
					"password": tt.config.Password,
				},
			}

			// Attempt to create datasource
			_, err = NewDatasource(context.Background(), settings)

			if tt.expectError {
				require.Error(t, err)
				// Should not be a validation error
				assert.NotContains(t, strings.ToLower(err.Error()), "config validation")
			} else {
				// No auth might succeed in creating datasource (no connection attempted)
				// Just verify config validates properly
				assert.NoError(t, tt.config.validate())
			}
		})
	}
}

// TestOAuthTokenManager_ConcurrentAccess tests thread safety of token manager
func TestOAuthTokenManager_ConcurrentAccess(t *testing.T) {
	requestCount := 0
	var requestCountMu sync.Mutex

	// Mock token server
	tokenServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requestCountMu.Lock()
		requestCount++
		count := requestCount
		requestCountMu.Unlock()

		// Ensure proper content type for oauth2 library
		w.Header().Set("Content-Type", "application/json")
		token := map[string]interface{}{
			"access_token": fmt.Sprintf("token-%d", count),
			"token_type":   "Bearer",
			"expires_in":   3600,
		}
		json.NewEncoder(w).Encode(token)
	}))
	defer tokenServer.Close()

	// Mock discovery server
	discoveryServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		discovery := map[string]interface{}{
			"token_endpoint": tokenServer.URL + "/oauth/token",
		}
		json.NewEncoder(w).Encode(discovery)
	}))
	defer discoveryServer.Close()

	mgr, err := NewOAuthTokenManager(
		discoveryServer.URL,
		"test-client-id",
		"test-client-secret",
		"",
	)
	require.NoError(t, err)

	// Simulate concurrent access
	const numGoroutines = 10
	var wg sync.WaitGroup
	tokens := make([]string, numGoroutines)
	errors := make([]error, numGoroutines)

	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			token, err := mgr.GetToken(context.Background())
			tokens[idx] = token
			errors[idx] = err
		}(i)
	}

	wg.Wait()

	// Check all requests succeeded
	for i, err := range errors {
		require.NoError(t, err, "goroutine %d failed", i)
	}

	// All goroutines should get the same cached token
	firstToken := tokens[0]
	for i, token := range tokens {
		assert.Equal(t, firstToken, token, "goroutine %d got different token", i)
	}

	// Should only have made one request to token server (or very few due to race)
	requestCountMu.Lock()
	defer requestCountMu.Unlock()
	// Due to concurrency, we might get a few requests before caching kicks in
	// but it should be much less than the number of goroutines
	assert.LessOrEqual(t, requestCount, 3, "too many token requests, caching not working")
}
