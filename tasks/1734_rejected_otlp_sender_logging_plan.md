# Identify the Sender of Rejected OTLP Requests Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1734

## Overview

A rejected OTLP/HTTP request leaves one `[auth_failure]` line with `client_ip`, `method`, `path` and
`user_agent`. Behind a shared egress NAT that doesn't say which application is misconfigured. This plan
extends the rejected-`insert_process` logging to the three OTLP routes: on a rejection, read a bounded
prefix of the (possibly gzipped) body, pull the OTLP `Resource` identity out of it (service, host,
SDK, deployment, Kubernetes workload) and log it as `claimed_*` properties, alongside a few
request headers. The extra read is bounded, rate limited, best-effort and runs only after the
credential has already been refused. The 401 response and the existing `[auth_failure]` line don't change.

## Current State

- `ingestion_auth_middleware` (`rust/public/src/servers/ingestion_auth.rs:~205`) wraps `protected_app`
  as the outermost layer (`rust/public/src/servers/ingestion.rs:~185`). On `AuthError::InvalidToken`
  for `INSERT_PROCESS_PATH` only, it hands the rejected request to
  `RejectedProcessLogger::log_rejected`, which does a bounded read (64 KiB cap, 2 s timeout,
  `Content-Length` precheck), parses CBOR, and logs `claimed_*` fields after
  `sanitize_for_log`, deduplicated per `(client_ip, process_id)` per minute by a moka cache.
- `authenticate` (`rust/auth/src/axum.rs:56`) already logs `[auth_failure]` through
  `warn_auth_rejection` (`rust/auth/src/log_sanitize.rs`) with network details only.
- OTLP routes (`rust/public/src/servers/otlp.rs:196`) sit **inside** `protected_app`. Their gzip
  decompression layer, 20 MiB limit and 300 MiB decompressed cap
  (`rust/public/src/servers/ingestion_limits.rs`) run after auth, so on rejection the body is the
  raw wire stream: possibly gzip, possibly up to 100 MiB, not yet bounded by the OTLP limits.
- The identity logic for accepted requests is `ProcessFromResource::build`
  (`rust/otel-ingestion/src/block.rs:440`): `exe` = `service.namespace/service.name` (or just
  `service.name`), `computer` = `host.name`. Helpers: `identity::attr` and `identity::attr_to_string`
  (`rust/otel-ingestion/src/identity.rs`). Proto types come from `opentelemetry-proto`
  (`rust/otel-ingestion/src/proto.rs`), which also implement serde for the JSON encoding.
- Encoding is chosen by `Content-Type` (`content_type_encoding`, `otlp.rs:~35`).
- Differences from `insert_process` that matter: OTLP bodies routinely exceed 64 KiB (so the
  "reject if `Content-Length` > cap" rule would skip most real traffic), they may be gzipped, they may
  be JSON, and the Resource is a prefix of the message, not the whole of it.

## Design

### 1. Resource extraction (`otel-ingestion`)

New module `rust/otel-ingestion/src/rejected.rs`, exported from `lib.rs`:

```rust
pub struct ClaimedResource { /* Vec<(&'static str, String)> over a fixed allowlist */ }

pub fn claimed_resource_from_prefix(
    prefix: &[u8],            // already decompressed and size-capped by the caller
    encoding: Encoding,
) -> Option<ClaimedResource>
```

Only the first `Resource` of the first `ResourceLogs`/`ResourceMetrics`/`ResourceSpans` is read.
All three signals' request messages start with `repeated Resource* <field 1>`, and each `Resource*`
message starts with `Resource resource = 1`, so one extractor serves every route. It needs no
signal parameter.

The body may be cut mid-message by the prefix cap, so `prost`'s `Message::decode` (all-or-nothing)
can't be used. Instead:

