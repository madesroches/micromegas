# Authentication Plan for Analytics Web App

## Current State

- **Frontend**: No authentication (fully open)
- **Backend (analytics-web-srv)**: Optional token passed to FlightSQL only, no HTTP middleware
- **FlightSQL/Ingestion servers**: Already have auth infrastructure (API keys or OIDC)

## Approach: OIDC Authentication Only

The analytics web app will use OIDC exclusively for authentication. No API key support - this is a user-facing web application that should use proper SSO flows.

---

## Phase 1: Backend Authentication Infrastructure

### Task 1.1: Add token proxy endpoints
**File**: `rust/analytics-web-srv/src/main.rs`

Create backend endpoints to handle OIDC token exchange and secure cookie management:

**`GET /auth/login?return_url=/path`**
- Accept optional `return_url` query param (default: `/`)
- Validate return_url is relative path starting with `/` (prevent open redirect)
- Generate random state nonce
- Store `{"nonce": "...", "return_url": "/path"}` as JSON, base64 encode
- Set `oauth_state` httpOnly cookie with the nonce (for validation)
- Redirect to OIDC provider authorization endpoint with encoded state

**`GET /auth/callback?code=...&state=...`**
- Decode state parameter from base64 JSON
- Validate nonce matches `oauth_state` cookie
- Clear `oauth_state` cookie
- Exchange authorization code for tokens via OIDC token endpoint
- Set `access_token` and `refresh_token` httpOnly cookies
- Redirect to return_url from state (or `/` if missing)

**`POST /auth/refresh`**
- Read refresh_token from cookie
- Exchange for new tokens via OIDC token endpoint
- Update both token cookies
- Return 200 on success, 401 if refresh fails

**`POST /auth/logout`**
- Clear access_token and refresh_token cookies
- Return 200

**`GET /auth/me`**
- Read access_token from cookie
- Decode JWT payload (no validation needed, just extract claims)
- Return `{"sub": "...", "email": "...", "name": "..."}`
- Return 401 if no cookie

See Security Considerations section for cookie and CSRF configuration details.

### Task 1.2: Add auth middleware to analytics-web-srv
**File**: `rust/analytics-web-srv/src/main.rs`

