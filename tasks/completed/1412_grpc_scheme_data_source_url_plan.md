# Issue #1412: Accept grpc:// Scheme for Data Source URLs Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1412

## Overview

`validate_data_source_config` (`rust/analytics-web-srv/src/app_db/models.rs:126-145`) rejects any
data source URL that doesn't start with `http://` or `https://`, even though the validated URL is
only ever used as a FlightSQL/gRPC endpoint. This is inconsistent with the rest of the codebase,
where `grpc://`/`grpc+tls://` is the standard convention for gRPC endpoints (Python client default
URI, `_normalize_uri`, the CLI named-profiles plan). This plan relaxes the validator to accept
`grpc://` and `grpc+tls://` (in addition to `http://`/`https://`), and fixes the one place that
consumes the validated URL — `BearerFlightSQLClientFactory::make_client()` — so a `grpc+tls://` URL
actually gets a TLS-enabled channel instead of silently connecting without TLS.

## Current State

- `validate_data_source_config` (`rust/analytics-web-srv/src/app_db/models.rs:126-145`) lower-cases
  the URL and rejects it unless it starts with `http://` or `https://`.
- The regression test locking this in is
  `rust/analytics-web-srv/tests/data_source_tests.rs::test_non_http_scheme_rejected`, which asserts
  `grpc://localhost:50051` is rejected. `test_ftp_scheme_rejected` and `test_no_scheme_rejected`
  cover other invalid schemes and must keep failing.
- The validated URL flows: `stream_query.rs:231` (`let flightsql_url = data_source_config.url`) →
  `BearerFlightSQLClientFactory::new_with_client_type` (`stream_query.rs:244-248`) →
  `BearerFlightSQLClientFactory::make_client()`
  (`rust/public/src/client/flightsql_client_factory.rs:77-115`), which parses the URL as an
  `http::Uri`, builds a `tonic::transport::Channel`, and decides whether to enable TLS with:
  ```rust
  if flight_url.scheme_str() == Some("https") {
      // ClientTlsConfig::new().with_native_roots()
  }
  ```
- `BearerFlightSQLClientFactory` has exactly one call site in the whole workspace
  (`stream_query.rs:244`), so changes to its scheme handling are self-contained to this data-source
  flow and cannot affect the CLI, `http-gateway`, or other FlightSQL clients.
- Tonic's channel connector disables hyper's scheme enforcement
  (`HttpConnector::enforce_http(false)`, see `tonic-0.14.6/src/transport/channel/endpoint.rs`), so a
  `grpc://host:port` URI would actually connect at the TCP level. But the TLS decision quoted above
  only matches the literal string `"https"` — a `grpc+tls://` URL would connect **without TLS** if
  the validator started accepting it without also updating this check. That's the one real risk in
  this change, not just a validation nicety.
- The Python client already does the mirror-image translation for Arrow Flight's own URI scheme
  convention: `FlightSQLClient._normalize_uri` (`python/micromegas/micromegas/flightsql/client.py:236-248`)
  maps `https://` → `grpc+tls://` and `http://` → `grpc://` before handing the URI to
  `pyarrow.flight.connect`. The Rust side needs the same idea in the opposite direction: map
  `grpc://`/`grpc+tls://` → `http://`/`https://` before constructing the `http::Uri`/tonic
  `Channel`, so the existing `scheme_str() == Some("https")` TLS check keeps working unchanged and
  the outgoing HTTP/2 request's `:scheme` pseudo-header stays a normal `http`/`https` value instead
  of depending on server leniency toward non-standard schemes.
- The web app's data source form (`analytics-web-app/src/routes/DataSourcesPage.tsx:205`) nudges
  users toward `https://` via its placeholder text (`"https://flight-sql.example.com:443"`),
  reinforcing the mismatch. There is no client-side scheme validation in that file — the URL is
  passed straight through to the API.

## Design

### Validator: accept grpc:// and grpc+tls:// as well as http:// and https://

In `validate_data_source_config`, replace the two-way `starts_with` check with a check against all
four accepted schemes (case-insensitive, already handled by the existing `to_lowercase()`):

```rust
const ACCEPTED_URL_SCHEMES: &[&str] = &["http://", "https://", "grpc://", "grpc+tls://"];
...
if !ACCEPTED_URL_SCHEMES.iter().any(|scheme| url_lower.starts_with(scheme)) {
    return Err(ValidationError::new(
        "INVALID_URL",
        "URL must start with grpc://, grpc+tls://, http://, or https://",
    ));
}
```

The URL is stored as-is (whatever scheme the user typed) — no rewriting happens at validation time.
`grpc+tls://` must be checked as its own prefix; it is not implied by `grpc://`.

### Factory: normalize the scheme before building the tonic channel

In `BearerFlightSQLClientFactory::make_client()`
(`rust/public/src/client/flightsql_client_factory.rs`), add a small pure helper that rewrites the
gRPC-style scheme to its HTTP-style equivalent before parsing the URL into an `http::Uri`:

