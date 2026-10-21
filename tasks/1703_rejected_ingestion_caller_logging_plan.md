# Log Caller Details for Rejected Ingestion Requests Plan

GitHub issue: #1703

## Overview

A rejected ingestion request today leaves one line, `[auth_failure] authentication failed with all
providers`, with nothing about who sent it. A client stuck retrying a bad or expired credential
can't be traced to a host, process, or user. This plan adds three things. Every auth rejection log
line gets the caller's network details (client IP, method, path, User-Agent) as structured
properties. The final error lists each provider's rejection reason. A rejected
`/ingestion/insert_process` request gets a bounded, sanitized, rate-limited read of its CBOR body,
so the caller-claimed process identity (`process_id`, `exe`, `username`, `computer`, build
properties) is logged too. The token is never logged. The 401 response body doesn't change, so
the client learns nothing new.

## Current State

- `rust/auth/src/axum.rs:41` `auth_middleware` builds `HttpRequestParts` (with the resolved
  `client_ip`, `rust/auth/src/client_ip.rs:46`), calls `validate_request`, and on a plain rejection
  logs `warn!("[auth_failure] {e}")` (`axum.rs:67`) before returning `AuthError::InvalidToken`
  (401, static body `"Invalid token"`). `ProviderUnavailable` logs at `debug!` and maps to 503.
- `rust/auth/src/multi.rs:83` `MultiAuthProvider::validate_request` tries the providers in order.
  It logs each failure only at `debug!("partial auth failed: {e:?}")`, then bails with the fixed
  message `"authentication failed with all providers"`. Every per-provider reason is lost at the
  default level.
- The production chain (`rust/auth/src/default_provider.rs:71` `compose`) is
  `OidcAuthProvider` → `DbApiKeyAuthProvider`, optionally wrapped in `MembershipProvider`
  (`membership.rs:44`), which propagates the inner error unchanged.
- Provider rejection messages:
  - api_key: `"missing bearer token"`, `"invalid API token"`, `"invalid API token: source IP not
    permitted"` (`api_key.rs:120,166,173`).
  - db_api_key: the same messages (`db_api_key.rs:340,347,466,507`).
  - oidc: `"Invalid JWT format"`, `"Token has expired"`, `"Unknown issuer: {iss}"`, and others
    (`oidc.rs:381-544`).
  - `Unknown issuer` echoes an unverified, attacker-chosen JWT claim, so reasons themselves need
    sanitizing before they reach a log line.
- Four more rejection sites log the same bare error:
  - `rust/public/src/servers/firehose_common.rs:127` `warn!("[firehose auth_failure] {e}")`.
  - `rust/auth/src/tower.rs:159` (gRPC / flight-sql) `warn!("authentication failed: {e}")`.
  - `rust/public/src/servers/tonic_auth_interceptor.rs:32` (`check_auth`, exported for embedders)
    `warn!("authentication failed: {e}")`.
  - `rust/analytics-web-srv/src/auth/handlers.rs:397` (`auth_me`) and `:514`
    (`cookie_auth_middleware`) `warn!("[auth_failure] {e}")`. Out of scope: they're web-app login,
    not ingestion, and `CookieTokenRequestParts` carries no caller network details to log.
- `auth_middleware`'s production callers are `serve_ingestion`
  (`rust/public/src/servers/ingestion.rs:187`) and `rust/object-cache-srv/src/object_cache_srv.rs:207`
  (bare `ApiKeyAuthProvider`, no Multi wrapper). The ingestion one wraps `protected_app` (native routes + OTLP +
  webhook) as the outermost layer, so it runs before `RequestBodyLimitLayer(100 MiB)` and before
  any handler reads the body.
- `/ingestion/insert_process` (`ingestion.rs:66`, route string at `ingestion.rs:125`) receives an
  uncompressed CBOR `ProcessInfo` (`rust/tracing/src/process_info.rs:50`), decoded with
  `ciborium::from_reader` in `WebIngestionService::insert_process`
  (`rust/ingestion/src/web_ingestion_service.rs:641`). The Rust sink sends a `Bytes` body, so
  `Content-Length` is always present (`rust/telemetry-sink/src/http_event_sink.rs:503`).
- Process properties sent by the Unreal sink (`unreal/MicromegasTelemetrySink/Private/HttpEventSink.cpp:519-531`):
  - Build identity: `platform-name`, `build-version`, `engine-version`, `build-config`,
    `build-target`, `branch-name`, `commit`.
  - Command line: `exe_args` and `command-line`. These carry the raw command line, which may
    hold secrets.
