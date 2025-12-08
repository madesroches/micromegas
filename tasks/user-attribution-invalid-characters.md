# User Attribution Invalid Characters Bug

## Problem

The Grafana plugin sends user attribution headers (`x-user-name`, `x-user-email`, `x-user-id`) containing UTF-8 characters (e.g., `é`), which causes the FlightSQL server to reject the request with:

```
flightsql: rpc error: code = Internal desc = header key "x-user-name" contains value with non-printable ASCII characters
```

This error occurs at the gRPC/tonic transport layer before the request reaches the FlightSqlServiceImpl. The error is not logged, and user attribution is completely lost.

## Root Cause Analysis

1. **Grafana Plugin** (`grafana/pkg/flightsql/query_data.go:38`): Sets `x-user-name` header directly from `user.Name` without encoding
2. **gRPC/HTTP2**: Metadata headers must be ASCII-safe; UTF-8 characters like `é` are rejected
3. **Tonic**: Validates header values and rejects non-ASCII bytes
4. **User names** commonly contain UTF-8 characters (accents: José, Müller, 田中, etc.)

## Two Issues to Fix

### Issue 1: FlightSQL Server Not Resilient (Priority)

The server should not crash on invalid header values. When invalid headers are received:
1. **Log the error** - Currently the error is not logged at all
2. **Extract what we can** - Preserve any ASCII-printable portions of the value for attribution
3. **Continue with query** - The query should still execute with partial/degraded attribution

### Issue 2: Grafana Plugin Sends Invalid Headers

The plugin should properly encode UTF-8 header values to support full Unicode user names.

---

## Part 1: FlightSQL Server Resilience

### Current State

- `rust/auth/src/user_attribution.rs`: Uses `.to_str().ok()` which silently drops invalid values
- The error occurs at the tonic transport layer before `validate_and_resolve_user_attribution_grpc()` is called
- The gRPC server rejects the entire request, not just the invalid header
- **No logging** when this happens - the error is completely silent on the server side

### Solution: Percent-Encoding for UTF-8

Use standard percent-encoding (URL encoding) to safely transmit UTF-8 in ASCII headers:
- "José García" → "Jos%C3%A9%20Garc%C3%ADa"
- Server decodes percent-encoded values back to UTF-8
- Simple, standard, widely understood

### Implementation Steps

#### Step 1: Add helper function for header extraction with percent-decoding

**File:** `rust/auth/src/user_attribution.rs`

```rust
use micromegas_tracing::prelude::*;
use percent_encoding::percent_decode_str;

/// Extract header value, decoding percent-encoded UTF-8
/// Best-effort: logs warning and extracts printable chars on failure
fn get_header_string_lossy(metadata: &MetadataMap, key: &str) -> Option<String> {
    let value = metadata.get(key)?;

    match value.to_str() {
        Ok(s) => {
            // Decode percent-encoded UTF-8
            match percent_decode_str(s).decode_utf8() {
                Ok(decoded) => Some(decoded.into_owned()),
                Err(e) => {
                    warn!("Header '{key}' has invalid percent-encoded UTF-8: {e}");
                    Some(s.to_string()) // Use raw value as fallback
                }
            }
        }
        Err(_) => {
            // Header contains non-ASCII bytes - log and extract what we can
            let bytes = value.as_bytes();
            let printable: String = bytes
                .iter()
                .filter(|&&b| b >= 0x20 && b <= 0x7E)
                .map(|&b| b as char)
                .collect();

            warn!(
                "Header '{key}' contains non-ASCII bytes, extracted printable portion: '{printable}'"
            );

            if !printable.is_empty() {
                Some(printable)
            } else {
                None
            }
        }
    }
}
```

**Add dependency** to `rust/auth/Cargo.toml`:
```toml
percent-encoding = "2"
```

#### Step 2: Update `validate_and_resolve_user_attribution_grpc()` to use helper

Replace direct `.get().and_then(|v| v.to_str().ok())` calls with `get_header_string_lossy()`:

```rust
let claimed_user_id = get_header_string_lossy(metadata, "x-user-id");
let claimed_user_email = get_header_string_lossy(metadata, "x-user-email");
let claimed_user_name = get_header_string_lossy(metadata, "x-user-name");
```

#### Step 3: Add x-user-name to attribution logging

Currently `x-user-name` is forwarded but not logged. Add it to the `execute_query` log line in `flight_sql_service_impl.rs`.

#### Step 4: Update tests

**File:** `rust/auth/tests/user_attribution_tests.rs`

- Test ASCII header values (existing)
- Test percent-encoded UTF-8: "Jos%C3%A9%20Garc%C3%ADa" → "José García"
- Test graceful degradation: raw non-ASCII bytes → extract printable portion
- Test unencoded ASCII passes through unchanged