- **Protobuf**: a small wire-format walker over the `&[u8]`. It reads tag → length-delimited at
  field 1 of the request, field 1 of `Resource*`, then the `Resource.attributes` entries (field 1,
  `KeyValue`). Each *complete* attribute that fits in the prefix is decoded (`KeyValue::decode`)
  and kept; walking stops at the first incomplete tag/length or at the end of the `Resource`.
  Nesting is fixed at three levels, so no recursion limit is needed.
- **JSON**: a `serde_json::Deserializer` over the prefix driven by a custom visitor that pushes
  each attribute into a collector as it is visited and returns a private "done" error once
  `resource.attributes` of the first resource ends. Truncation produces a normal `serde_json`
  error, which is ignored if at least one attribute was collected. Keys are matched
  case-sensitively per the OTLP/JSON mapping (`resourceLogs[0].resource.attributes[]`). Nesting
  is bounded by serde_json's default recursion limit.

Both walkers keep an attribute only if its key is on the allowlist below and its value is a
scalar (`string`, `int`, `bool`, `double`), converted with the existing `attr_to_string`. No other
attribute is retained, so unexpected custom attributes (which could hold secrets) never reach a log.

Allowlist (log property name ← resource attribute):

| Property | Attribute |
|---|---|
| `claimed_exe` | `service.namespace` + `service.name`, composed exactly as `ProcessFromResource::build` |
| `claimed_computer` | `host.name` |
| `claimed_sdk_language` / `claimed_sdk_name` / `claimed_sdk_version` | `telemetry.sdk.language` / `.name` / `.version` |
| `claimed_service_version` | `service.version` |
| `claimed_service_instance_id` | `service.instance.id` |
| `claimed_deployment_environment` | `deployment.environment.name`, falling back to `deployment.environment` |
| `claimed_k8s_namespace` / `claimed_k8s_pod` / `claimed_k8s_deployment` / `claimed_k8s_cluster` | `k8s.namespace.name` / `k8s.pod.name` / `k8s.deployment.name` / `k8s.cluster.name` |
| `claimed_container` | `container.name` |
| `claimed_cloud_region` | `cloud.region` |
| `claimed_pid` / `claimed_runtime` | `process.pid` / `process.runtime.name` + `process.runtime.version` |
| `claimed_username` | same lookup as `identity::process_owner_string` |

Deliberately excluded: `process.command_line`, `process.command_args`,
`process.environment_variable.*` and every other attribute (`insert_process` logging excludes
command lines for the same reason). Payload content (log bodies, span names) is never read.

To keep `exe` composition in one place, the composition currently inline in
`ProcessFromResource::build` moves to a small `identity::exe_from_service(namespace, name)` that both
call (DRY).

### 2. Bounded body prefix (`public`)

New helper in `rust/public/src/servers/ingestion_auth.rs`:

```rust
async fn read_body_prefix(req: Request, content_encoding, cap: usize) -> Result<PrefixRead, ...>
```

- Streams `req.into_body().into_data_stream()` and stops after `MAX_INSPECTED_BODY_BYTES` (64 KiB)
  of **wire** bytes, then drops the rest. This differs from the `insert_process` path, which
  rejects on `Content-Length` > cap: here a prefix is enough, so large and chunked bodies are
  inspected too. A missing `Content-Length` is therefore not an error. The whole read runs under the
  existing `BODY_READ_TIMEOUT` (2 s).
- `Content-Encoding: gzip` (case-insensitive) → `flate2::read::GzDecoder` over the prefix, wrapped
  in `Read::take(MAX_INSPECTED_BODY_BYTES)` so the **decompressed** output is capped too (the
  gzip-bomb bound; the wire prefix is cheap, the expansion is what needs a cap). A decoder error
  after some output (truncated stream) keeps the bytes produced so far. A decoder error before any
  output counts as undecodable. Any other `Content-Encoding` value (`zstd`, `deflate`, …) is
  logged as `reason = "unsupported_encoding"` without reading the body.
- `Content-Type` selects `Encoding`, via `content_type_encoding` made `pub(crate)` in `otlp.rs`
  (an unknown type logs `reason = "unsupported_content_type"` and reads nothing).