- `observability_middleware` (`rust/public/src/servers/axum_utils.rs:25`) already logs one
  `request method=… uri=… client_ip=…` line per request, but nothing ties it to the rejection.
- Log properties: `warn!(key = %value, "msg")` records a tagged log whose dynamic properties land
  in `log_entries.properties` (`rust/tracing/src/macros.rs` `log!` docs). Tests read them back
  through `block.dyn_deps.set_properties(id)`, as `rust/tracing/tests/property_macro_tests.rs:180`
  does. The reusable `micromegas_tracing::test_utils` (`rust/tracing/src/test_utils.rs`) only sets
  up an `InMemorySink`. Each test crate hand-rolls its own log collector
  (`rust/public/tests/auth_observability_tests.rs:43` reads only untagged `LogStringEvent`s).

## Design

### 1. Sanitization helper (auth crate)

New module `rust/auth/src/log_sanitize.rs`:

```rust
pub fn sanitize_for_log(value: &str, max_chars: usize) -> String
```

- Replaces every `char::is_control()` character (CR, LF, TAB, ESC, …) and every bidi
  override/isolate (U+202A–U+202E, U+2066–U+2069) with `?`, so one log line can't pretend to be
  two and terminal-rendered logs can't be visually reordered.
- When the input exceeds `max_chars` characters, truncates to `max_chars - 1` *characters*
  (never splitting a code point) and appends `…`, so the output is never longer than `max_chars`.

The auth middleware and the ingestion body logger both use it.

### 2. Named providers and aggregated reasons

- `AuthProvider` gains a required `fn name(&self) -> &'static str`. Implementations:
  - `OidcAuthProvider` → `"oidc"`
  - `ApiKeyAuthProvider` → `"api_key"`
  - `DbApiKeyAuthProvider` → `"db_api_key"`
  - `MultiAuthProvider` → `"multi"`
  - `MembershipProvider` → `self.inner.name()`
  - every test double → its own label

  It's required rather than defaulted, so the compiler lists every implementation, embedders'
  included (see `CLAUDE.md` Interface stability); an `"unknown"` default would hide unnamed
  providers in the reasons forever.
- `MultiAuthProvider::validate_request` collects `"{name}: {reason}"` for each rejection, where
  `reason` is `sanitize_for_log(&format!("{e:#}"), 160)`. Truncating each reason separately means
  one huge `Unknown issuer: …` can't push the others off the line. The final error becomes:

  ```
  authentication failed with all providers: oidc: Invalid JWT format; db_api_key: invalid API token
  ```

  The existing prefix stays, so saved searches on it keep matching. An empty chain bails with
  `authentication failed with all providers: none configured`. The per-provider
  `debug!("partial auth failed")` stays. `ProviderUnavailable` still wins over plain rejections,
  unchanged. The error stays `anyhow`, because no caller branches on the reasons (`rust/CLAUDE.md`
  anyhow-vs-thiserror rule).

### 3. Enriched rejection log line (auth crate)

In the same `log_sanitize.rs` module:

```rust
pub fn warn_auth_rejection(prefix: &'static str, parts: &dyn RequestParts, err: &anyhow::Error)
```

It emits:

```rust
warn!(
    client_ip = %ip_or_empty,
    method = %method_or_empty,
    path = %sanitized_path,          // RequestParts::uri() is already path-only for HTTP
    user_agent = %sanitized_ua,      // get_header("user-agent"), 256 chars
    "{prefix} {reason}"              // reason = sanitize_for_log(&format!("{err}"), 1024)
);
```

- It never reads `authorization_header()`, `bearer_token()`, or any header but `user-agent`, so
  the token has no path into the line. Only the path is logged, never the query string, which
  matches the `/auth/*` redaction rule in `axum_utils.rs:16`.
- Four call sites switch to it, each keeping its current prefix:
  - `auth_middleware` (`"[auth_failure]"`)
  - `firehose_auth_middleware` (`"[firehose auth_failure]"`)
  - `tower.rs` AuthService (`"authentication failed:"`)
  - `tonic_auth_interceptor.rs` `check_auth` (`"authentication failed:"`)

  For gRPC, `method`/`path` are empty because `GrpcRequestParts` has neither; client IP and the
  gRPC user-agent still identify the caller.
