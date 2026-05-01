# OTel Identity Collapse Plan

## Overview

Make the OTLP "degenerate resource" case observable, actionable, and easy to fix from the client side. Real-world OTel SDKs — most notably **Claude Code itself**, the driving use case for the OTLP feature — emit Resource attributes that contain `service.name`/`service.version`/`os.*` but *none* of the four identity-bearing fields (`host.id`, `host.name`, `process.pid`, `service.instance.id`). Our `process_id_from_resource` formula then collapses every session, on every machine, onto a single `process_id`. The formula itself is correct and load-bearing — it cannot change without a `_V2` namespace bump and a data-lake migration. The fix lives at the edges: better client-side guidance, less server-log spam, and operator-side observability.

Goal in one sentence: **keep the identity formula as-is; make the collapse visible, ship a working Claude Code wrapper, and stop warning on every batch.**

## Current State

### Identity formula (load-bearing, do not change)

`rust/otel-ingestion/src/identity.rs:118` synthesizes `process_id` by hashing the lower-cased `host.id`, `host.name`, `process.pid`, `process.creation.time`/`process.start_time`, `service.namespace`, `service.name`, and `service.instance.id` under `NS_OTEL_PROCESS_V1`. The namespace UUID was generated 2026-05-01 and shipped — changing the formula requires `_V2` and a data-lake migration.

### Degenerate-resource detection

`is_degenerate_resource(attrs)` (`identity.rs:109`) returns true when *all four* of `host.id`, `host.name`, `process.pid`, and `service.instance.id` are empty. Called from `build_prepared_block` (`block.rs:195`) which fires a `warn!` *per block*. Block fan-out is one-per-Resource-per-export-request, so for a Claude Code session we observed **42 warnings for 24 emitted records** in five minutes.

### What Claude Code actually emits

From a live capture (`process_id = bfa3ee77-9f8d-54e6-a049-92fd9bc51f18`):

```json
{
  "deployment.environment": "local-otel-test",
  "host.arch": "amd64",
  "os.type": "linux",
  "os.version": "6.6.87.2-microsoft-standard-WSL2",
  "service.name": "claude-code",
  "service.version": "2.1.126",
  "wsl.version": "2"
}
```

None of `host.id`, `host.name`, `process.pid`, `service.instance.id`, `service.namespace`, `process.creation.time` are set. Two different machines, two different users, two different runs all hash to the same `process_id`.

### How docs cover this today

- `mkdocs/docs/otlp/index.md` mentions the limitation in the "Process identity" section ("Missing fields are treated as empty strings…") and in the troubleshooting bullet "Process collapses across runs". The Claude Code recipe (lines 175–204) does *not* mention the identity issue or the workaround.
- `mkdocs/docs/otlp/index.md:202` — `OTEL_RESOURCE_ATTRIBUTES="team.id=...,deployment.environment=prod"` — is presented as an *optional* multi-team-rollup tag, not as load-bearing for identity.

### What works without code change

