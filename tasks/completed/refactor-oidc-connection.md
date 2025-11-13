# Refactor OIDC Connection Code

## Objective
Move OIDC connection logic from CLI-specific code to the main library, making it reusable and flexible by accepting explicit arguments instead of relying on environment variables.

## Current State

### CLI Connection Code (`cli/connection.py`)
- `_connect_with_oidc()`: Creates FlightSQL client with OIDC auth
  - Reads from environment variables:
    - `MICROMEGAS_OIDC_ISSUER`
    - `MICROMEGAS_OIDC_CLIENT_ID`
    - `MICROMEGAS_OIDC_CLIENT_SECRET`
    - `MICROMEGAS_TOKEN_FILE` (optional)
    - `MICROMEGAS_ANALYTICS_URI`
  - Uses `_load_or_login_oidc()` to get auth provider
  - Returns configured `FlightSQLClient`

- `_load_or_login_oidc()`: Loads existing tokens or performs browser login
  - Tries to load from file first
  - Falls back to browser login if no tokens or refresh fails
  - Parameters: issuer, client_id, client_secret, token_file

### Library Code (`micromegas/auth/oidc.py`)
- `OidcAuthProvider`: Already library-ready, accepts explicit arguments
- `OidcClientCredentialsProvider`: Has `from_env()` method that reads env vars

### Library Entry Point (`micromegas/__init__.py`)
- `connect()`: Simple connection with no auth, only takes `preserve_dictionary` argument

## Proposed Changes

### 1. Create new module `micromegas/oidc_connection.py`
Create a new module in the library with OIDC connection functions:
- `load_or_login()`: Loads existing tokens or performs browser login
  - Accepts: `issuer`, `client_id`, `client_secret` (optional), `token_file`
  - Tries to load from file if it exists
  - Falls back to browser login if needed
  - Returns `OidcAuthProvider` instance
  - This is essentially the `_load_or_login_oidc()` logic but as a public library function

- `connect()`: Creates FlightSQL client with OIDC auth
  - Accepts explicit arguments: `uri`, `issuer`, `client_id`, `client_secret` (optional), `token_file` (optional), `preserve_dictionary` (optional)
  - Calls `load_or_login()` to get auth provider
  - Returns configured `FlightSQLClient`
  - Does NOT rely on environment variables

### 2. Update CLI `connection.py` to use new library module
Refactor `_connect_with_oidc()` and `_load_or_login_oidc()` to:
- Read environment variables (as it does now)
- Call `micromegas.oidc_connection.connect()` with explicit arguments
- This keeps CLI behavior identical while using library code
- Remove the local `_load_or_login_oidc()` helper (now in library)

### 3. Update `OidcClientCredentialsProvider.from_env()` documentation
- Add note that this is a convenience method for CLI/scripts
- Point to constructor for library usage with explicit arguments

## Benefits

1. **Reusability**: Python applications can use OIDC auth without environment variables
2. **Testability**: Easier to test with explicit arguments vs mocking environment
3. **Flexibility**: Users can configure multiple connections with different credentials
4. **Separation of Concerns**: CLI remains thin wrapper over library functionality
5. **Documentation**: Clear API for library users vs CLI users

## Implementation Steps

1. Create new module `micromegas/oidc_connection.py` with:
   - `load_or_login()` function
   - `connect()` function
2. Update `cli/connection.py` to use new library module
3. Add tests for new library functions (optional but recommended)
4. Update documentation/examples

## API Examples

### Library Usage (Explicit Arguments)
```python
from micromegas import oidc_connection

# Connect with OIDC using explicit arguments
client = oidc_connection.connect(
    uri="grpc+tls://analytics.example.com:50051",
    issuer="https://accounts.google.com",
    client_id="my-client-id.apps.googleusercontent.com",
    client_secret="optional-secret",  # Optional for web apps
    token_file="~/.micromegas/tokens.json"  # Optional, defaults to standard location
)

df = client.query("SELECT * FROM logs")
```

### Advanced Usage (Separate Auth and Connection)
```python
from micromegas import oidc_connection
from micromegas.flightsql.client import FlightSQLClient

# Load or login with OIDC
auth = oidc_connection.load_or_login(
    issuer="https://accounts.google.com",
    client_id="my-client-id.apps.googleusercontent.com",
    token_file="~/.micromegas/tokens.json"
)

# Connect with auth provider
client = FlightSQLClient(
    "grpc+tls://analytics.example.com:50051",
    auth_provider=auth
)

df = client.query("SELECT * FROM logs")
```

### CLI Usage (Environment Variables)
```python
# cli/connection.py continues to work with env vars
import os
from micromegas import oidc_connection

def _connect_with_oidc():
    return oidc_connection.connect(
        uri=os.environ.get("MICROMEGAS_ANALYTICS_URI", "grpc://localhost:50051"),
        issuer=os.environ["MICROMEGAS_OIDC_ISSUER"],
        client_id=os.environ["MICROMEGAS_OIDC_CLIENT_ID"],
        client_secret=os.environ.get("MICROMEGAS_OIDC_CLIENT_SECRET"),
        token_file=os.environ.get("MICROMEGAS_TOKEN_FILE")
    )
```

## Files to Create/Modify

1. **CREATE**: `python/micromegas/micromegas/oidc_connection.py` - New module with:
   - `load_or_login()` function
   - `connect()` function
2. **MODIFY**: `python/micromegas/cli/connection.py` - Refactor to use library module
3. **OPTIONAL**: `python/micromegas/tests/` - Add tests for new library module

## Backward Compatibility

- CLI behavior remains unchanged (still uses environment variables)
- Existing library code unaffected
- New functions are additions, no breaking changes
- `from_env()` methods remain available for convenience
