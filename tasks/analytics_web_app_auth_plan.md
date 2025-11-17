# Authentication Plan for Analytics Web App

## Current State

- **Frontend**: OIDC authentication flow implemented (Phase 2 & 3 complete)
- **Backend (analytics-web-srv)**: OIDC authentication infrastructure implemented (Phase 1 complete)
- **FlightSQL/Ingestion servers**: Already have auth infrastructure (API keys or OIDC)

### Phase 1 Implementation Status (COMPLETED)

**Files Modified:**
- `rust/analytics-web-srv/src/auth.rs` - NEW: OIDC flow endpoints and cookie-based auth middleware
- `rust/analytics-web-srv/src/main.rs` - Modified: Added auth routes, `--disable-auth` flag, CORS credentials
- `rust/analytics-web-srv/Cargo.toml` - Modified: Added dependencies (axum-extra, openidconnect, base64, rand, reqwest, time, url)

**Features Implemented:**
- OIDC authorization code flow with PKCE (no client secret needed)
- httpOnly cookie storage for access and refresh tokens
- Cookie-based authentication middleware for API routes
- Token refresh endpoint
- User info endpoint
- Logout endpoint
- CORS with credentials support
- `--disable-auth` CLI flag for development
- Environment variables: `MICROMEGAS_OIDC_CLIENT_CONFIG`, `MICROMEGAS_COOKIE_DOMAIN`, `MICROMEGAS_SECURE_COOKIES`

**Not Yet Implemented:**
- Per-request token passthrough to FlightSQL (currently still uses `MICROMEGAS_AUTH_TOKEN`)
- Full JWT signature validation in middleware (currently validates expiry only)

### Phase 2 & 3 Implementation Status (COMPLETED)

**Files Created:**
- `analytics-web-app/src/lib/auth.tsx` - Auth context/provider with login, logout, refresh functions
- `analytics-web-app/src/app/login/page.tsx` - Login page with SSO redirect button and Suspense boundary
- `analytics-web-app/src/components/AuthGuard.tsx` - Route protection component with loading/error states
- `analytics-web-app/src/components/UserMenu.tsx` - User dropdown menu with logout functionality

**Files Modified:**
- `analytics-web-app/src/lib/api.ts` - Added `credentials: 'include'` to all fetch calls, AuthenticationError class for 401 handling
- `analytics-web-app/src/app/layout.tsx` - Wrapped app with AuthProvider
- `analytics-web-app/src/app/page.tsx` - Wrapped with AuthGuard, added UserMenu to header
- `analytics-web-app/src/app/process/[id]/page.tsx` - Wrapped with AuthGuard, added UserMenu to header
- `analytics-web-app/src/components/ErrorBoundary.tsx` - Added AuthenticationError handling with redirect to login

**Features Implemented:**
- Auth context with state management (`loading | authenticated | unauthenticated | error`)
- Automatic auth check on app load via `/auth/me`
- Login redirect to backend `/auth/login` with return URL preservation
- Logout via POST to `/auth/logout`
- Token refresh support via `/auth/refresh`
- Route protection with AuthGuard component
- User menu with name/email display and logout button
- 401 error handling with automatic redirect to login
- Cross-origin cookie support via `credentials: 'include'`
- Suspense boundary for Next.js 15 useSearchParams requirement

**Not Yet Implemented:**
- Environment variable template file (`.env.local.example`)

## Approach: OIDC Authentication Only

The analytics web app will use OIDC exclusively for authentication. No API key support - this is a user-facing web application that should use proper SSO flows.

---

## Phase 1: Backend Authentication Infrastructure (COMPLETED)

### Task 1.1: Add token proxy endpoints ✅ COMPLETED
**Files**: `rust/analytics-web-srv/src/auth.rs`, `rust/analytics-web-srv/src/main.rs`

Create backend endpoints to handle OIDC token exchange and secure cookie management:

**`GET /auth/login?return_url=/path`** ✅
- Accept optional `return_url` query param (default: `/`)
- Validate return_url is relative path starting with `/` (prevent open redirect)
- Generate random state nonce
- Store `{"nonce": "...", "return_url": "/path", "pkce_verifier": "..."}` as JSON, base64 encode
- Set `oauth_state` httpOnly cookie with the nonce (for validation)
- Redirect to OIDC provider authorization endpoint with encoded state and PKCE challenge

**`GET /auth/callback?code=...&state=...`** ✅
- Decode state parameter from base64 JSON
- Validate nonce matches `oauth_state` cookie
- Clear `oauth_state` cookie
- Exchange authorization code for tokens via OIDC token endpoint (with PKCE verifier)
- Set `access_token` and `refresh_token` httpOnly cookies
- Redirect to return_url from state (or `/` if missing)