OTel SDKs (including Claude Code's) honor `OTEL_RESOURCE_ATTRIBUTES` and merge those KV pairs into the Resource. So the client-side fix is one shell line:

```bash
export OTEL_RESOURCE_ATTRIBUTES="host.name=$(hostname),process.pid=$$,service.instance.id=$(uuidgen)"
```

This is what we want users to do; the work in this plan is making that path discoverable, frictionless, and obvious.

## Design

### Principles

1. **Identity formula is frozen.** No `_V2` bump in this work. Adding new identity-bearing fields (or making the hash depend on connection metadata, ingest time, etc.) breaks idempotency and re-reorganizes the data lake.
2. **Push the fix to the client.** OTel's intended escape hatch for missing resource detection is `OTEL_RESOURCE_ATTRIBUTES`. The server's job is to make the failure mode observable and explain how to fix it.
3. **Don't punish good clients.** Warnings should fire once per server-lifetime per collapsed `process_id`, not once per batch.

### Changes (low → high cost)

#### 1. Make the server warning actionable and bounded

Current message:
```
OTLP resource without host.id/host.name/process.pid/service.instance.id —
multiple processes may collapse onto process_id={}
```

Proposed message:
```
OTLP resource missing identity attributes
  service.name = "<name>"
  collapsed process_id = <uuid>
  fix client-side with:
    OTEL_RESOURCE_ATTRIBUTES="host.name=$(hostname),process.pid=$$,service.instance.id=$(uuidgen)"
```

Plus rate-limiting: maintain an in-process `DashSet<Uuid>` of `process_id`s already warned about; fire the warning once per process_id per server-process lifetime. Memory cost is negligible (one UUID per collapsed identity; degenerate-resource cardinality is small by definition).

Code site: `rust/otel-ingestion/src/block.rs:195` (the `warn!` in `build_prepared_block`). The `DashSet` lives in a module-level `OnceLock<DashSet<Uuid>>` or threaded through the handler — preference for a module-level static to avoid plumbing.

`dashmap` is already in the workspace via the analytics crate — confirm and reuse the same pin to keep `cargo machete` clean.

#### 2. Surface a `degenerate_resource_warnings` ingest metric

Increment a `micromegas_metrics::Counter` named `otel_ingest.degenerate_resources` from the same code path. Operators can alert on it via existing `measures` queries — no new infrastructure. Counter creation lives next to the warning.

This is what makes the collapse *visible* without grepping logs. Two-minute change, big payoff for operators.

#### 3. Ship a working Claude Code launcher in `local_test_env/`

Add `local_test_env/claude_code_otel.sh`:

```bash
#!/usr/bin/env bash
# Launches Claude Code with the OTel identity attributes that the SDK
# does not emit on its own. Without these, every Claude Code session
# collapses onto a single process_id at the Micromegas server.
set -euo pipefail

: "${MICROMEGAS_OTEL_ENDPOINT:=http://127.0.0.1:9000/ingestion/otlp}"
: "${MICROMEGAS_API_KEY:=}"

export OTEL_EXPORTER_OTLP_ENDPOINT="${MICROMEGAS_OTEL_ENDPOINT}"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"

# Identity attributes Claude Code's resource detector omits.
# host.name + process.pid + service.instance.id together disambiguate
# every (machine × OS process × invocation).
INSTANCE_ID="$(uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid)"
export OTEL_RESOURCE_ATTRIBUTES="host.name=$(hostname),process.pid=$$,service.instance.id=${INSTANCE_ID}${OTEL_RESOURCE_ATTRIBUTES:+,${OTEL_RESOURCE_ATTRIBUTES}}"

# Telemetry on.
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp

if [[ -n "${MICROMEGAS_API_KEY}" ]]; then
  export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${MICROMEGAS_API_KEY}"
fi

exec claude "$@"
```

Honors caller-set `OTEL_RESOURCE_ATTRIBUTES` by appending. Falls back to `/proc/sys/kernel/random/uuid` when `uuidgen` isn't installed (Alpine / minimal images).

#### 4. Document the workaround prominently

In `mkdocs/docs/otlp/index.md`:

- **Add an explicit "Identity attributes" subsection under the Claude Code recipe** that shows the `OTEL_RESOURCE_ATTRIBUTES` line and explains *why* it's required (Claude Code's resource detector omits the fields the formula keys on). Link back to the wrapper script.
- **Promote the troubleshooting bullet** to its own subsection at the same level as "Authentication" — currently it's the third bullet in a generic troubleshooting list and easy to miss.
- **Show what the warning looks like** in the troubleshooting subsection so users searching for the warning text find it.

#### 5. (Optional, deferred) Operator-side detection query

A starter SQL snippet for the docs:

```sql
-- Suspected collapsed processes: same OTel resource fingerprint, but
-- the process row's identity attrs are all empty. If count(*) > 1 these
-- are likely multiple physical processes hashed onto one process_id.
SELECT process_id,
       jsonb_as_string(jsonb_get(properties, 'otel.resource.service.name')) AS service,
       count(*) OVER (PARTITION BY process_id) AS streams_under_id
FROM processes
WHERE jsonb_as_string(jsonb_get(properties, 'otel.resource.host.name')) IS NULL
  AND jsonb_as_string(jsonb_get(properties, 'otel.resource.host.id'))   IS NULL
  AND jsonb_as_string(jsonb_get(properties, 'otel.resource.process.pid')) IS NULL
  AND jsonb_as_string(jsonb_get(properties, 'otel.resource.service.instance.id')) IS NULL;
```

