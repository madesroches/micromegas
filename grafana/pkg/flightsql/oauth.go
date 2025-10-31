package flightsql

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"

	"golang.org/x/oauth2"
	"golang.org/x/oauth2/clientcredentials"
)

// OAuthTokenManager handles OAuth 2.0 client credentials flow
// Uses golang.org/x/oauth2 for automatic token caching and refresh
type OAuthTokenManager struct {
	tokenSource oauth2.TokenSource
	config      *clientcredentials.Config
}

// NewOAuthTokenManager creates a new OAuth token manager
// The oauth2 library handles caching and automatic token refresh
func NewOAuthTokenManager(issuer, clientId, clientSecret, audience string) (*OAuthTokenManager, error) {
	// Discover token endpoint from OIDC provider
	tokenEndpoint, err := discoverTokenEndpoint(issuer)
	if err != nil {
		return nil, fmt.Errorf("OIDC discovery failed: %w", err)
	}

	// Configure client credentials flow
	config := &clientcredentials.Config{
		ClientID:     clientId,
		ClientSecret: clientSecret,
		TokenURL:     tokenEndpoint,
	}

	// Add audience if provided (required for Auth0/Azure AD)
	if audience != "" {
		config.EndpointParams = map[string][]string{
			"audience": {audience},
		}
	}

	logInfof("OAuth token manager initialized: issuer=%s, endpoint=%s", issuer, tokenEndpoint)

	// Create token source - handles all caching and refresh automatically!
	tokenSource := config.TokenSource(context.Background())

	return &OAuthTokenManager{
		tokenSource: tokenSource,
		config:      config,
	}, nil
}

// GetToken returns a valid access token
// The oauth2 library automatically handles caching and refresh
func (m *OAuthTokenManager) GetToken(ctx context.Context) (string, error) {
	token, err := m.tokenSource.Token()
	if err != nil {
		return "", fmt.Errorf("failed to get OAuth token: %w", err)
	}

	// Note: No logging here - this is called on every query (hot path)
	// Token caching and refresh are handled automatically by oauth2 library

	return token.AccessToken, nil
}

// discoverTokenEndpoint fetches OIDC discovery document to find token endpoint
func discoverTokenEndpoint(issuer string) (string, error) {
	discoveryURL := strings.TrimSuffix(issuer, "/") + "/.well-known/openid-configuration"

	// Create context with 10 second timeout to prevent indefinite hanging
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Create HTTP request with timeout context
	req, err := http.NewRequestWithContext(ctx, "GET", discoveryURL, nil)
	if err != nil {
		return "", fmt.Errorf("failed to create discovery request: %w", err)
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("discovery request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("discovery failed with status: %d", resp.StatusCode)
	}

	var discovery struct {
		TokenEndpoint string `json:"token_endpoint"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&discovery); err != nil {
		return "", fmt.Errorf("failed to parse discovery response: %w", err)
	}

	if discovery.TokenEndpoint == "" {
		return "", fmt.Errorf("token_endpoint not found in discovery document")
	}

	return discovery.TokenEndpoint, nil
}
