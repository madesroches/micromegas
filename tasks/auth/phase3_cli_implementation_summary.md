# Phase 3: CLI OIDC Support - Implementation Summary

**Date:** 2025-10-28  
**Status:** ✅ COMPLETE

## Overview

Implemented OIDC authentication support for CLI tools with token persistence, browser-based login, and automatic token refresh. All existing CLI tools now support OIDC authentication without requiring any code changes.

## Changes Made

### 1. Updated `python/micromegas/cli/connection.py`

**What changed:**
- Added OIDC authentication support via environment variables
- Implemented token persistence with automatic refresh
- Maintained backward compatibility with `MICROMEGAS_PYTHON_MODULE_WRAPPER`

**Key features:**
- Checks for OIDC environment variables (issuer, client_id, client_secret)
- Loads existing tokens from file if available
- Opens browser for first-time authentication
- Re-authenticates automatically if token refresh fails
- Falls back to simple connection if no auth configured

**Environment variables:**
- `MICROMEGAS_OIDC_ISSUER` (required) - OIDC issuer URL
- `MICROMEGAS_OIDC_CLIENT_ID` (required) - OAuth client ID
- `MICROMEGAS_OIDC_CLIENT_SECRET` (optional) - Only for Web app clients
- `MICROMEGAS_TOKEN_FILE` (optional) - Custom token file path (default: ~/.micromegas/tokens.json)
- `MICROMEGAS_ANALYTICS_URI` (optional) - Analytics server URI (default: grpc://localhost:50051)

### 2. Created `python/micromegas/cli/logout.py`

**Purpose:** Command-line tool to clear saved OIDC tokens

**Features:**
- Respects `MICROMEGAS_TOKEN_FILE` environment variable
- Provides clear user feedback
- Safe to run even if no tokens exist

**Usage:**
```bash
micromegas_logout
```

### 3. Created `python/micromegas/cli/__init__.py`

**Purpose:** Makes CLI directory a proper Python package

### 4. Updated `python/micromegas/pyproject.toml`

**What changed:**
- Added `[tool.poetry.scripts]` section
- Registered `micromegas_logout` command as CLI entry point

**Effect:** After poetry install, users can run `micromegas_logout` from anywhere

### 5. Created `python/micromegas/examples/cli_oidc_example.py`

**Purpose:** Documentation and usage example showing:
- Environment variable configuration
- First-time authentication flow
- Subsequent usage (token reuse)
- Logout procedure
- Backward compatibility

## User Experience

### First-Time Usage
```bash
# Set environment variables
export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"
export MICROMEGAS_OIDC_CLIENT_ID="123-abc.apps.googleusercontent.com"

# Run any CLI tool - browser opens for authentication
python -m micromegas.cli.query_processes --since 1h

# Output:
# "No saved tokens found. Opening browser for authentication..."
# Browser opens → user authenticates → tokens saved
# Query executes
```

### Subsequent Usage
```bash
# Run CLI tools normally - no browser interaction
python -m micromegas.cli.query_processes --since 1h
python -m micromegas.cli.query_process_log <process-id>

# Tokens automatically:
# - Loaded from ~/.micromegas/tokens.json
# - Refreshed if expiring soon (5-minute buffer)
# - Never shown to user
```

### Logout
```bash
# Clear saved tokens
micromegas_logout

# Or manually
rm ~/.micromegas/tokens.json
```

## Backward Compatibility

### Corporate Wrapper Support
The existing `MICROMEGAS_PYTHON_MODULE_WRAPPER` pattern still works and takes precedence:

```bash
export MICROMEGAS_PYTHON_MODULE_WRAPPER="your_corporate_auth_module"
python -m micromegas.cli.query_processes
# Uses corporate auth module - OIDC not invoked
```

### No Auth Mode
If no environment variables are set, falls back to simple connection:

```bash
# No OIDC variables, no wrapper → uses micromegas.connect()
python -m micromegas.cli.query_processes
```

## Testing

### Unit Tests
- All existing OIDC unit tests pass (6/6)
- Tests cover token lifecycle, refresh, thread safety

### Manual Testing
Tested scenarios:
- First-time browser login
- Token file persistence
- Token reuse across CLI invocations
- Automatic token refresh
- Re-authentication on token expiration
- Backward compatibility with wrapper

## Implementation Details

### Token Persistence
- Tokens saved to `~/.micromegas/tokens.json` by default
- File format shared with Python client `OidcAuthProvider`
- Secure permissions (0600) enforced by OidcAuthProvider
- Contains: issuer, client_id, access_token, id_token, refresh_token, expiration

### Automatic Refresh
- Checks token expiration before each query
- Refreshes if expiring within 5 minutes
- Uses refresh_token for seamless renewal
- Re-authenticates if refresh fails

### Error Handling
- Token file corrupted → re-authenticate
- Token refresh failed → re-authenticate
- Network errors → clear error messages
- Graceful fallback to simple connection if OIDC not configured

## Integration

All existing CLI tools work without modification:
- `query_processes.py`
- `query_process_log.py`
- `query_process_metrics.py`
- `write_perfetto.py`

They all use `connection.connect()`, which now supports:
1. Corporate wrapper (via `MICROMEGAS_PYTHON_MODULE_WRAPPER`)
2. OIDC authentication (via `MICROMEGAS_OIDC_*` variables)
3. Simple connection (fallback)

## Next Steps

Phase 4 (Documentation) - Not started:
- Admin setup guide (Google/Azure AD/Okta registration)
- User authentication guide
- Troubleshooting guide
- More examples (Jupyter notebooks, etc.)

## Files Changed

```
python/micromegas/cli/connection.py          # Updated - OIDC support
python/micromegas/cli/logout.py              # New - logout command
python/micromegas/cli/__init__.py            # New - package marker
python/micromegas/pyproject.toml             # Updated - logout script
python/micromegas/examples/cli_oidc_example.py  # New - usage example
tasks/auth/oidc_auth_subplan.md              # Updated - status tracking
```

## Success Criteria ✅

All acceptance criteria met:
- ✅ First invocation opens browser and saves tokens
- ✅ Subsequent invocations use saved tokens (no browser)
- ✅ Tokens auto-refresh transparently
- ✅ All existing CLI tools work without modification
- ✅ Backward compatible with MICROMEGAS_PYTHON_MODULE_WRAPPER
- ✅ Shares same token file format as Python client
- ✅ Logout command available
- ✅ Clear user feedback for authentication state
- ✅ Secure token storage (via OidcAuthProvider)

## Conclusion

Phase 3 complete! CLI tools now have full OIDC authentication support with a seamless user experience. No code changes required for existing tools, and the implementation maintains full backward compatibility with existing authentication methods.
