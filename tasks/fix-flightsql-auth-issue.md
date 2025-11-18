# Fix FlightSQL Authentication Issue

## Problem
The analytics web app displays "FlightSQL Connection Required" error instead of showing the list of processes. The token sent to the FlightSQL service is not being accepted.

## TL;DR - The Fix

**ONE LINE CHANGE in `rust/analytics-web-srv/src/auth.rs` line ~714:**
```rust
// Change cookie_auth_middleware to use ID token instead of access token:
let id_token = jar.get(ID_TOKEN_COOKIE)...  // was: ACCESS_TOKEN_COOKIE
```

**Why:** The Python API uses ID tokens and works fine. The web app was incorrectly using access tokens (JWE) which FlightSQL cannot validate. FlightSQL expects ID tokens (JWT) just like the Python client sends.

## ROOT CAUSE IDENTIFIED

**Token Type Confusion: Access Token (JWE) vs ID Token (JWT)**

The analytics-web-srv is sending the **wrong token type** to FlightSQL!

### How the Python API Works (CORRECTLY)

**python/micromegas/micromegas/auth/oidc.py (Lines 327-350):**
```python
def get_token(self) -> str:
    """Get valid ID token, refreshing if necessary.

    Returns:
        Valid ID token for Authorization header  # <-- Returns ID TOKEN
    """
    # ...refresh logic...
    id_token = self.client.token["id_token"]  # <-- Uses ID TOKEN
    return id_token
```

The Python client sends the **ID token (JWT)** to FlightSQL, and it works perfectly because:
1. ID tokens are JWTs that can be validated locally
2. FlightSQL's OIDC auth provider validates JWT signatures
3. No token introspection needed

### How the Web App Works (INCORRECTLY)

**analytics-web-srv/src/auth.rs (Lines 233-234, 444-449, 470-486):**
```rust
const ID_TOKEN_COOKIE: &str = "id_token";  // Contains ID token (JWT) for user info
const ACCESS_TOKEN_COOKIE: &str = "access_token";  // Contains access token (JWE) for FlightSQL API  <-- WRONG COMMENT!

// In auth_callback:
let access_token = token_response.access_token;  // JWE - CANNOT be validated by FlightSQL
let id_token = token_response.id_token.ok_or(...)?;  // JWT - CAN be validated by FlightSQL

// Stores BOTH tokens in separate cookies
```

**analytics-web-srv/src/auth.rs (Lines 706-733 - cookie_auth_middleware):**
```rust
// Extracts ACCESS_TOKEN (JWE) from cookie to send to FlightSQL
let access_token = jar.get(ACCESS_TOKEN_COOKIE)...  // <-- WRONG TOKEN!
req.extensions_mut().insert(AuthToken(access_token));
```

**The Problem:**
- The middleware sends the **access token (JWE)** to FlightSQL service
- FlightSQL expects an **ID token (JWT)** just like the Python client sends
- JWE access tokens cannot be validated by FlightSQL's OIDC provider
- Auth0 access tokens are encrypted (JWE) and require token introspection

### Why This is Counterintuitive

The comment says "access token (JWE) for FlightSQL API" but this is **wrong**. The Python API proves that FlightSQL expects **ID tokens**, not access tokens. Access tokens are for OAuth2 API authorization with resource servers, not for OIDC authentication with internal services.

## Solution: Match Python API Behavior (Use ID Token)

The Python API already proves this works! We just need to make the web app do the same thing.

## Implementation Plan

### Why This Works
1. **Proven**: Python API uses ID tokens and works perfectly with FlightSQL
2. **Simpler**: No token introspection needed
3. **Faster**: Local JWT validation, no HTTP calls to Auth0
4. **Already working**: The `/auth/me` endpoint already validates ID tokens
5. **Consistent**: Same token type across all micromegas clients

### Changes Required in `rust/analytics-web-srv/src/auth.rs`

**Single Change: Use ID_TOKEN_COOKIE in cookie_auth_middleware**

Currently the middleware uses:
```rust
let access_token = jar.get(ACCESS_TOKEN_COOKIE)...  // Line 714 - WRONG!
```

Should be:
```rust
let id_token = jar.get(ID_TOKEN_COOKIE)...  // Use ID token like Python API
```

That's it! Everything else already works:
- `auth_callback` already stores ID token in `ID_TOKEN_COOKIE` (line 472-477)
- `auth_me` already uses ID token from `ID_TOKEN_COOKIE` (line 616-622)
- `auth_logout` already clears `ID_TOKEN_COOKIE` (line 601)
- FlightSQL already validates ID tokens (just like Python API)

#### Optional Cleanup (not required for fix):
- Update comments to clarify that ID token is used for both user auth and API calls
- Remove confusing "for FlightSQL API" comment on ACCESS_TOKEN_COOKIE (line 234)
- Consider removing ACCESS_TOKEN_COOKIE entirely if not needed for future use

### Minimal Fix

**File: `rust/analytics-web-srv/src/auth.rs`**

Change line ~714 in `cookie_auth_middleware`:
```rust
// FROM:
let access_token = jar.get(ACCESS_TOKEN_COOKIE)...

// TO:
let id_token = jar.get(ID_TOKEN_COOKIE)...
```

And update the variable name throughout the function to use `id_token` instead of `access_token`.

**No other files need changes!**

### Optional Cleanup

For clarity, update comments in `rust/analytics-web-srv/src/auth.rs`:

Line 233-234:
```rust
// FROM:
const ID_TOKEN_COOKIE: &str = "id_token";  // Contains ID token (JWT) for user info
const ACCESS_TOKEN_COOKIE: &str = "access_token";  // Contains access token (JWE) for FlightSQL API

// TO:
const ID_TOKEN_COOKIE: &str = "id_token";  // ID token (JWT) for user info and FlightSQL API authorization
const ACCESS_TOKEN_COOKIE: &str = "access_token";  // Access token (reserved for future use)
```