- The `ProviderUnavailable` branches stay at `debug!` with their current text. An outage isn't a
  caller-attribution problem.

### 4. Split `auth_middleware` so ingestion can act on the rejected request

`auth_middleware` currently returns 401 itself, so no outer layer ever gets the rejected request
back. Buffering the body ahead of auth would cost every successful request a full-body buffer ahead of the 100 MiB limit layer. Instead, factor
the body of `auth_middleware` into:

```rust
/// Validates `req`, logs a rejection, strips the spoofable x-auth-* headers and inserts the
/// AuthContext on success.
pub async fn authenticate(provider: &dyn AuthProvider, req: &mut Request) -> Result<(), AuthError>
```

`auth_middleware` becomes `authenticate(...)?; Ok(next.run(req).await)`, with the same behavior
as today. `object-cache-srv` layers `auth_middleware` and so gets the enriched line
automatically, with an unprefixed reason (bare provider, no `name:` prefix).

### 5. Ingestion-specific rejection layer

New module `rust/public/src/servers/ingestion_auth.rs`:

```rust
pub(crate) const INSERT_PROCESS_PATH: &str = "/ingestion/insert_process"; // also used by register_routes

pub struct RejectedProcessLogger { recent: moka::sync::Cache<(Option<IpAddr>, Option<String>), ()> }

pub async fn ingestion_auth_middleware(
    provider: Arc<dyn AuthProvider>,
    logger: Arc<RejectedProcessLogger>,
    mut req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    match authenticate(provider.as_ref(), &mut req).await {
        Ok(()) => Ok(next.run(req).await),
        Err(AuthError::InvalidToken) if req.uri().path() == INSERT_PROCESS_PATH => {
            logger.log_rejected(req).await;
            Err(AuthError::InvalidToken)
        }
        Err(e) => Err(e),
    }
}
```

`serve_ingestion` builds one `RejectedProcessLogger` and layers `ingestion_auth_middleware` in
place of `auth_middleware` (`ingestion.rs:187`). The Firehose routers stay untouched. They never
carry `insert_process`.

`RejectedProcessLogger::log_rejected(req)`:

1. **Gate on Content-Length.** If `Content-Length` is missing, unparseable, or above
   `MAX_INSPECTED_BODY_BYTES` (64 KiB), log `"[auth_failure] insert_process body not inspected"`
   with `client_ip`, `reason = missing_content_length|oversize`, and `content_length`, then
   return. A real `ProcessInfo` is a few KiB.
2. **Bounded read.** `tokio::time::timeout(BODY_READ_TIMEOUT /* 2 s */,
   axum::body::to_bytes(body, MAX_INSPECTED_BODY_BYTES))`. A timeout or read error logs the same "not inspected" line with
   `reason = read_timeout|read_error`.
3. **Lenient minimal decode.** `ciborium::de::from_reader_with_recursion_limit(&bytes[..], 16)` into:

   ```rust
   #[derive(Deserialize, Default)]
   #[serde(default)]
   struct ClaimedProcessInfo {
       process_id: Option<String>, exe: Option<String>, username: Option<String>,
       computer: Option<String>, parent_process_id: Option<String>, start_time: Option<String>,
       properties: HashMap<String, String>,
   }
   ```

   Every field is optional, and unknown fields (`tsc_frequency`, `cpu_brand`, …) are ignored, so a
   partial or older body still yields what it has. This is a separate struct, not `ProcessInfo`,
   because `ProcessInfo`'s strict UUID/chrono deserializers would reject a malformed id outright
   and lose the other fields. A decode failure logs one line,
   `"[auth_failure] insert_process body unparseable"`, with `client_ip`.
4. **Rate limit.** Log only when `recent.entry((client_ip, process_id)).or_insert(()).is_fresh()`.
   - Settings: `max_capacity(10_000)`, `time_to_live(60 s)`.
   - The claims line's key is always `Some(sanitized_process_id)`, using `Some("")` when the body has
     no `process_id`, so it never collides with the `(client_ip, None)` key of steps 1–3. The
     sanitized value also keeps a cache entry bounded.
   - All four log sites (steps 1–3 and the claims line) go through one rate-limit helper that
     checks `recent.entry(...)` before logging. Steps 1–3 key on `(client_ip, None)`. A client stuck in a retry loop produces at most one
     line per minute per (IP, process), on top of the unchanged per-request `[auth_failure]` line.