### Files to Modify

- `rust/auth/Cargo.toml` - Add `percent-encoding` dependency
- `rust/auth/src/user_attribution.rs` - Add `get_header_string_lossy()` helper with logging
- `rust/auth/tests/user_attribution_tests.rs` - Add UTF-8 and degradation tests
- `rust/public/src/servers/flight_sql_service_impl.rs` - Add x-user-name to logging

---

## Part 2: Grafana Plugin Fix

### Current State

`grafana/pkg/flightsql/query_data.go:37-38`:
```go
if user.Name != "" {
    requestMd.Set("x-user-name", user.Name) // Generic: works for any client
}
```

The value is passed directly without any validation or encoding. UTF-8 characters like `é` cause gRPC to reject the request.

### Solution: Percent-Encode UTF-8 Values

Use standard percent-encoding (URL encoding) to safely transmit UTF-8:

```go
import "net/url"

if user.Name != "" {
    requestMd.Set("x-user-name", url.PathEscape(user.Name))
}
```

**Why percent-encoding:**
- Standard, well-understood encoding
- Preserves full Unicode: "José García" → "Jos%C3%A9%20Garc%C3%ADa"
- ASCII-safe for HTTP/2 headers
- Server decodes back to original UTF-8

### Implementation Steps

#### Step 1: Add percent-encoding for user attribution headers

**File:** `grafana/pkg/flightsql/query_data.go`

Add import:
```go
import "net/url"
```

Change:
```go
if user.Login != "" {
    requestMd.Set("x-user-id", user.Login)
}
if user.Email != "" {
    requestMd.Set("x-user-email", user.Email)
}
if user.Name != "" {
    requestMd.Set("x-user-name", user.Name)
}
```

To:
```go
if user.Login != "" {
    requestMd.Set("x-user-id", url.PathEscape(user.Login))
}
if user.Email != "" {
    requestMd.Set("x-user-email", url.PathEscape(user.Email))
}
if user.Name != "" {
    requestMd.Set("x-user-name", url.PathEscape(user.Name))
}
```

#### Step 2: Add tests for UTF-8 handling

**File:** `grafana/pkg/flightsql/query_data_test.go` (create if needed)

Test cases:
- ASCII-only user name: "John Smith" → "John%20Smith"
- Latin accents: "José García" → "Jos%C3%A9%20Garc%C3%ADa"
- German umlauts: "Müller" → "M%C3%BCller"
- CJK characters: "田中太郎" → "%E7%94%B0%E4%B8%AD..."
- Empty string → empty string

### Files to Modify

- `grafana/pkg/flightsql/query_data.go` - Add percent-encoding for user attribution headers

---

## Testing Plan

### Server Tests

1. Request with plain ASCII `x-user-name` header works (backward compat)
2. Request with percent-encoded UTF-8 header works:
   - "Jos%C3%A9%20Garc%C3%ADa" → "José García"
   - "M%C3%BCller" → "Müller"
   - "%E7%94%B0%E4%B8%AD" → "田中"
3. Request with invalid (raw non-ASCII) header:
   - **Logs a warning** with the extracted printable portion
   - Extracts printable ASCII characters for attribution
   - Query continues to execute
4. Request with completely non-printable header logs warning and uses appropriate fallback

### Plugin Tests

1. ASCII-only user names are percent-encoded (spaces become %20)
2. UTF-8 user names are properly percent-encoded
3. Empty/nil user names don't cause errors
4. Various Unicode scripts encode correctly: Latin-extended, CJK, Cyrillic, Arabic

### Integration Tests

1. Grafana with UTF-8 user name → FlightSQL server → query executes successfully
2. User attribution logged correctly with full Unicode name preserved (decoded)
3. Legacy client with plain ASCII headers still works (backward compatibility)

---

## Execution Order

1. **Server first**: Deploy server-side changes
   - Add percent-decoding for UTF-8 support
   - Add logging for invalid header values
   - Add graceful degradation (extract printable chars)
   - Backward compatible with existing clients (plain ASCII still works)

2. **Plugin second**: Update Grafana plugin to percent-encode
   - Encode all user attribution headers with `url.PathEscape()`
   - Full UTF-8 user names preserved

## Rollback Plan

- Server changes are backward compatible (decodes percent-encoded OR passes through plain ASCII)
- Plugin can be rolled back independently
- Old clients continue to work with plain ASCII headers

## Summary

| Component | Before | After |
|-----------|--------|-------|
| Server: Invalid headers | Silent rejection | Log warning, extract printable chars |
| Server: UTF-8 support | None | Full via percent-decoding |
| Plugin: UTF-8 names | Causes error | Full support via percent-encoding |
| Backward compat | N/A | Maintained (plain ASCII still works) |