Lines 701-704 (middleware comment):
```rust
// FROM:
/// Note: We use the access token (JWE) for FlightSQL API calls because:
/// - Access tokens are meant for API authorization
/// - The FlightSQL service will validate the access token with the OIDC provider
/// - ID tokens are only used for user info extraction in /auth/me

// TO:
/// Note: We use the ID token (JWT) for FlightSQL API calls because:
/// - ID tokens can be validated locally by FlightSQL's OIDC provider
/// - This matches the Python API behavior which also uses ID tokens
/// - Access tokens (JWE) would require token introspection endpoints
```

### Implementation Progress

✅ **ALL FIXES COMPLETED:**

**Backend Changes (`rust/analytics-web-srv/`):**
1. **Auth middleware fix:** Updated `cookie_auth_middleware` to use `ID_TOKEN_COOKIE` instead of `ACCESS_TOKEN_COOKIE` (auth.rs line 714)
2. Updated all variable names in the function from `access_token` to `id_token`
3. Updated middleware comment to explain why we use ID tokens (auth.rs lines 696-704)
4. Updated cookie constant comments to clarify usage (auth.rs lines 233-234)
5. **Health check fix:** Removed FlightSQL connectivity check from health endpoint (main.rs line 313)
6. Removed state dependency from health_routes (main.rs line 227)
7. Health endpoint now always returns `status: "healthy"` and `flightsql_connected: false`
8. Rust code compiles successfully

**Frontend Changes (`analytics-web-app/src/app/page.tsx`):**
1. Removed health check query (lines 21-25 deleted)
2. Removed `enabled: health?.flightsql_connected === true` from processes query (line 36)
3. Removed "FlightSQL Connection Required" UI block (lines 101-110)
4. Removed unused `fetchHealthCheck` import (line 6)
5. Processes query now runs immediately after authentication
6. Error handling now properly shows "Failed to Load Processes" if API call fails

## Detailed Investigation History

### Initial Problem Discovery

The health check endpoint was using an empty token from environment variable:
- `/analyticsweb/health` used `state.auth_token` from `MICROMEGAS_AUTH_TOKEN` env var
- Env var not set → empty string
- Frontend relied on health check's `flightsql_connected` field
- Showed "FlightSQL Connection Required" error

### Root Cause Analysis

Two separate issues identified:
1. **Auth middleware using wrong token type** - Using access token (JWE) instead of ID token (JWT)
2. **Health check architecture problem** - Health endpoint shouldn't check authenticated services

### Solution Implemented

**Backend fixes:**
1. Changed `cookie_auth_middleware` to extract ID token instead of access token
2. Simplified health check to be stateless and unauthenticated
3. Added client type identification for better logging

**Frontend fixes:**
1. Removed dependency on health check for FlightSQL connectivity
2. Directly query `/analyticsweb/processes` after authentication
3. Proper error handling when API calls fail

### Testing Results

1. [x] Login flow works ✅
2. [x] `/auth/me` returns user info correctly ✅
3. [x] `/analyticsweb/processes` connects to FlightSQL successfully ✅
4. [x] Process list is displayed in web app ✅
5. [x] FlightSQL logs show `client=web` ✅

## ✅ SUCCESS - Issue Resolved!

The analytics web app now successfully:
- Authenticates users via OIDC
- Extracts ID token from cookies
- Passes ID token to FlightSQL service
- FlightSQL validates the token
- Process list displays correctly!

## Additional Improvement: Client Type Identification

Added `x-client-type` header to identify web app requests in FlightSQL logs.

**Changes:**
1. `rust/public/src/client/flightsql_client_factory.rs`:
   - Added `client_type` field to `BearerFlightSQLClientFactory`
   - Added `new_with_client_type()` constructor
   - Sets `x-client-type` header when creating FlightSQL client

2. `rust/analytics-web-srv/src/main.rs`:
   - Updated all `BearerFlightSQLClientFactory::new()` calls to `new_with_client_type(..., "web".to_string())`
   - Affects: `list_processes`, `get_trace_info`, `get_process_log_entries`, `get_process_statistics`, `generate_trace`

**Result:**
FlightSQL logs now show `client=web` instead of `client=unknown`:
```
INFO execute_query ... user=google-oauth2|... email=... client=web
```

### Files Changed

**Backend:**
- `rust/analytics-web-srv/src/auth.rs` - Cookie auth middleware uses ID tokens
- `rust/analytics-web-srv/src/main.rs` - Health check simplified, client type added
- `rust/public/src/client/flightsql_client_factory.rs` - Client type support added

**Frontend:**
- `analytics-web-app/src/app/page.tsx` - Removed health check dependency

**Documentation:**
- `tasks/fix-flightsql-auth-issue.md` - Complete investigation and resolution

## Success Criteria ✅ ALL MET

- ✅ Web app successfully connects to FlightSQL service
- ✅ List of processes is displayed correctly
- ✅ Token is validated by FlightSQL service
- ✅ User can execute queries through the web interface
- ✅ Code is clear and not counterintuitive about which token is used where

## Key Learnings

1. **ID tokens vs Access tokens matter:** Auth0 issues JWE (encrypted) access tokens that cannot be validated locally. ID tokens (JWT) can be validated by OIDC providers like FlightSQL uses.

2. **Python API was the blueprint:** The working Python client (`python/micromegas/micromegas/auth/oidc.py`) showed us that ID tokens are the correct choice.

3. **Health checks should be stateless:** Health endpoints shouldn't perform authenticated operations or depend on application state.

4. **Frontend should fail gracefully:** Don't preemptively block features based on health checks - try the actual API call and handle errors properly.