5. **Sanitized, structured emit.** Every value goes through `sanitize_for_log(_, 128)`. Absent
   values are `""`. The message is static, and every caller-supplied value is a property prefixed
   `claimed_`, so nobody mistakes it for verified identity:

   ```rust
   warn!(
       client_ip = %ip, claimed_process_id = %…, claimed_exe = %…, claimed_username = %…,
       claimed_computer = %…, claimed_parent_process_id = %…, claimed_start_time = %…,
       claimed_platform_name = %…, claimed_build_version = %…, claimed_engine_version = %…,
       claimed_build_config = %…, claimed_build_target = %…, claimed_branch_name = %…,
       claimed_commit = %…,
       "[auth_failure] rejected insert_process caller claims"
   );
   ```

   - Properties use a fixed allowlist (`LOGGED_PROPERTY_KEYS`), which matches the macro's
     compile-time keys.
   - `command-line` and `exe_args` are deliberately excluded because they can hold secrets passed
     on the command line.
   - Everything else in the map is dropped.

```
request ─► observability_middleware ─► ingestion_auth_middleware ──ok──► body limit ─► handler
                                             │ InvalidToken
                                             ├─ warn_auth_rejection  (every rejection)
                                             └─ path == insert_process?
                                                  └─ RejectedProcessLogger (≤64 KiB, ≤2 s,
                                                     lenient CBOR, sanitized, rate-limited) ─► 401
```

### Security / privacy

- Every `claimed_*` value is attacker-controlled. The `claimed_` prefix says so in the data
  itself, not just in docs.
- `claimed_username` / `claimed_computer` are PII of the same kind already stored in `processes`.
  They go only to the ingestion service's own log stream, which should have the same access
  controls as process metadata. The docs say so.
- Extra cost falls only on rejected `insert_process` requests, capped at 64 KiB, 2 s, and one
  10k-entry cache. Waiting up to 2 s on a slow body delays only a 401 that would otherwise be
  sent at once. The authenticated route already reads up to 100 MiB with no timeout.

## Implementation Steps

1. **Sanitizer + rejection helper.** Add `rust/auth/src/log_sanitize.rs` (`sanitize_for_log`,
   `warn_auth_rejection`) and register it in `rust/auth/src/lib.rs`.
2. **Provider names.** Add `fn name(&self) -> &'static str` to `AuthProvider`
   (`rust/auth/src/types.rs`) and implement it in:
   - `oidc.rs`, `api_key.rs`, `db_api_key.rs`, `multi.rs`, `membership.rs`
   - the test doubles in `rust/auth/tests/{axum,membership,tower,multi}_tests.rs` and
     `rust/public/tests/{read_policy_threading,firehose}_tests.rs`
3. **Aggregated reasons.** Rewrite the failure tail of `MultiAuthProvider::validate_request`
   (`rust/auth/src/multi.rs:83`).
4. **`authenticate`.** Factor it out of `auth_middleware` (`rust/auth/src/axum.rs`) and switch its
   rejection branch to `warn_auth_rejection`. Do the same for `firehose_auth_middleware`
   (`rust/public/src/servers/firehose_common.rs:127`), `tower.rs:159`, and `check_auth`
   (`rust/public/src/servers/tonic_auth_interceptor.rs:32`).
5. **Test log collector.** Add `collect_logs(&InMemorySink) -> Vec<CapturedLog { msg: String,
   properties: Vec<(String, String)> }>` to `rust/tracing/src/test_utils.rs`. It calls
   `flush_log_buffer()` first, since blocks only reach the sink once flushed. The message
   comes from `desc.fmt_str` for `LogStaticStrEvent` and `TaggedLogStaticStrEvent`, and from
   `msg` for `LogStringEvent` and `TaggedLogString` (the type of every `warn_auth_rejection`
   line); template events are not covered. Dynamic properties of both tagged types are
   resolved the way `property_macro_tests.rs:180-230` does, via
   `block.dyn_deps.set_properties`. The auth and public tests share it, so
   neither crate grows another copy.