```rust
/// Rewrites the `grpc://`/`grpc+tls://` scheme convention used by data source configs into the
/// `http://`/`https://` scheme tonic's `Channel` expects for its TLS decision. `http://`/`https://`
/// URLs pass through unchanged.
pub fn normalize_channel_scheme(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("grpc+tls://") {
        format!("https://{}", &url["grpc+tls://".len()..])
    } else if lower.starts_with("grpc://") {
        format!("http://{}", &url["grpc://".len()..])
    } else {
        url.to_string()
    }
}
```

`make_client()` calls `normalize_channel_scheme(&self.url)` before `.parse::<Uri>()`; the existing
`flight_url.scheme_str() == Some("https")` TLS check is untouched, since by the time it runs the
scheme is always `http` or `https`. The match is case-insensitive (`validate_data_source_config`
accepts any case but stores the URL as typed): the prefix check runs against a lowercased copy,
while the rewrite slices the *original* string's tail so host/port casing (relevant for punycode
domains, unusual but possible) is preserved.

This keeps the change surgical: one helper, one call site, no change to the TLS `if` condition, no
change to any other `BearerFlightSQLClientFactory` caller (there is only one).

### Web app placeholder

Update the placeholder in `analytics-web-app/src/routes/DataSourcesPage.tsx:205` from
`"https://flight-sql.example.com:443"` to `"grpc+tls://flight-sql.example.com:50051"` — the
TLS-enabled scheme, since the previous `https://` example implied TLS and a plaintext `grpc://`
example against a remote-looking host would model bad practice. No other changes needed in that
file — there's no client-side scheme validation to relax.

## Implementation Steps

1. **`rust/analytics-web-srv/src/app_db/models.rs`**: replace the `http://`/`https://` check in
   `validate_data_source_config` with the four-scheme check described above. Update the error
   message to list all four accepted schemes.
2. **`rust/public/src/client/flightsql_client_factory.rs`**: add the `normalize_channel_scheme`
   helper as a `pub fn` (reachable as `micromegas::client::flightsql_client_factory::normalize_channel_scheme`,
   since the module and `client` are already `pub`) — it needs to be `pub`, not private, so it can
   be unit-tested from `rust/public/tests/` per this crate's convention — and call it in
   `make_client()` before `self.url.parse::<Uri>()`.
3. **`rust/analytics-web-srv/tests/data_source_tests.rs`**:
   - Replace `test_non_http_scheme_rejected` (`grpc://` is no longer rejected) with
     `test_valid_grpc_url` and `test_valid_grpc_tls_url`, mirroring `test_valid_http_url`/
     `test_valid_https_url` — assert `Ok` and that the URL round-trips unchanged.
   - Extend `test_case_insensitive_scheme` to also cover `GRPC://` and `GRPC+TLS://`.
   - Keep `test_ftp_scheme_rejected` and `test_no_scheme_rejected` as-is — still invalid.
4. **New test file `rust/public/tests/flightsql_client_factory_scheme_tests.rs`** (no existing test
   file covers this module): call `micromegas::client::flightsql_client_factory::normalize_channel_scheme`
   directly, per Testing Strategy below — this works because the helper is `pub` (step 2).
5. **`analytics-web-app/src/routes/DataSourcesPage.tsx`**: update the URL input placeholder to
   `"grpc+tls://flight-sql.example.com:50051"`.
6. **`rust/public/Cargo.toml`**: add a `[[test]]` entry for the new test file, matching the existing
   entries for every other file under `rust/public/tests/`:
   ```toml
   [[test]]
   name = "flightsql_client_factory_scheme_tests"
   path = "tests/flightsql_client_factory_scheme_tests.rs"
   required-features = ["server"]
   ```
7. Run `cargo fmt` and `cargo clippy --workspace -- -D warnings` in `rust/` before committing.

## Files to Modify

- `rust/analytics-web-srv/src/app_db/models.rs`
- `rust/analytics-web-srv/tests/data_source_tests.rs`
- `rust/public/src/client/flightsql_client_factory.rs`
- `rust/public/tests/flightsql_client_factory_scheme_tests.rs` (new)
- `rust/public/Cargo.toml` (add `[[test]]` entry for the new test file)
- `analytics-web-app/src/routes/DataSourcesPage.tsx`

## Trade-offs

- **Accept all four schemes rather than replacing http/https outright**: the issue explicitly asks
  to keep `http://`/`https://` working as aliases ("forcing them is not [fine]" to remove) — existing
  stored data sources using `https://` must keep working with no migration.
- **Normalize scheme at the factory call site, not at validation/storage time**: preserves the
  user's chosen scheme in the database (what they typed is what they see when they edit the data
  source later), and keeps the rewrite colocated with the only place that actually cares about it
  (the tonic channel's TLS decision). An alternative — rewriting to canonical `http`/`https` at
  validation time — would silently change what the user typed and offers no benefit since there is
  only one consumer of the URL.
- **Free function scoped to this module, not a shared utility**: `normalize_channel_scheme` has
  exactly one caller in the workspace, so there's no second call site to share logic with;
  introducing a shared scheme-normalization utility now would be speculative.

## Testing Strategy

- `rust/analytics-web-srv/tests/data_source_tests.rs`: cover accept/reject for all four schemes
  (case-insensitive) plus the existing ftp/no-scheme rejection cases, per Implementation Step 3.
- `rust/public/tests/flightsql_client_factory_scheme_tests.rs`: cover `normalize_channel_scheme`'s
  behavior — `grpc://host:port` → `http://host:port`, `grpc+tls://host:port` → `https://host:port`,
  `http://`/`https://` unchanged, and mixed-case input (`GRPC://Host:1234` → `http://Host:1234`,
  preserving host casing) — by calling the `pub` helper directly, per Implementation Step 4.
- Manual check: point a local data source at `grpc://localhost:50051` (the monolith's default
  FlightSQL port) via the web app's data source form and confirm queries succeed.

## Open Questions

- None outstanding.
