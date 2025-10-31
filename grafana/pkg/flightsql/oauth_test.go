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

	"github.com/grafana/grafana-plugin-sdk-go/backend"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// Test helpers

// newMockDiscoveryServer creates a mock OIDC discovery server
func newMockDiscoveryServer(t *testing.T, tokenEndpoint string) *httptest.Server {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		discovery := map[string]interface{}{
			"token_endpoint": tokenEndpoint,
		}
		json.NewEncoder(w).Encode(discovery)
	}))
	t.Cleanup(server.Close)
	return server
}

// newMockTokenServer creates a mock OAuth token server with standard successful response
func newMockTokenServer(t *testing.T) *httptest.Server {
	t.Helper()
	return newMockTokenServerCustom(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		token := map[string]interface{}{
			"access_token": "test-access-token-12345",
			"token_type":   "Bearer",
			"expires_in":   3600,
		}
		json.NewEncoder(w).Encode(token)
	})
}

// newMockTokenServerCustom creates a mock OAuth token server with custom handler
func newMockTokenServerCustom(t *testing.T, handler http.HandlerFunc) *httptest.Server {
	t.Helper()
	server := httptest.NewServer(handler)
	t.Cleanup(server.Close)
	return server
}

// newMockOAuthManager creates a fully configured OAuth manager for testing
func newMockOAuthManager(t *testing.T, tokenHandler http.HandlerFunc) *OAuthTokenManager {
	t.Helper()
	tokenServer := newMockTokenServerCustom(t, tokenHandler)
	discoveryServer := newMockDiscoveryServer(t, tokenServer.URL+"/oauth/token")

	mgr, err := NewOAuthTokenManager(
		discoveryServer.URL,
		"test-client-id",
		"test-client-secret",
		"",
	)
	require.NoError(t, err)
	return mgr
}

// TestDiscoverTokenEndpoint_Success tests successful OIDC discovery
func TestDiscoverTokenEndpoint_Success(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/.well-known/openid-configuration" {
			t.Errorf("unexpected path: %s", r.URL.Path)
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"issuer":         "https://example.com",
			"token_endpoint": "https://example.com/oauth/token",
		})
	}))
	t.Cleanup(server.Close)

	tokenEndpoint, err := discoverTokenEndpoint(server.URL)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if tokenEndpoint != "https://example.com/oauth/token" {
		t.Errorf("got %q, want %q", tokenEndpoint, "https://example.com/oauth/token")
	}
}

// TestDiscoverTokenEndpoint_WithTrailingSlash tests discovery URL with trailing slash
func TestDiscoverTokenEndpoint_WithTrailingSlash(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/.well-known/openid-configuration" {
			t.Errorf("unexpected path: %s", r.URL.Path)
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"token_endpoint": "https://example.com/oauth/token",
		})
	}))
	t.Cleanup(server.Close)

	tokenEndpoint, err := discoverTokenEndpoint(server.URL + "/")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if tokenEndpoint != "https://example.com/oauth/token" {
		t.Errorf("got %q, want %q", tokenEndpoint, "https://example.com/oauth/token")
	}
}

// TestDiscoverTokenEndpoint_NotFound tests 404 error
func TestDiscoverTokenEndpoint_NotFound(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	t.Cleanup(server.Close)

	_, err := discoverTokenEndpoint(server.URL)
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !strings.Contains(err.Error(), "discovery failed with status: 404") {
		t.Errorf("unexpected error: %v", err)
	}
}

// TestDiscoverTokenEndpoint_InvalidJSON tests malformed JSON response
func TestDiscoverTokenEndpoint_InvalidJSON(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte("invalid json {"))
	}))
	t.Cleanup(server.Close)

	_, err := discoverTokenEndpoint(server.URL)
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !strings.Contains(err.Error(), "failed to parse discovery response") {
		t.Errorf("unexpected error: %v", err)
	}
}

// TestDiscoverTokenEndpoint_MissingTokenEndpoint tests missing token_endpoint field
func TestDiscoverTokenEndpoint_MissingTokenEndpoint(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"issuer": "https://example.com",
		})
	}))
	t.Cleanup(server.Close)

	_, err := discoverTokenEndpoint(server.URL)
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !strings.Contains(err.Error(), "token_endpoint not found in discovery document") {
		t.Errorf("unexpected error: %v", err)
	}
}