6. **Ingestion layer.** Add `rust/public/src/servers/ingestion_auth.rs` (`INSERT_PROCESS_PATH`,
   `ClaimedProcessInfo`, `RejectedProcessLogger`, `ingestion_auth_middleware`) and register it in
   `servers/mod.rs` as `pub mod ingestion_auth;` (integration tests reach it via
   `micromegas::servers::ingestion_auth`). `RejectedProcessLogger` gets a `pub fn new() -> Self`
   building the cache with `max_capacity(10_000)` and `time_to_live(60 s)`. Wire it into
   `serve_ingestion` with a closure cloning both `provider` and `logger` into
   `ingestion_auth_middleware`, and use `INSERT_PROCESS_PATH` in `register_routes`
   (`ingestion.rs:125`).
7. **Dependencies.** Add `ciborium` and `moka` (`features = ["sync"]`) as optional `server`-feature
   dependencies of `rust/public/Cargo.toml`. Both are already workspace dependencies. Keep the
   list alphabetical, and add `"dep:ciborium"` and `"dep:moka"` to the `server` feature list. Add `features = ["test-util"]` to the tokio dev-dependency (needed by
   `start_paused`). Add a `[[test]] name = "ingestion_auth_tests"` entry with
   `required-features = ["server"]`, as `auth_observability_tests` and `firehose_tests` have
   (`default = []`).
8. **Sink User-Agent.** In `rust/telemetry-sink/src/http_event_sink.rs` (`run`), add
   `.user_agent(concat!("micromegas-telemetry-sink/", env!("CARGO_PKG_VERSION")))` to the
   `reqwest::Client` builder, so Rust clients' rejection lines carry a non-empty `user_agent`.
9. Write the tests (see Testing Strategy), the docs, and the CHANGELOG entry.

## Files to Modify

- `rust/auth/src/log_sanitize.rs` (new), `rust/auth/src/lib.rs`
- `rust/auth/src/types.rs`, `oidc.rs`, `api_key.rs`, `db_api_key.rs`, `multi.rs`, `membership.rs`
- `rust/auth/src/axum.rs`, `rust/auth/src/tower.rs`
- `rust/auth/tests/axum_tests.rs`, `multi_tests.rs`, `membership_tests.rs`, `tower_tests.rs`,
  `log_sanitize_tests.rs` (new)
- `rust/tracing/src/test_utils.rs`
- `rust/public/src/servers/ingestion_auth.rs` (new), `servers/mod.rs`, `servers/ingestion.rs`,
  `servers/firehose_common.rs`, `servers/tonic_auth_interceptor.rs`
- `rust/public/Cargo.toml` (optional deps, tokio `test-util` dev feature, `[[test]]` entry)
- `rust/public/tests/ingestion_auth_tests.rs` (new), `read_policy_threading_tests.rs`,
  `firehose_tests.rs`
- `rust/telemetry-sink/src/http_event_sink.rs`
- `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/admin/ingestion.md`
- `CHANGELOG.md`

## Trade-offs

- **Hook inside the generic middleware vs. an `authenticate` split.** One option was a callback
  parameter on `auth_middleware` (`on_reject: Option<Fn(Request)>`). That puts an
  ingestion-shaped concept into the auth crate. Splitting out `authenticate` keeps the auth crate
  generic, and the ingestion layer composes it. Any service can do the same later (open/closed).
- **Reading the body in a spawned task after responding.** It doesn't delay the 401, but it
  holds a connection-bound body past the response and adds a detached task whose failures are
  invisible. The 2 s inline cap is simpler and only slows down rejected callers.
- **Message interpolation vs. properties.** Existing lines interpolate `key=value`. For the new
  caller fields, properties keep attacker text out of the message (no fake `key=` injection), and
  they're queryable directly from `log_entries.properties`. The rejection *reason* stays in the
  message so the existing `[auth_failure] authentication failed with all providers` text still
  matches.
- **No rejection counter or request span.** Both are optional in the issue; count `[auth_failure]` rows in `log_entries` instead.
- **Full `properties` map vs. allowlist.** The full map would include `command-line`. Allowlist.
- **`insert_stream` / `insert_block`.** They could log a claimed `process_id` the same way. Left
  out, as the issue suggests. `insert_process` is the first call any client makes, so a client
  rejected there is already attributed.

## Documentation

- `mkdocs/docs/admin/authentication.md` → Troubleshooting → *Authentication Failures*:
  - Describe the rejection line's properties (`client_ip`, `method`, `path`, `user_agent`) and the
    per-provider reason format.
  - Give an example query, e.g.
    `SELECT time, msg, properties FROM log_entries WHERE msg LIKE '[auth_failure]%'`.
  - Describe the `insert_process` claims line: its `claimed_*` fields are unverified, it's
    rate-limited per client IP + process id per minute, and username/computer are PII.
