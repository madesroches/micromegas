# Single Warning per Rejected OTLP Request Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1739

## Overview
A rejected OTLP request currently logs two `[auth_failure]` warnings: the generic provider-failure
line from the auth layer, then `rejected otlp caller claims` from the ingestion server. Emit one
warning per rejected OTLP request that carries both the rejection reason and the claimed sender
identity, so no information is lost and log volume is halved.

## Current State
- `rust/auth/src/axum.rs:59` `authenticate` logs the generic line via `warn_auth_rejection`
  (`rust/auth/src/log_sanitize.rs:42`) as soon as the provider rejects, before the body is read.
- `rust/public/src/servers/ingestion_auth.rs` `ingestion_auth_middleware` calls `authenticate`,
  then on `InvalidToken` hands the request to `RejectedRequestLogger::log_rejected_otlp`, which
  reads the body prefix and logs a second line (`rejected otlp caller claims`), or a
  `body not inspected` line, throttled to one per client IP and claimed sender per minute.
- The two lines alternate, so consecutive-only repeat collapsing in the log view never fires.
- `insert_process` rejections have the same two-line shape. The issue scopes this to OTLP; they are
  left alone (see Decisions).

## Design
The reason and the claims must end up on one line, and that line must still be emitted for every
rejected request (the generic line is not throttled today, the claims line is).

1. **auth crate**: split `authenticate` into a quiet core that returns the rejection data instead
   of logging it, and keep `authenticate` as a thin wrapper that logs through
   `warn_auth_rejection`, so `auth_middleware`, firehose and all other callers behave exactly as
   before. The quiet variant's invalid-token error carries the request parts needed for logging
   (client ip, method, path, user agent) and the provider error. Provider-unavailable keeps its
   debug-level handling.
2. **ingestion middleware**: for OTLP paths call the quiet core. On invalid token, pass the
   rejection data to `log_rejected_otlp`, which emits exactly one `warn!` per request:
   - the same base properties as `warn_auth_rejection` (`client_ip`, `method`, `path`,
     `user_agent`) with the sanitized provider reason as the message (`[auth_failure] <reason>`),
     so existing queries on that prefix keep working;
   - plus the `claimed_*`, `content_*` properties when the sender is fresh in the per-minute
     window; a repeat from the same sender within the window still logs the line, without the
     claim properties, so every rejection stays visible and counted;
   - plus an `inspection` property carrying the existing not-inspected reason
     (`unsupported_encoding`, `busy`, `no_resource`, ...) when claims could not be extracted, which
     replaces the separate `body not inspected` line for OTLP.
   Non-OTLP paths (and `insert_process`) keep using `authenticate` unchanged.
3. Share the base-property sanitization between `warn_auth_rejection` and the new line (a helper
   in `log_sanitize.rs` returning the sanitized fields) rather than duplicating it.

## Implementation Steps
1. `rust/auth/src/log_sanitize.rs`: extract a helper producing the sanitized rejection fields;
   make `warn_auth_rejection` use it.
2. `rust/auth/src/axum.rs`: add the quiet authentication function and rejection type; make
   `authenticate` wrap it.
3. `rust/public/src/servers/ingestion_auth.rs`: use the quiet core for OTLP paths and fold the
   reason into the single OTLP warning as described; drop the separate OTLP not-inspected line.
4. Update `rust/public/tests/ingestion_auth_tests.rs` (see Testing Strategy).
5. Update `mkdocs/docs/admin/authentication.md` and `CHANGELOG.md`.

## Files to Modify
- `rust/auth/src/log_sanitize.rs`
- `rust/auth/src/axum.rs`
- `rust/public/src/servers/ingestion_auth.rs`
- `rust/public/tests/ingestion_auth_tests.rs`
- `mkdocs/docs/admin/authentication.md`
- `CHANGELOG.md`

## Trade-offs
- **Drop the claims line, keep the generic one**: loses the sender identity that the claims line
  was added to provide.
- **Drop the generic line, keep the claims line**: loses the provider reason and, because the
  claims line is throttled, the per-request count. Rejected in favor of folding.
- **Read the body before logging, for every request**: bounded by the existing semaphore and
  timeout, and the fold design needs it anyway; the throttle only decides whether claim
  properties are attached.

## Decisions
- `insert_process` rejections keep their two lines; the issue is about OTLP and changing them is
  a separate, equally small follow-up.

## Documentation
`mkdocs/docs/admin/authentication.md` (the "Finding the caller of a rejected request" section,
~lines 636-690): the OTLP paragraph describes the claims as a separate
`rejected otlp caller claims` line and a separate `body not inspected` line. Rewrite it to say the
claims and `inspection` reason are properties of the single rejection line. Add a `CHANGELOG.md`
entry.

## Testing Strategy
All no-DB tests in `rust/public/tests/ingestion_auth_tests.rs`, which already drive the
middleware with a rejecting provider and capture logs:
- A rejected OTLP request produces exactly one warning for its client IP, carrying the provider
  reason as message and the `claimed_*` properties (replaces the current
  `lines_for(..., "rejected otlp caller claims")` plus "two lines" assertions around lines 623-800).
- A second request from the same sender inside the throttle window still logs one line, without
  claim properties.
- A request whose body can't be inspected logs one line with the `inspection` reason.
- Unchanged: `insert_stream` and `insert_process` still behave as before; `auth` crate tests for
  `authenticate` and `warn_auth_rejection` keep passing.

## Open Questions
None.
