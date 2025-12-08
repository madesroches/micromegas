# User Attribution Invalid Characters Bug

## Status: IMPLEMENTED

All changes have been implemented and tested. See implementation details below.

---

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

## Solution: Percent-Encoding

Use standard percent-encoding (URL encoding) to safely transmit UTF-8 in ASCII headers:
- "José García" → "Jos%C3%A9%20Garc%C3%ADa"
- Server decodes percent-encoded values back to UTF-8
- Simple, standard, widely understood

---

## Implementation Summary

### Part 1: FlightSQL Server - COMPLETED ✓

#### Files Modified

1. **`rust/auth/src/user_attribution.rs`**
   - Added `UserAttribution` struct with named fields for cleaner API:
     ```rust
     pub struct UserAttribution {
         pub user_id: String,
         pub user_email: String,
         pub user_name: Option<String>,
         pub service_account: Option<String>,
     }
     ```
   - Added `get_header_string_lossy()` helper function that:
     - Decodes percent-encoded UTF-8 values
     - Falls back to raw value if percent-decoding fails
     - Extracts printable ASCII chars from non-ASCII bytes as last resort
     - Logs warnings on invalid headers
   - Updated `validate_and_resolve_user_attribution_grpc()` to:
     - Use `get_header_string_lossy()` for `x-user-id`, `x-user-email`, `x-user-name`
     - Return `UserAttribution` struct instead of tuple

2. **`rust/auth/Cargo.toml`**
   - `percent-encoding = "2.3"` dependency already present

3. **`rust/public/src/servers/flight_sql_service_impl.rs`**
   - Updated to use `UserAttribution` struct
   - Added `name={user_name_display:?}` to `execute_query` log line

4. **`rust/auth/tests/user_attribution_tests.rs`**
   - Updated existing tests to use `UserAttribution` struct
   - Added 6 new UTF-8 tests:
     - `test_percent_encoded_utf8_user_name` - Spanish: José García
     - `test_percent_encoded_german_umlaut` - German: Müller
     - `test_percent_encoded_cjk` - Japanese: 田中
     - `test_plain_ascii_user_name` - ASCII passthrough
     - `test_user_name_with_oidc` - UTF-8 with OIDC auth
     - `test_user_name_with_delegation` - UTF-8 with service account delegation

### Part 2: Grafana Plugin - COMPLETED ✓

#### Files Modified

1. **`grafana/pkg/flightsql/query_data.go`**
   - Added `"net/url"` import
   - Changed user attribution headers to use `url.PathEscape()`:
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

---

## Testing

All tests pass:
- `cargo test -p micromegas-auth` - 14 user attribution tests including 6 UTF-8 tests
- `cargo clippy -p micromegas-auth -- -D warnings` - No warnings
- `cargo build -p micromegas` - Builds successfully
- `yarn build` (grafana/) - Builds successfully

---

## Backward Compatibility

- **Server**: Decodes percent-encoded values OR passes through plain ASCII unchanged
- **Plugin**: Can be rolled back independently
- **Old clients**: Continue to work with plain ASCII headers

## Summary

| Component | Before | After |
|-----------|--------|-------|
| Server: Invalid headers | Silent rejection | Log warning, extract printable chars |
| Server: UTF-8 support | None | Full via percent-decoding |
| Server: Return type | Complex tuple | Clean `UserAttribution` struct |
| Plugin: UTF-8 names | Causes error | Full support via percent-encoding |
| Backward compat | N/A | Maintained (plain ASCII still works) |
