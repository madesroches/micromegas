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
- `telemetry-sink` re-exports reqwest (`src/lib.rs:82`, `pub use reqwest::*`) and exposes
  `reqwest::Request` in the public `RequestDecorator` trait, so the bump is visible to downstream crates.
- `Cargo.lock` has `reqwest 0.12.28`, which is also pulled in transitively by `oauth2 5.0.0` and
  `object_store 0.13.2`; those are outside our control.
- `rust/object-cache/src/client.rs:523` has a comment tied to the 0.12.28 behavior of `Response::json()`.

## Design
Change the workspace dependency only; the call sites use stable API (`Client`, `ClientBuilder`,
`Request`, `RequestBuilder`, `header`, `redirect::Policy`, `StatusCode`, `bytes_stream`).

- Version: `0.13`.
- Features: reqwest 0.13 renames `rustls-tls` to `rustls` (now the default TLS backend, using
  aws-lc-rs) and may drop `macos-system-configuration`; `charset`, `http2`, `json` remain opt-in.
  Keep `default-features = false` and map the feature list to the 0.13 names, verified against the
  0.13 `Cargo.toml` rather than assumed.
- TLS crypto provider: the workspace uses ring elsewhere (`tonic` `tls-ring`). If the 0.13 rustls
  feature pulls in aws-lc-rs, prefer the ring-based variant if 0.13 offers one, so the build does not
  gain a second crypto provider and a C toolchain requirement; otherwise accept the default and note it.
- Compile fixes: address whatever `cargo build --all-targets` reports (e.g. renamed/removed methods
  in `ClientBuilder`, or behavior changes in default redirect/timeouts). `Cargo.lock` is regenerated.
- Re-check the `object-cache/src/client.rs:523` comment: if 0.13's `json()` no longer re-wraps body
  errors, reword the comment so it states current behavior without the version pin; the two-step read
  stays either way unless the behavior makes it pointless.

## Implementation Steps
1. Edit `rust/Cargo.toml:86` to `0.13` with the renamed features.
2. `cargo update -p reqwest` / `cargo build --all-targets` from `rust/`; fix compile errors in
   `telemetry-sink`, `auth`, `object-cache`, `public` tests.
3. Review `object-cache/src/client.rs:523` comment against 0.13 behavior.
4. Confirm via `cargo tree -d` that `telemetry-sink`, `auth`, `object-cache` resolve to one reqwest 0.13
   (a residual 0.12 only via `oauth2`/`object_store` is expected).
5. Add CHANGELOG entry with a **Minor breaking change** note (public `RequestDecorator` /
   `micromegas_telemetry_sink::reqwest` re-export now expose reqwest 0.13 types).

## Files to Modify
- `rust/Cargo.toml`, `rust/Cargo.lock`
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

## Open Questions
- Whether reqwest 0.13 offers a ring-backed rustls feature (resolved when reading its Cargo.toml in step 1).