- `mkdocs/docs/admin/ingestion.md` → *Authentication*: a short paragraph linking to the above.
- `CHANGELOG.md` → new *Unreleased* section. **Minor breaking change:** `AuthProvider` gains a
  required `name()`. `micromegas_auth::axum::authenticate` is new. `object-cache-srv`'s rejection
  line gains the same properties, with an unprefixed reason.
  The Rust telemetry sink now sends `User-Agent: micromegas-telemetry-sink/<version>`.

## Testing Strategy

All tests are no-DB unit tests built on a real `Router` + middleware + `InMemorySink`, using the
`collect_logs` helper from step 5. Tests that capture logs call
`set_max_level(LevelFilter::Trace)` (see `auth_observability_tests.rs:35`). In every test binary
that gains log capture (`axum_tests`, `tower_tests`, `firehose_tests`, `ingestion_auth_tests`),
every test that can emit logs is `#[serial]`, and assertions select their line by a unique marker
such as the `X-Forwarded-For` IP.

Every `ingestion_auth_tests` test except the `missing_content_length` one sets `Content-Length`
explicitly, since `oneshot` bypasses hyper and adds no headers; for the streaming
`read_error`/`read_timeout` bodies the declared length is ≤ 64 KiB.

`rust/auth/tests/log_sanitize_tests.rs`:
- CR/LF/ESC and bidi overrides are replaced.
- Truncation never splits a multi-byte character and appends `…`.
- A clean, short value passes through unchanged.

`rust/auth/tests/multi_tests.rs`:
- Two rejecting providers → the error lists `a: …; b: …` in chain order.
- An oversized first reason is truncated and the second reason still appears.
- An empty chain → `none configured`.
- An unavailable + rejecting mix still returns the `ProviderUnavailable` error (existing
  behavior kept).

`rust/auth/tests/axum_tests.rs`:
- A rejected request with `X-Forwarded-For`, a query string, and a `User-Agent` carrying `\r\n`:
  - the `[auth_failure]` log has `client_ip`, `method`, `path` (no query), and a sanitized
    `user_agent`
  - the bearer token appears in no message or property
  - the response is still 401 `"Invalid token"`
- A successful request logs no `[auth_failure]` line.

`rust/auth/tests/tower_tests.rs`: a rejected gRPC call logs `client_ip` and `user_agent`.

`rust/public/tests/ingestion_auth_tests.rs` (Router with `ingestion_auth_middleware` and a stub
`insert_process` route that asserts it was never reached on rejection):
- A valid CBOR `ProcessInfo` (encoded with `ciborium` from a real `ProcessInfo`) → the claims line
  has every `claimed_*` field. `command-line` / `exe_args` values appear in no property.
- Log-injection payloads (`exe` with `\n[auth_success] …`, bidi chars, 10 KB `username`) → the
  values come out sanitized and ≤128 chars, and exactly one claims line is produced (the injected text creates no extra line).
- Missing `Content-Length` → the `not inspected` line with `missing_content_length`, and the
  body isn't read.
- `Content-Length` > 64 KiB → `oversize`.
- A body stream that yields an error (`Body::from_stream` over a stream returning an `Err`) →
  `read_error`.
- A body stream that never finishes, under `#[tokio::test(start_paused = true)]` → `read_timeout`.
  Paused time advances deterministically, so there's no real sleep.
- Truncated CBOR and non-CBOR bytes → one `unparseable` line.
- The same `(ip, process_id)` twice → one claims line. A different `process_id` → a second line.
- A partial body (only `process_id` + `exe`) → those two are filled and the rest are `""`.
- An authenticated `insert_process` → the request reaches the handler with its body intact, and no
  claims line is logged.
- A rejected `/ingestion/insert_stream` → only the generic `[auth_failure]` line.

`firehose_tests.rs`: `wrong_access_key_is_rejected_with_firehose_error_shape` gains in-memory log
capture via `collect_logs`, `#[serial]`, `set_max_level`, and an `X-Forwarded-For` header, and
asserts the `client_ip` property.

No live-DB tests. Nothing here fixes a bug seen in the wild that only a live DB reproduces.

## Decisions

- The Rust telemetry sink sets `User-Agent: micromegas-telemetry-sink/<version>` (step 8).