- Decompression is synchronous CPU work on at most 64 KiB, so it runs inline; no `spawn_blocking`.

### 3. Logging, dedup and concurrency

`RejectedProcessLogger` becomes `RejectedRequestLogger` (a Rust-only rename, compiler-enumerated)
and gains `log_rejected_otlp(req)`. The middleware's `match` arm selects the method by path:
`INSERT_PROCESS_PATH` → `log_rejected`, `OTLP_PATHS` (the three `/ingestion/otlp/v1/{logs,metrics,traces}`
routes; the Firehose and webhook routes carry their own auth and aren't touched) →
`log_rejected_otlp`. Everything else is unchanged.

`log_rejected_otlp` emits a second line next to the existing `[auth_failure]` one:

```
[auth_failure] rejected otlp caller claims
  client_ip, path, content_type, content_encoding, content_length (headers, sanitized)
  + every non-empty ClaimedResource property
```

Header values go through `sanitize_for_log`. Only `Content-Type`, `Content-Encoding`,
`Content-Length` are read; `Authorization` and every other header are never touched. `user_agent`
is already on the `[auth_failure]` line and isn't repeated. When nothing could be extracted, a
`[auth_failure] otlp body not inspected` line carries `reason` (`unsupported_encoding`,
`unsupported_content_type`, `read_error`, `read_timeout`, `undecodable`, `no_resource`, `busy`) and
the headers, via the shared `log_not_inspected` shape.

Two guards, because a misconfigured exporter retries continuously and a NAT can hide many
different apps behind one IP:

- **Dedup on the identity, not the IP**: the moka cache key is `(client_ip, Some(identity_key))`
  where `identity_key` is `"otlp\x1f{exe}\x1f{computer}\x1f{instance_id}"`. One line per
  distinct sender per minute, so two apps behind one NAT both appear. The `insert_process`
  entries (`Some(process_id)`) and the "not inspected" entries (`None`) can't collide with it.
  The *not inspected* lines use the existing `(client_ip, None)` key.
- **Cap concurrent inspections**: a `tokio::sync::Semaphore` (8 permits) around read+decode.
  `try_acquire` failing logs nothing but `reason = "busy"` under the `(client_ip, None)` key. This bounds the
  total CPU and memory of the unauthenticated path (≤ 8 × 128 KiB buffers) no matter how many
  rejected requests arrive, which dedup alone doesn't (dedup runs after the parse because the key
  comes from the body).

### Alternatives for the log shape

The issue proposes adding properties to the existing `[auth_failure]` line. A separate line is chosen
because the parse must run after `authenticate` has already logged (the existing line is emitted
inside `authenticate`, before the request is handed back), the same split `insert_process` already has,
and it lets the extra line be deduplicated without suppressing the per-request rejection line.

## Implementation Steps

1. `otel-ingestion`: add `identity::exe_from_service`, switch `ProcessFromResource::build` to it.
2. `otel-ingestion`: add `rejected.rs` with the protobuf walker, JSON visitor, allowlist and
   `ClaimedResource`; export from `lib.rs`; unit tests.
3. `public/servers/otlp.rs`: make `content_type_encoding` and the OTLP path constants `pub(crate)`.
4. `public/servers/ingestion_auth.rs`: add `read_body_prefix`, gzip handling, semaphore, rename
   `RejectedProcessLogger` → `RejectedRequestLogger`, add `log_rejected_otlp`, extend the middleware
   `match`.
5. `public/servers/ingestion.rs`: update the renamed type at the construction site.
6. Tests (below), docs, `CHANGELOG.md`.

## Files to Modify

- `rust/otel-ingestion/src/identity.rs`, `src/block.rs`, `src/lib.rs`, new `src/rejected.rs`
- `rust/public/src/servers/ingestion_auth.rs`, `otlp.rs`, `ingestion.rs`
- `rust/public/tests/ingestion_auth_tests.rs`
- `mkdocs/docs/admin/authentication.md`, `CHANGELOG.md`