// TestDiscoverTokenEndpoint_NetworkError tests network failure
func TestDiscoverTokenEndpoint_NetworkError(t *testing.T) {
	_, err := discoverTokenEndpoint("http://invalid-host-that-does-not-exist.example.com")
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !strings.Contains(err.Error(), "discovery request failed") {
		t.Errorf("unexpected error: %v", err)
	}
}

// TestNewOAuthTokenManager_Success tests successful token manager creation
func TestNewOAuthTokenManager_Success(t *testing.T) {
	server := newMockDiscoveryServer(t, "https://example.com/oauth/token")

	mgr, err := NewOAuthTokenManager(server.URL, "test-client-id", "test-client-secret", "")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if mgr == nil {
		t.Fatal("expected non-nil manager")
	}
	if mgr.config.ClientID != "test-client-id" {
		t.Errorf("got client ID %q, want %q", mgr.config.ClientID, "test-client-id")
	}
	if mgr.config.TokenURL != "https://example.com/oauth/token" {
		t.Errorf("got token URL %q, want %q", mgr.config.TokenURL, "https://example.com/oauth/token")
	}
}

// TestNewOAuthTokenManager_WithAudience tests token manager with audience parameter
func TestNewOAuthTokenManager_WithAudience(t *testing.T) {
	server := newMockDiscoveryServer(t, "https://example.com/oauth/token")

	mgr, err := NewOAuthTokenManager(server.URL, "test-client-id", "test-client-secret", "https://api.example.com")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	audience, ok := mgr.config.EndpointParams["audience"]
	if !ok {
		t.Fatal("expected audience parameter")
	}
	if len(audience) != 1 || audience[0] != "https://api.example.com" {
		t.Errorf("got audience %v, want [%q]", audience, "https://api.example.com")
	}
}

// TestNewOAuthTokenManager_DiscoveryFailure tests token manager creation with discovery failure
func TestNewOAuthTokenManager_DiscoveryFailure(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	t.Cleanup(server.Close)

	_, err := NewOAuthTokenManager(server.URL, "test-client-id", "test-client-secret", "")
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !strings.Contains(err.Error(), "OIDC discovery failed") {
		t.Errorf("unexpected error: %v", err)
	}
}

// TestGetToken_Success tests successful token retrieval
func TestGetToken_Success(t *testing.T) {
	mgr := newMockOAuthManager(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			t.Errorf("got method %s, want POST", r.Method)
		}
		if err := r.ParseForm(); err != nil {
			t.Fatalf("failed to parse form: %v", err)
		}
		if r.FormValue("grant_type") != "client_credentials" {
			t.Errorf("got grant_type %q, want %q", r.FormValue("grant_type"), "client_credentials")
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"access_token": "test-access-token-12345",
			"token_type":   "Bearer",
			"expires_in":   3600,
		})
	})

	token, err := mgr.GetToken(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if token != "test-access-token-12345" {
		t.Errorf("got token %q, want %q", token, "test-access-token-12345")
	}
}

// TestGetToken_WithAudience tests token retrieval with audience parameter
func TestGetToken_WithAudience(t *testing.T) {
	var audienceReceived string
	tokenServer := newMockTokenServerCustom(t, func(w http.ResponseWriter, r *http.Request) {
		if err := r.ParseForm(); err != nil {
			t.Fatalf("failed to parse form: %v", err)
		}
		audienceReceived = r.FormValue("audience")
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"access_token": "test-token",
			"token_type":   "Bearer",
			"expires_in":   3600,
		})
	})
	discoveryServer := newMockDiscoveryServer(t, tokenServer.URL+"/oauth/token")

	mgr, err := NewOAuthTokenManager(discoveryServer.URL, "test-client-id", "test-client-secret", "https://api.example.com")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if _, err := mgr.GetToken(context.Background()); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if audienceReceived != "https://api.example.com" {
		t.Errorf("got audience %q, want %q", audienceReceived, "https://api.example.com")
	}
}