- Import `micromegas::auth::axum` middleware
- Apply auth middleware to all `/analyticsweb/*` routes (except health and /auth/*)
- Read JWT token from httpOnly cookie (not Authorization header)
- Validate OIDC JWT tokens only (no API key support)
- Add `--disable-auth` CLI flag for development
- Extract authenticated user info and pass to FlightSQL client per-request

### Task 1.3: Environment variables
- Use `MICROMEGAS_OIDC_CONFIG` for OIDC provider configuration (includes client_id)
- Token from cookie passed to FlightSQL client per-request
- Remove `MICROMEGAS_AUTH_TOKEN` in favor of per-request tokens

### Task 1.4: Configure CORS middleware
**File**: `rust/analytics-web-srv/src/main.rs`

- Add `tower-http` CORS layer to axum router
- Configure single allowed origin from environment variable (`MICROMEGAS_CORS_ORIGIN`)
- Allow credentials (required for cross-origin cookie support)
- Set allowed methods: GET, POST, OPTIONS
- Set allowed headers: Content-Type, Accept
- Expose headers: Content-Type
- Set max age for preflight caching (e.g., 3600 seconds)
- Example: `MICROMEGAS_CORS_ORIGIN=https://app.yourdomain.com`
- Development: `MICROMEGAS_CORS_ORIGIN=http://localhost:3000`

### Task 1.5: Token expiry and refresh strategy
**File**: `rust/analytics-web-srv/src/auth.rs`

- Store both access token and refresh token in separate httpOnly cookies
- Access token cookie: short-lived (matches token expiry, typically 1 hour)
- Refresh token cookie: long-lived (matches refresh token expiry, typically 7-30 days)
- Auth middleware checks access token expiry before validation
- If access token expired but refresh token valid:
  - Automatically refresh tokens via OIDC provider
  - Update both cookies with new tokens
  - Continue request processing
- If both tokens expired: return 401, frontend redirects to login
- `/auth/refresh` endpoint for explicit refresh (frontend can call proactively)
- Set cookie `maxAge` to match respective token expiry times

---

## Phase 2: Frontend Authentication Flow

### Task 2.1: Create auth context/provider
**File**: `analytics-web-app/src/lib/auth.tsx`

- Login function (redirects to backend `/auth/login`)
- Logout function (calls backend `/auth/logout`)
- User info state (fetched from backend `/auth/me`)
- Check auth status on app load
- No direct token access (handled by httpOnly cookies)

**Auth check behavior:**
- Call `/auth/me` on app load
- 200 with JSON: user is logged in, store user info
- 401: user is not logged in, redirect to login
- Network error or 5xx: service unavailable, show error page (not login redirect)
- State: `loading | authenticated | unauthenticated | error`

Note: No `oidc-client-ts` needed - backend handles all OIDC flows

### Task 2.2: Protect routes
**Files**:
- `analytics-web-app/src/app/layout.tsx`
- `analytics-web-app/src/components/AuthGuard.tsx`

- Wrap app in auth provider
- Redirect to login if unauthenticated (check via `/auth/me`)
- Show loading state during auth check
- Preserve return URL for post-login redirect (pass as state to backend)

### Task 2.3: Update API client
**File**: `analytics-web-app/src/lib/api.ts`

- No Authorization header needed (cookies sent automatically)
- Handle 401 responses (redirect to login)
- Credentials: 'include' for cross-origin cookie support
- Retry logic for token refresh (backend handles refresh)

### Task 2.4: Add login page
**File**: `analytics-web-app/src/app/login/page.tsx`

- Redirect to backend `/auth/login` endpoint
- Pass return URL as query parameter
- Error handling for auth failures
- Display provider information

---

## Phase 3: UI Integration

### Task 3.1: Add user menu/logout button
**File**: `analytics-web-app/src/components/UserMenu.tsx`

- Display logged-in user info (name, email)
- Logout functionality
- Session indicator
- Link to user settings (if applicable)

### Task 3.2: Update header/layout
**File**: `analytics-web-app/src/app/layout.tsx`

- Include UserMenu component
- Show auth status
- Consistent navigation

### Task 3.3: Error handling for auth errors
**Files**:
- `analytics-web-app/src/components/ErrorBoundary.tsx`
- `analytics-web-app/src/lib/api.ts`

- Distinguish 401 (unauthorized) from other errors
- Clear session on auth failure
- Redirect to login with return URL
- Show appropriate error messages

---

## Phase 4: Testing & Documentation

### Task 4.1: Backend testing
- Unit tests for auth middleware
- Integration tests with mock OIDC provider
- Test OIDC token validation
- Test disable-auth flag
- Test CSRF protection (see Security Considerations for requirements)

### Task 4.2: Frontend testing
- Unit tests for auth hooks
- Integration tests for protected routes
- Test token refresh flow
- Test login/logout cycle
- Test logout uses POST method

### Task 4.3: Documentation
- Update environment variable docs
- OIDC provider setup guide (Google, Keycloak, etc.)
- Development mode instructions (disable-auth)
- Deployment configuration guide

---

## Files to Create/Modify

### Backend (Rust)
| File | Action | Description |
|------|--------|-------------|
| `rust/analytics-web-srv/src/main.rs` | Modify | Add auth endpoints, middleware, cookie handling |
| `rust/analytics-web-srv/src/auth.rs` | Create | Token proxy logic, cookie management |
| `rust/analytics-web-srv/Cargo.toml` | Modify | Add cookie and OIDC client dependencies |

### Frontend (TypeScript)
| File | Action | Description |
|------|--------|-------------|
| `analytics-web-app/src/lib/auth.tsx` | Create | Auth context/provider (no token storage) |
| `analytics-web-app/src/lib/api.ts` | Modify | Add credentials: 'include', 401 handling |
| `analytics-web-app/src/app/login/page.tsx` | Create | Login redirect page |
| `analytics-web-app/src/app/layout.tsx` | Modify | Wrap with auth provider |
| `analytics-web-app/src/components/AuthGuard.tsx` | Create | Route protection component |
| `analytics-web-app/src/components/UserMenu.tsx` | Create | User info/logout UI |
| `analytics-web-app/src/components/ErrorBoundary.tsx` | Modify | Handle 401 errors |
| `analytics-web-app/.env.local.example` | Create | Environment variable template |

### Documentation
| File | Action | Description |
|------|--------|-------------|
| `analytics-web-app/README.md` | Modify | Add auth setup instructions |
| `docs/` or `mkdocs/` | Modify | Update deployment docs |

---

## Environment Variables

### Backend
```bash
# OIDC Configuration (required unless auth disabled)
MICROMEGAS_OIDC_CONFIG={"issuer":"https://accounts.google.com","audience":"your-client-id"}
MICROMEGAS_COOKIE_DOMAIN=.yourdomain.com  # Cookie domain (optional)
MICROMEGAS_CORS_ORIGIN=https://app.yourdomain.com  # Single allowed origin per deployment

# Development only: use --disable-auth CLI flag instead of environment variable
```

### Frontend
```bash
# Backend API URL (for auth redirects)
NEXT_PUBLIC_API_URL=http://localhost:8080

# No OIDC config needed - backend handles all OIDC flows
# Cookies are httpOnly, frontend cannot access tokens
```

---

## Implementation Order

1. **Task 1.1** - Backend token proxy endpoints (foundation)
2. **Task 1.2** - Backend auth middleware (cookie-based)
3. **Task 1.3** - Backend environment variables
4. **Task 1.4** - CORS middleware configuration
5. **Task 1.5** - Token expiry and refresh strategy
6. **Task 2.1** - Frontend auth context (fetch user from backend)
7. **Task 2.3** - API client with credentials: 'include'
8. **Task 2.4** - Login page
9. **Task 2.2** - Route protection
10. **Task 3.1-3.3** - UI polish (user menu, error handling)
11. **Task 4.1-4.3** - Testing and docs

---

## Security Considerations

### Cookie Configuration
- **httpOnly**: true (prevents XSS token theft)
- **Secure**: true in production (HTTPS only)
- **SameSite**: Lax (CSRF protection for state-changing requests)
- **path**: /
- **maxAge**: Match token expiry
- **No signing needed**: JWT is already signed by OIDC provider, tamper-proof by design

### CSRF Protection
- **OIDC state parameter**: Generate random state before redirect, store in httpOnly cookie (`oauth_state`), validate on callback
- **POST for logout**: Logout requires POST request (SameSite=Lax blocks cross-origin POST with cookies)
- **Origin header validation**: Optionally validate Origin/Referer headers on sensitive endpoints

### Additional Security
- **PKCE flow**: Use PKCE for authorization code exchange (no client secret needed)
- **Token validation**: Validate JWT on every request (middleware)
- **CORS**: Configure allowed origins for production domains only

---

## Dependencies

### Backend (Rust)
- `micromegas-auth` crate (already has `openidconnect`, `reqwest`, `jsonwebtoken`)
- `tower-cookies` for cookie management
- `tower-http` for CORS middleware

### Frontend (TypeScript)
- No additional auth libraries needed
- Standard fetch API with credentials: 'include'

### External
- OIDC provider account (Google, Keycloak, or any OIDC-compliant provider)
- Client ID from OIDC provider (public client, no secret needed with PKCE)