## Trade-offs

- **Hand-rolled prefix walker vs. `Message::decode`**: `decode` rejects a truncated message, and a
  cap is the whole point. The walker is ~60 lines over a fixed three-level shape; the attribute
  values themselves are still decoded by prost's `KeyValue`.
- **Decode the whole body with a bigger cap**: simpler, but the Resource is always at the front of
  the message, so reading more only adds exposure for an unauthenticated caller.
- **Rate limit per IP** (the issue's suggestion): rejected because it would hide all but one app
  behind a shared NAT, which is the problem being fixed. Dedup by sender identity plus a global
  concurrency cap bounds the cost instead.
- **Log every Resource attribute**: most useful, but attributes can hold secrets (command lines,
  environment variables, custom keys); an allowlist is the same stance `insert_process` logging took.
- **gzip only** for decompression: it's the only encoding the OTLP sub-router accepts
  (`RequestDecompressionLayer::new().gzip(true)`), so anything else would be rejected by the
  accepted path as well.

## Decisions

- `claimed_username` and `claimed_computer` are personal data of the kind already stored in
  `processes`; they are logged here as for `insert_process`, and the docs repeat the access-control note.
- Firehose and webhook routes are out of scope: they have separate auth and carry no OTLP Resource.

## Documentation

`mkdocs/docs/admin/authentication.md` ("Finding the caller of a rejected request"): add the
`[auth_failure] rejected otlp caller claims` line, its properties, the reasons for
`otlp body not inspected`, the once-per-sender-per-minute limit, and the fact that values are
unverified caller claims. `CHANGELOG.md` Unreleased: new log line; **Minor breaking change**: the Rust type
`RejectedProcessLogger` is renamed to `RejectedRequestLogger`.

## Testing Strategy

Unit tests, no DB:

- `rejected.rs`: protobuf and JSON each for logs/metrics/traces; resource with every allowlisted
  attribute; namespace + name composition equals `ProcessFromResource::build`'s `exe`; absent
  `host.name`; scalar-only values (a `kvlist` value is skipped); non-allowlisted attribute (e.g.
  `process.command_line`) never appears; truncation at every byte offset of a sample body returns
  either `None` or a prefix-consistent subset and never panics; garbage input and empty input →
  `None`; control characters in a value survive extraction (sanitization is the logger's job).
- `ingestion_auth` (extending the existing `ingestion_auth_tests.rs` harness, store seam with
  a canned rejecting provider): rejected OTLP request with plain protobuf, plain JSON, gzip
  protobuf, gzip JSON → one `claimed_*` line with the expected properties; truncated gzip stream
  still yields what was decoded; gzip bomb (small wire, huge expansion) is stopped at the cap and
  the test asserts bounded output; body > 64 KiB and chunked (no `Content-Length`) are inspected;
  `Content-Encoding: zstd` and unknown `Content-Type` → `not inspected` line with the right
  `reason`; undecodable body → the existing `[auth_failure]` line plus `not inspected`; two
  different services from the same IP both logged, the same one twice within a minute logged once;
  concurrency cap exhausted → `busy`; `Authorization` value never appears in any captured log
  property; control characters/newlines in `service.name` are sanitized; an accepted request
  produces no extra line and the body reaches the handler untouched.

## Manual Verification

1. Start the stack (`python3 local_test_env/ai_scripts/start_services.py`) with auth enabled and
   no valid key, send an OTLP export with the OpenTelemetry SDK (e.g. `OTEL_SERVICE_NAME=demo`
   `OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:9000/ingestion/otlp`).
2. `micromegas-query "SELECT msg, properties FROM log_entries WHERE msg LIKE '%rejected otlp%'"`.
   Expect `claimed_exe=demo` and the SDK language and version. This checks the real SDK's
   wire format and layer order, which the unit tests build by hand.