Goes in `mkdocs/docs/otlp/index.md` troubleshooting. No new server code.

### What we are NOT doing

- **Changing `process_id_from_resource`.** Adding `service.version` / `os.type` / `host.arch` would need `_V2` namespace + a data-lake migration; gain is marginal (still collapses all WSL/x86 Claude sessions on the same OS version onto one id).
- **Fall back to connection peer IP.** Breaks idempotency (a retry through a different LB hashes differently) and is defeated by NAT / load balancers anyway.
- **Auto-synthesize `service.instance.id` server-side.** Same idempotency problem — every retry on the same payload has to produce the same `process_id` for `block_id` deduplication to keep working.
- **Admin command to "split" an already-collapsed process_id.** Splitting requires re-keying every stream and block under the process — high risk, low payoff vs. preventing the collapse in the first place.
- **A new identity formula version.** Out of scope for this work; tracked separately if and when we accumulate enough other reasons to bump.

## Implementation Steps

### Phase 1: warning quality

1. Edit `rust/otel-ingestion/src/block.rs:195` to:
   - Pull `service.name` out of the resource (via existing `attr_norm`) for the message body.
   - Print the actionable `OTEL_RESOURCE_ATTRIBUTES` snippet inline.
   - Rate-limit via a module-level `OnceLock<DashSet<Uuid>>` keyed by `process_id` — first sighting per server-process lifetime warns, the rest are silent.
2. Add `dashmap` as a direct dep on `rust/otel-ingestion/Cargo.toml` if not already pulled in transitively (`cargo tree -p micromegas-otel-ingestion -i dashmap` — if absent, add it; reuse the workspace pin from analytics).
3. Add a unit test in `rust/otel-ingestion/src/block.rs` (or a dedicated `tests/degenerate_warning.rs` if we want it as integration) verifying that calling `build_prepared_block` twice on the same degenerate resource only logs once. Use `tracing-subscriber::fmt::TestWriter` or equivalent; the project already has tracing-test patterns in other crates — match those.

### Phase 2: ingest counter

4. Add a `Counter` for `otel_ingest.degenerate_resources` (the `micromegas_tracing` `imetric!` macro is the existing convention — see `rust/public/src/servers/flight_sql_service_impl.rs:296` for usage). Increment alongside the warning. Counter emission goes through the native telemetry-sink; it lands in `measures` like every other internal metric.
5. Add a one-line entry to the OTLP page documenting the counter.

### Phase 3: client tooling

6. Create `local_test_env/claude_code_otel.sh` with the launcher script above. `chmod +x`. No README required if the OTLP page links to it.
7. (Optional) `local_test_env/ai_scripts/start_services.py` already orchestrates the dev backend; it does not need to change for this plan.

### Phase 4: docs

8. Edit `mkdocs/docs/otlp/index.md`:
   - Insert an "Identity attributes (required for Claude Code)" subsection inside the Claude Code recipe block, before the `claude` invocation. Quote the exact `OTEL_RESOURCE_ATTRIBUTES` line. Cross-link to the wrapper.
   - Add a "Detecting collapsed processes" subsection in Troubleshooting with the SQL snippet from Design §5.
   - Update the existing "Process collapses across runs" bullet to point at the new subsection (one-line stub).
   - Show the warning text the operator will see in the server log so a grep for the warning lands here.
9. CHANGELOG entry under "Unreleased / OTLP Ingestion": "Rate-limit and improve the degenerate-resource warning; add `otel_ingest.degenerate_resources` counter."

### Phase 5: verification

