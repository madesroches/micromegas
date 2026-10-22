# reqwest 0.13 Upgrade Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1706

## Overview
Bump the workspace `reqwest` dependency from 0.12 to 0.13 so binaries embedding
`micromegas-telemetry-sink` alongside crates already on 0.13 (e.g. rig 0.45) compile a single
reqwest/hyper/rustls stack instead of two.

## Current State
- `rust/Cargo.toml:86` declares the workspace dep:
  `reqwest = { version = "0.12.23", default-features = false, features = ["charset", "http2", "json", "macos-system-configuration", "rustls-tls"] }`.
- Consumers (all `reqwest.workspace = true`): `rust/telemetry-sink`, `rust/auth`, `rust/object-cache`,
  and `rust/public` (dev-dependency, used by `tests/graceful_shutdown_tests.rs`).
  `rust/analytics-web-srv` does not declare reqwest; it uses `.form()` (`src/auth/handlers.rs:160,275`) on
  the reqwest 0.13 it receives via the `micromegas` re-export and feature unification, so it is affected by the feature list.
- `telemetry-sink` re-exports reqwest (`src/lib.rs:82`, `pub use reqwest::*`) and exposes
  `reqwest::Request` in the public `RequestDecorator` trait, so the bump is visible to downstream crates.
- `Cargo.lock` has `reqwest 0.12.28`, which is also pulled in transitively by `oauth2 5.0.0` and
  `object_store 0.13.2`; those are outside our control.
- `rust/object-cache/src/client.rs:523` has a comment tied to the 0.12.28 behavior of `Response::json()`.

## Design
Change the workspace dependency only; the call sites use stable API (`Client`, `ClientBuilder`,
`Request`, `RequestBuilder`, `header`, `redirect::Policy`, `StatusCode`). `bytes_stream()`
(`object-cache/src/client.rs:345,418,610`) needs feature `stream`; `.form()`
(`telemetry-sink/src/oidc_client_credentials_decorator.rs:117`, `analytics-web-srv/src/auth/handlers.rs:160,275`)
needs feature `form`.

- Version: `0.13`.
- Features: `default-features = false` with `charset`, `http2`, `json`, `form`, `stream`, `system-proxy`, `rustls`.
  `rustls` replaces `rustls-tls` (aws-lc-rs backed; 0.13 has no ring feature). `macos-system-configuration`
  no longer exists; its replacement is `system-proxy` (in the default set, so it must be listed explicitly).
  Do not use `rustls-no-provider`.
- Compile fixes: address whatever `cargo build --all-targets` reports (e.g. renamed/removed methods
  in `ClientBuilder`, or behavior changes in default redirect/timeouts). `Cargo.lock` is regenerated.
- Re-check the `object-cache/src/client.rs:523` comment: if 0.13's `json()` no longer re-wraps body
  errors, reword the comment so it states current behavior without the version pin; the two-step read
  stays either way unless the behavior makes it pointless.

## Implementation Steps
1. Edit `rust/Cargo.toml:86` to `0.13` with the feature list from Design.
2. `cargo update -p reqwest` / `cargo build --all-targets` from `rust/`; fix compile errors in
   `telemetry-sink`, `auth`, `object-cache`, `public` tests.
3. Review `object-cache/src/client.rs:523` comment against 0.13 behavior.
4. Confirm via `cargo tree -d` that `telemetry-sink`, `auth`, `object-cache` resolve to one reqwest 0.13
   (a residual 0.12 only via `oauth2`/`object_store` is expected). `cargo tree -d` will also show two
   crypto providers (ring via tonic, aws-lc-rs via reqwest); this is accepted (see Decisions).
5. Add CHANGELOG entry with a **Minor breaking change** note (public `RequestDecorator` /
   `micromegas_telemetry_sink::reqwest` re-export now expose reqwest 0.13 types).

## Files to Modify
- `rust/Cargo.toml`, `rust/Cargo.lock`
- `rust/analytics-web-srv` (no manifest change expected; verify it compiles)
- `rust/object-cache/src/client.rs` (comment, possibly)
- Any call site the compiler flags
- `CHANGELOG.md`

## Trade-offs
- Staying on 0.12 keeps downstream double-compiling two stacks — the problem being solved.
- Waiting for `oauth2`/`object_store` to move to 0.13 was rejected: the issue targets our own crate's
  dependency and those are independent upstreams.

## Documentation
No `mkdocs/` pages reference the reqwest version; none to update beyond the CHANGELOG.

## Testing Strategy
- `cargo build --all-targets`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`.
- `cargo test` for `micromegas-telemetry-sink`, `micromegas-auth`, `micromegas-object-cache`, and
  `micromegas` (graceful shutdown tests). Existing unit tests (e.g. `api_key_decorator_tests.rs`)
  cover the `Request`/header surface; no new tests are needed for a dependency bump.

## Decisions
- TLS uses reqwest's `rustls` feature (aws-lc-rs); Cargo.lock gains aws-lc-rs alongside tonic's ring. The two-crypto-provider outcome is accepted.