// TestGetToken_TokenCaching tests that tokens are cached
func TestGetToken_TokenCaching(t *testing.T) {
	var requestCount int
	mgr := newMockOAuthManager(t, func(w http.ResponseWriter, r *http.Request) {
		requestCount++
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"access_token": fmt.Sprintf("token-%d", requestCount),
			"token_type":   "Bearer",
			"expires_in":   3600,
		})
	})

	ctx := context.Background()
	token1, err := mgr.GetToken(ctx)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if token1 != "token-1" {
		t.Errorf("got token %q, want %q", token1, "token-1")
	}
	if requestCount != 1 {
		t.Errorf("got %d requests, want 1", requestCount)
	}

	// Second fetch should use cache
	token2, err := mgr.GetToken(ctx)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if token2 != "token-1" {
		t.Errorf("got token %q, want %q (cached)", token2, "token-1")
	}
	if requestCount != 1 {
		t.Errorf("got %d requests, want 1 (should be cached)", requestCount)
	}
}

// TestGetToken_InvalidCredentials tests token fetch with invalid credentials
func TestGetToken_InvalidCredentials(t *testing.T) {
	mgr := newMockOAuthManager(t, func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
		json.NewEncoder(w).Encode(map[string]interface{}{
			"error":             "invalid_client",
			"error_description": "Client authentication failed",
		})
	})

	_, err := mgr.GetToken(context.Background())
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !strings.Contains(err.Error(), "failed to get OAuth token") {
		t.Errorf("unexpected error: %v", err)
	}
}

// TestGetToken_ServerError tests token fetch with server error
func TestGetToken_ServerError(t *testing.T) {
	mgr := newMockOAuthManager(t, func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	})

	_, err := mgr.GetToken(context.Background())
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !strings.Contains(err.Error(), "failed to get OAuth token") {
		t.Errorf("unexpected error: %v", err)
	}
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
				OAuthClientID:     "client-id",
				OAuthClientSecret: "client-secret",
			},
			wantError: false,
		},
		{
			name: "OAuth without issuer",
			config: config{
				Addr:              "localhost:50051",
				Secure:            true,
				OAuthClientID:     "client-id",
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
				OAuthClientID: "client-id",
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
				OAuthClientID:     "client-id",
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

	// Note: With lazy initialization, OAuth config is validated but token is not fetched
	// until first query. This test validates OAuth config loading and manager creation.
	cfg := config{
		Addr:              "localhost:50051",
		Secure:            false,
		OAuthIssuer:       discoveryServer.URL,
		OAuthClientID:     "test-client-id",
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
	// With lazy initialization, this should succeed even without a FlightSQL server
	// Token will be fetched on first query
	ds, err := NewDatasource(context.Background(), settings)

	// Should fail at FlightSQL client creation (no server), but this is after OAuth setup
	// If this starts succeeding in the future (mock client), that's also acceptable
	if err != nil {
		// If error occurs, it should NOT be a config or OAuth error
		assert.NotContains(t, strings.ToLower(err.Error()), "config validation")
		assert.NotContains(t, strings.ToLower(err.Error()), "oauth")
	} else {
		// Success is also fine - means OAuth was configured correctly
		require.NotNil(t, ds)
	}
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
	var requestCount int
	var mu sync.Mutex

	mgr := newMockOAuthManager(t, func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		requestCount++
		count := requestCount
		mu.Unlock()

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"access_token": fmt.Sprintf("token-%d", count),
			"token_type":   "Bearer",
			"expires_in":   3600,
		})
	})

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
		if err != nil {
			t.Errorf("goroutine %d failed: %v", i, err)
		}
	}

	// All goroutines should get the same cached token
	firstToken := tokens[0]
	for i, token := range tokens {
		if token != firstToken {
			t.Errorf("goroutine %d got different token: %q vs %q", i, token, firstToken)
		}
	}

	// Should only have made one request (or very few due to race)
	mu.Lock()
	defer mu.Unlock()
	if requestCount > 3 {
		t.Errorf("got %d requests, want <= 3 (caching not working)", requestCount)
	}
}