10. Restart `telemetry-ingestion-srv` after the warning change. Run a degenerate POST twice (e.g., copy a captured Claude Code payload, or hand-build a no-host/no-pid request via `python/micromegas/tests/test_otlp_e2e.py` style). Assert exactly one warning lands in `log_entries`.
11. Run the Claude Code wrapper script against a fresh service. Verify two consecutive `claude` invocations produce two distinct `process_id`s and zero `degenerate_resources` warnings.
12. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`, `python3 build/rust_ci.py native`.
13. `cd mkdocs && mkdocs build --strict` (must stay clean per the same-day check we just established).

## Files to Modify

**Modified:**
- `rust/otel-ingestion/src/block.rs` — warning text + rate-limit set + counter increment
- `rust/otel-ingestion/Cargo.toml` — `dashmap` dep if not already present (workspace pin)
- `mkdocs/docs/otlp/index.md` — Claude Code identity-attributes subsection, prominent troubleshooting subsection, warning text reference, detection SQL
- `CHANGELOG.md` — one-line entry under Unreleased / OTLP Ingestion

**New:**
- `local_test_env/claude_code_otel.sh` — wrapper script with the right `OTEL_RESOURCE_ATTRIBUTES`

## Trade-offs

- **Why not bump the formula to `_V2` to include `service.version` + `os.*`?** Even with those added, every Claude Code v2.1.126 session on Linux/x86 still hashes identically — the disambiguation is too coarse. The headline collapse case isn't fixed and we'd pay the data-lake migration cost. Defer until we have multiple independent reasons to bump.
- **Why not auto-synthesize `service.instance.id` from the connection on the server?** Idempotency. The whole point of `block_id = uuid_v5(payload_bytes)` and `process_id = uuid_v5(resource_attrs)` is that retries collapse to the same row at the database. Mixing in connection-time metadata breaks the retry-safety property and loses cross-pod consistency.
- **Why a wrapper script instead of patching Claude Code's resource detector?** Out of our control. Even if we open an upstream issue, the fix has to ship, deploy, and adopt — meanwhile every existing Claude Code install collapses. The wrapper works today.
- **Why rate-limit by `process_id` rather than by IP / by `service.name`?** `process_id` is what's actually load-bearing — that's the noun we're warning about. Two collapses to two different `process_id`s deserve two warnings; two batches under the same collapsed `process_id` deserve one.
- **Why not silence the warning entirely once `service.instance.id` is in `OTEL_RESOURCE_ATTRIBUTES`?** The formula already produces a non-degenerate `process_id` in that case, and `is_degenerate_resource` already returns false. The warning silences itself naturally.

## Documentation

- `mkdocs/docs/otlp/index.md` — primary updates as listed under Phase 4.
- `CHANGELOG.md` — one line.
- The architecture page and schema reference do not need updates (this is a behavior/observability fix, not a data-shape change).

## Testing Strategy

- **Unit:** `block::tests` — call `build_prepared_block` twice on a degenerate resource; assert one warning, two `PreparedBlock`s.
- **Unit:** verify the warning message contains `service.name` (the operator's first clue when grepping) and the literal `OTEL_RESOURCE_ATTRIBUTES=` token (so a future copy-paste catches typos).
- **E2E:** extend `python/micromegas/tests/test_otlp_e2e.py` with a `test_degenerate_resource_warns_once` that POSTs two batches with identical degenerate resources and asserts: (a) both POSTs return 200, (b) both write rows, (c) `log_entries WHERE target='micromegas_otel_ingestion::block' AND msg LIKE 'OTLP resource missing%'` returns exactly one row for that `process_id`.
- **Manual:** run the wrapper against a live service, confirm two `claude` invocations produce two `process_id`s and the `otel_ingest.degenerate_resources` counter stays at zero.
- **Doc lint:** `mkdocs build --strict` clean.

## Open Questions

1. **Counter name** — `otel_ingest.degenerate_resources` matches the `<subsystem>.<event>` convention; is there a project-internal naming standard for ingestion-side counters that should override? (Default: stick with the proposed name; rename if a reviewer flags inconsistency.)
2. **Rate-limit scope** — process-lifetime is the cheap default. Do we want a periodic re-warn (e.g. every 24h per collapsed `process_id`) so a long-running ingestion pod doesn't go silent on a chronic problem? Lean: no. The counter is the long-running observability surface; the log warning is for first detection.
3. **Wrapper script location** — `local_test_env/` is for dev infra. Is `branding/` or a top-level `examples/` more appropriate? Lean toward `local_test_env/` because that's where the start-services tooling already lives and that's where someone setting up a dev box will look.
4. **Should we also open an upstream Claude Code issue** asking their resource detector to populate `host.name`, `process.pid`, and `service.instance.id`? Not part of this plan, but worth queueing as a follow-up — if upstream ships it, the wrapper becomes redundant for new Claude Code versions.