**`POST /auth/refresh`** ✅
- Read refresh_token from cookie
- Exchange for new tokens via OIDC token endpoint
- Update both token cookies
- Return 200 on success, 401 if refresh fails

**`POST /auth/logout`** ✅
- Clear access_token and refresh_token cookies
- Return 200

**`GET /auth/me`** ✅
- Read access_token from cookie
- Decode JWT payload (no validation needed, just extract claims)
- Return `{"sub": "...", "email": "...", "name": "..."}`
- Return 401 if no cookie

See Security Considerations section for cookie and CSRF configuration details.

### Task 1.2: Add auth middleware to analytics-web-srv ✅ COMPLETED
**Files**: `rust/analytics-web-srv/src/auth.rs`, `rust/analytics-web-srv/src/main.rs`

- Created custom `cookie_auth_middleware` (reads from httpOnly cookie, not Authorization header)
- Apply auth middleware to all `/analyticsweb/*` routes (except health and /auth/*)
- Validates JWT expiry (signature validation deferred to FlightSQL)
- Add `--disable-auth` CLI flag for development
- Extract authenticated user token and store in request extensions (AuthToken wrapper)
- Note: Token passthrough to FlightSQL per-request not yet implemented

### Task 1.3: Environment variables ✅ COMPLETED
- Use `MICROMEGAS_OIDC_CLIENT_CONFIG` for OIDC client configuration (JSON with issuer, client_id, redirect_uri)
- `MICROMEGAS_COOKIE_DOMAIN` for cookie domain (optional)
- `MICROMEGAS_SECURE_COOKIES` for secure cookie flag (optional, defaults to false)
- Note: Still uses `MICROMEGAS_AUTH_TOKEN` for FlightSQL; per-request token passthrough pending

### Task 1.4: Configure CORS middleware ✅ COMPLETED
**File**: `rust/analytics-web-srv/src/main.rs`

- `tower-http` CORS layer already present, updated with `.allow_credentials(true)`
- Configure single allowed origin from environment variable (`ANALYTICS_WEB_CORS_ORIGIN`)
- Allow credentials (required for cross-origin cookie support)
- Set allowed methods: GET, POST, OPTIONS
- Set allowed headers: Content-Type, Authorization
- Example: `ANALYTICS_WEB_CORS_ORIGIN=https://app.yourdomain.com`
- Development: `ANALYTICS_WEB_CORS_ORIGIN=http://localhost:3000`

### Task 1.5: Token expiry and refresh strategy ✅ COMPLETED
**File**: `rust/analytics-web-srv/src/auth.rs`

- Store both access token and refresh token in separate httpOnly cookies ✅
- Access token cookie: short-lived (matches token expiry from OIDC provider, default 1 hour) ✅
- Refresh token cookie: long-lived (hardcoded to 30 days) ✅
- Auth middleware checks access token expiry before allowing request ✅
- Note: Automatic refresh not implemented; returns 401 if expired
- `/auth/refresh` endpoint for explicit refresh (frontend should call proactively) ✅
- Set cookie `maxAge` to match respective token expiry times ✅

---

## Phase 2: Frontend Authentication Flow (COMPLETED)

### Task 2.1: Create auth context/provider ✅ COMPLETED
**File**: `analytics-web-app/src/lib/auth.tsx`

- Login function (redirects to backend `/auth/login`) ✅
- Logout function (calls backend `/auth/logout`) ✅
- User info state (fetched from backend `/auth/me`) ✅
- Check auth status on app load ✅
- Token refresh function ✅
- No direct token access (handled by httpOnly cookies) ✅

**Auth check behavior:**
- Call `/auth/me` on app load ✅
- 200 with JSON: user is logged in, store user info ✅
- 401: user is not logged in, redirect to login ✅
- Network error or 5xx: service unavailable, show error page (not login redirect) ✅
- State: `loading | authenticated | unauthenticated | error` ✅

Note: No `oidc-client-ts` needed - backend handles all OIDC flows

### Task 2.2: Protect routes ✅ COMPLETED
**Files**:
- `analytics-web-app/src/app/layout.tsx` - Wrapped with AuthProvider ✅
- `analytics-web-app/src/components/AuthGuard.tsx` - NEW: Route protection component ✅
- `analytics-web-app/src/app/page.tsx` - Wrapped with AuthGuard ✅
- `analytics-web-app/src/app/process/[id]/page.tsx` - Wrapped with AuthGuard ✅

- Wrap app in auth provider ✅
- Redirect to login if unauthenticated (check via `/auth/me`) ✅
- Show loading state during auth check ✅
- Preserve return URL for post-login redirect (pass as state to backend) ✅
- Show error page for service unavailable (not login redirect) ✅

### Task 2.3: Update API client ✅ COMPLETED
**File**: `analytics-web-app/src/lib/api.ts`

- No Authorization header needed (cookies sent automatically) ✅
- Handle 401 responses (redirect to login via AuthenticationError) ✅
- Credentials: 'include' for cross-origin cookie support ✅
- Added AuthenticationError class for 401 handling ✅

### Task 2.4: Add login page ✅ COMPLETED
**File**: `analytics-web-app/src/app/login/page.tsx`

- Redirect to backend `/auth/login` endpoint ✅
- Pass return URL as query parameter ✅
- Error handling for auth failures ✅
- Display provider information ✅
- Suspense boundary for useSearchParams (Next.js 15 requirement) ✅
- Auto-redirect if already authenticated ✅

---

## Phase 3: UI Integration (COMPLETED)

### Task 3.1: Add user menu/logout button ✅ COMPLETED
**File**: `analytics-web-app/src/components/UserMenu.tsx`

- Display logged-in user info (name, email) ✅
- Logout functionality (POST request) ✅
- Dropdown menu with user details ✅
- Loading state during logout ✅

### Task 3.2: Update header/layout ✅ COMPLETED
**Files**:
- `analytics-web-app/src/app/page.tsx` - Added UserMenu to header ✅
- `analytics-web-app/src/app/process/[id]/page.tsx` - Added UserMenu to header ✅

- Include UserMenu component in all protected pages ✅
- Show auth status via UserMenu ✅
- Consistent navigation ✅

### Task 3.3: Error handling for auth errors ✅ COMPLETED
**Files**:
- `analytics-web-app/src/components/ErrorBoundary.tsx` - Updated with AuthenticationError handling ✅
- `analytics-web-app/src/lib/api.ts` - Added AuthenticationError class ✅

- Distinguish 401 (unauthorized) from other errors ✅
- Redirect to login with return URL on 401 ✅
- Show appropriate error messages ✅

---

## Phase 4: Testing & Documentation

### Task 4.1: Backend testing ✅ COMPLETED
- ✅ Unit tests for auth middleware (12 tests in src/auth.rs)
- ✅ Integration tests with JWT token validation (13 tests in tests/auth_integration.rs)
- ✅ Test OIDC token validation (JWT decoding, expiry checking)
- ✅ Test CSRF protection (state/nonce validation tested in unit tests)
- ✅ Test cookie security (httpOnly, SameSite=Lax, secure flags)
- ✅ Test error handling and status codes
- ✅ Test /auth/me and /auth/logout endpoints

**Results**: 37/37 backend tests passing

### Task 4.2: Frontend testing ✅ COMPLETED
- ✅ Unit tests for auth hooks (AuthProvider, useAuth - 13 tests)
- ✅ Integration tests for protected routes (AuthGuard - 6 tests)
- ✅ Component tests (UserMenu - 6 tests)
- ✅ Test token refresh flow
- ✅ Test login/logout cycle
- ✅ Test logout uses POST method
- ✅ Test error handling (401, 500, network errors)
- ✅ Test loading and error states

**Results**: 25/25 frontend tests passing

### Task 4.3: Documentation
(let's keep it TLDR and avoid any repetition - there is already plenty of auth doc)
- Update environment variable docs
- OIDC provider setup guide (Google, Keycloak, etc.)
- Development mode instructions (disable-auth)
- Deployment configuration guide

---

## Files to Create/Modify

### Backend (Rust) - PHASE 1 COMPLETED ✅
| File | Action | Status | Description |
|------|--------|--------|-------------|
| `rust/analytics-web-srv/src/auth.rs` | Create | ✅ Done | Token proxy logic, cookie management, auth middleware |
| `rust/analytics-web-srv/src/main.rs` | Modify | ✅ Done | Add auth routes, --disable-auth flag, CORS credentials |
| `rust/analytics-web-srv/Cargo.toml` | Modify | ✅ Done | Add dependencies: axum-extra, openidconnect, base64, rand, reqwest, time, url |

### Frontend (TypeScript) - PHASE 2 & 3 COMPLETED ✅
| File | Action | Status | Description |
|------|--------|--------|-------------|
| `analytics-web-app/src/lib/auth.tsx` | Create | ✅ Done | Auth context/provider (no token storage) |
| `analytics-web-app/src/lib/api.ts` | Modify | ✅ Done | Add credentials: 'include', 401 handling |
| `analytics-web-app/src/app/login/page.tsx` | Create | ✅ Done | Login redirect page with Suspense |
| `analytics-web-app/src/app/layout.tsx` | Modify | ✅ Done | Wrap with auth provider |
| `analytics-web-app/src/app/page.tsx` | Modify | ✅ Done | Wrap with AuthGuard, add UserMenu |
| `analytics-web-app/src/app/process/[id]/page.tsx` | Modify | ✅ Done | Wrap with AuthGuard, add UserMenu |
| `analytics-web-app/src/components/AuthGuard.tsx` | Create | ✅ Done | Route protection component |
| `analytics-web-app/src/components/UserMenu.tsx` | Create | ✅ Done | User info/logout UI |
| `analytics-web-app/src/components/ErrorBoundary.tsx` | Modify | ✅ Done | Handle 401 errors |
| `analytics-web-app/.env.local.example` | Create | Pending | Environment variable template |

### Testing (TypeScript/Rust) - PHASE 4 COMPLETED ✅
| File | Action | Status | Description |
|------|--------|--------|-------------|
| `rust/analytics-web-srv/src/lib.rs` | Create | ✅ Done | Library module for test access |
| `rust/analytics-web-srv/src/auth.rs` | Modify | ✅ Done | Added unit tests module (12 tests) |
| `rust/analytics-web-srv/tests/auth_integration.rs` | Create | ✅ Done | Integration tests (13 tests) |
| `rust/analytics-web-srv/Cargo.toml` | Modify | ✅ Done | Added dev-dependencies, lib section |
| `analytics-web-app/jest.config.js` | Create | ✅ Done | Jest configuration for Next.js |
| `analytics-web-app/src/test-setup.ts` | Create | ✅ Done | Test setup with mocks |
| `analytics-web-app/src/lib/__tests__/auth.test.tsx` | Create | ✅ Done | Auth context tests (13 tests) |
| `analytics-web-app/src/components/__tests__/AuthGuard.test.tsx` | Create | ✅ Done | Route protection tests (6 tests) |
| `analytics-web-app/src/components/__tests__/UserMenu.test.tsx` | Create | ✅ Done | User menu tests (6 tests) |
| `analytics-web-app/package.json` | Modify | ✅ Done | Added test scripts and dependencies |

### Documentation
| File | Action | Description |
|------|--------|-------------|
| `analytics-web-app/README.md` | Modify | Add auth setup instructions |
| `docs/` or `mkdocs/` | Modify | Update deployment docs |

---

## Environment Variables

### Backend (IMPLEMENTED)
```bash
# OIDC Client Configuration (required unless --disable-auth flag is used)
MICROMEGAS_OIDC_CLIENT_CONFIG='{
  "issuer": "https://accounts.google.com",
  "client_id": "your-client-id.apps.googleusercontent.com",
  "redirect_uri": "http://localhost:3000/auth/callback"
}'

# Cookie Configuration (optional)
MICROMEGAS_COOKIE_DOMAIN=.yourdomain.com  # Cookie domain (omit for localhost)
MICROMEGAS_SECURE_COOKIES=true  # Set to true for production (HTTPS only)

# CORS Configuration (existing)
ANALYTICS_WEB_CORS_ORIGIN=http://localhost:3000  # Must match frontend URL

# FlightSQL Authentication (unchanged, will be replaced with per-request tokens)
MICROMEGAS_AUTH_TOKEN=your-flightsql-token

# Development: use --disable-auth CLI flag
# ./analytics-web-srv --disable-auth
```

### Frontend
```bash
# Backend API URL (for auth redirects)
NEXT_PUBLIC_API_URL=http://localhost:3000

# No OIDC config needed - backend handles all OIDC flows
# Cookies are httpOnly, frontend cannot access tokens
```

### Example: Google OIDC Setup
1. Go to Google Cloud Console > APIs & Services > Credentials
2. Create OAuth 2.0 Client ID (Web application)
3. Add authorized redirect URI: `http://localhost:3000/auth/callback`
4. Copy Client ID to `MICROMEGAS_OIDC_CLIENT_CONFIG`

---

## Implementation Order

### Phase 1 - Backend (COMPLETED ✅)
1. ✅ **Task 1.1** - Backend token proxy endpoints (foundation)
2. ✅ **Task 1.2** - Backend auth middleware (cookie-based)
3. ✅ **Task 1.3** - Backend environment variables
4. ✅ **Task 1.4** - CORS middleware configuration
5. ✅ **Task 1.5** - Token expiry and refresh strategy

### Phase 2 - Frontend (COMPLETED ✅)
6. ✅ **Task 2.1** - Frontend auth context (fetch user from backend)
7. ✅ **Task 2.3** - API client with credentials: 'include'
8. ✅ **Task 2.4** - Login page
9. ✅ **Task 2.2** - Route protection

### Phase 3 - UI Polish (COMPLETED ✅)
10. ✅ **Task 3.1-3.3** - UI polish (user menu, error handling)

### Phase 4 - Testing & Documentation (COMPLETED ✅)
11. ✅ **Task 4.1** - Backend testing (37 tests passing)
12. ✅ **Task 4.2** - Frontend testing (25 tests passing)
13. **Task 4.3** - Documentation (pending)

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

