# Blender Observability Extension Plan

## Overview
Give the team visibility into Blender stability and performance by emitting logs, performance metrics, and crash information into Micromegas. The work has two reusable parts and one Blender-specific part: (1) a new C ABI crate (`micromegas-capi`) that exposes the existing Rust telemetry **producer** stack to any non-Rust process, (2) a Blender Python add-on that loads that library and captures semantic events (user actions, lifecycle, performance), and (3) a crash-capture path that survives the native crash and attaches the last user actions to the crash report. Root-cause analysis is an existing, external consumer of the resulting lakehouse data and is out of scope here.

## Current State

### Telemetry producer stack (reusable as-is)
- `micromegas` (`rust/public/`) is the sanctioned user-facing crate. Its **default feature set is empty** (`default = []`, `rust/public/Cargo.toml:14`) and the producer crates — `micromegas-telemetry`, `micromegas-telemetry-sink`, `micromegas-tracing` — are non-optional, always-on dependencies (`rust/public/Cargo.toml:44-49`). The analytics/DataFusion/object_store/server stack is entirely behind the optional `server` feature (`rust/public/Cargo.toml:15-41`), so a `default-features` dependency does **not** drag the query engine into a shipped library.
- Initialization is an RAII guard: `TelemetryGuardBuilder` → `TelemetryGuard` (`rust/telemetry-sink/src/lib.rs`). Builder options include `.with_telemetry_sink_url()`, `.with_auth_from_env()` (API key or OIDC from env), `.with_process_properties()`, `.with_system_metrics_enabled()`. Dropping the guard flushes buffers and shuts down the background thread.
- Transport already runs on its **own OS thread**: `HttpEventSink` spawns a `std::thread` (`rust/telemetry-sink/src/http_event_sink.rs:128`) whose body owns a private `tokio::runtime::Runtime` (`rust/telemetry-sink/src/http_event_sink.rs:562`). Event recording (`dispatch::log`, `dispatch::int_metric`, `dispatch::float_metric`, `dispatch::on_begin_named_scope`/`on_end_named_scope` in `rust/tracing/src/dispatch.rs`) is **fully synchronous and thread-safe** — callers need no async runtime. This is the "embed a native part that owns its own thread" property we want, already implemented.
- Event metadata uses `&'static str` (`LogMetadata` in `rust/tracing/src/logs/events.rs`, `StaticMetricMetadata` in `rust/tracing/src/metrics/events.rs`). Log *message* bodies are passed as `fmt::Arguments` and may be runtime strings; only the *metadata* (target, metric name, unit) must be `'static`. This shapes the FFI design (see below).

### What does not exist yet
- **No exported C ABI in the repo.** No `#[no_mangle]` exports and no cbindgen usage — there are no `extern "C"` exports anywhere in the repo. The only `cdylib` is `rust/datafusion-wasm/` (WASM, unrelated). This is greenfield.
- **No crash/minidump ingestion.** The Unreal sink (`unreal/MicromegasTelemetrySink/.../SystemErrorReporter.cpp`) captures symbolized stack traces as log entries on `OnHandleSystemError`, but there is no minidump/Breakpad/Crashpad support anywhere.
- The Python client (`python/micromegas/`) is **query + bulk-ingest only** (`bulk_ingest()` is an Arrow-table loader for replication/admin) — no telemetry event-production (logs/metrics) API.

## Design

### Architecture
```
Blender process
├── Blender C/C++ core ──(crash)──> Blender's own *.crash.txt
└── Embedded CPython
    └── Add-on (bpy)                         semantic capture
        ├── modal recorder  (operators, key/mouse events)
        ├── bpy.msgbus      (property edits)
        ├── bpy.app.handlers(load/save/undo/render/frame/depsgraph)
        └── ctypes/cffi  ─────────────┐
                                       ▼
        libmicromegas_capi.{so,dll,dylib}   (new: micromegas-capi)
        ├── flat C ABI (extern "C", #[no_mangle])
        ├── holds TelemetryGuard (init/shutdown)
        ├── string interner  (runtime str -> &'static, bounded)
        └── HttpEventSink  ── own OS thread + tokio runtime ──► ingestion-srv
```

### Component 1 — `micromegas-capi` (C ABI crate)
New workspace member `rust/capi/` → crate `micromegas-capi`. Depends on `micromegas = { workspace = true }` (default features only — producer side, no `server`). Wrapping `public` rather than the internal crates keeps the FFI on the stable, sanctioned surface and insulates it from internal refactors.

`Cargo.toml`:
```toml
[lib]
crate-type = ["cdylib", "staticlib"]
```
- **cdylib** → the `.so`/`.dll`/`.dylib` the Blender add-on loads at runtime (a static lib cannot be `dlopen`'d).
- **staticlib** → the `.a`/`.lib` for linking into other native studio tools at build time, and a future convergence target for the Unreal sink.

Per-platform cdylib notes (all from one source + `extern "C"` shim; only target and loaded filename change):
- **Windows:** build with `x86_64-pc-windows-msvc` to match stock Blender's MSVC-built Python; Rust auto-exports `#[no_mangle] extern "C"` symbols from a cdylib (no `.def`/`dllexport`); static-link the CRT (`-C target-feature=+crt-static`) so no VC++ redistributable is required (safe — nothing CRT-owned crosses the C ABI). Output is `micromegas_capi.dll` (no `lib` prefix). x64-only ⇒ `ctypes.CDLL` is the correct loader (single calling convention).
- **Linux / WSL:** `x86_64-unknown-linux-gnu` → `libmicromegas_capi.so`. WSL2 is a real Linux environment, so the same `.so` covers it (`sys.platform == 'linux'`). Build against an old-enough glibc to load in whatever Blender build runs there.
- No macOS / arm64 target needed.
- The Python loader selects the filename per `sys.platform`.

Proposed flat C ABI (opaque handle, all functions `extern "C"`):
```c
typedef struct mm_handle mm_handle;

mm_handle* mm_init(const mm_config* cfg);   // builds TelemetryGuard, stores it
void       mm_shutdown(mm_handle*);         // drops guard -> flush + join bg thread

void mm_log   (mm_handle*, int level, const char* target, const char* msg);
void mm_metric_i(mm_handle*, const char* name, const char* unit, uint64_t value);
void mm_metric_f(mm_handle*, const char* name, const char* unit, double  value);
void mm_flush  (mm_handle*);
```
`mm_config` carries sink URL, optional auth fields, and process properties (key/value array); the shim maps it to `TelemetryGuardBuilder` calls.

**Static-metadata handling (the one real wrinkle).** The metric dispatch entry points do not take loose `&'static str` — they take a whole leaked `&'static` metadata struct: `dispatch::int_metric`/`float_metric` take `&'static StaticMetricMetadata` (`rust/tracing/src/metrics/events.rs:5-12`: `lod/name/unit/target/file/line`). So the shim cannot just intern strings; for **metrics** it must construct and `Box::leak` the *entire* metadata struct (filling the non-string fields — `lod`, synthetic `file`/`line`) and cache it behind a `Mutex<HashMap<…, &'static StaticMetricMetadata>>` keyed by the struct's string fields (`(name, unit, target)`). Interning is safe **because these are low-cardinality by design** (a bounded set of metric names, units); the cardinality discipline below is what keeps the leak bounded. The shim should expose this contract clearly so callers don't intern unbounded values. (Spans are out of scope — see below — so no `SpanLocation` leaking is required.)

**Logs avoid the leak entirely.** `dispatch::log_interop(metadata: &LogMetadata, args)` (`rust/tracing/src/dispatch.rs:145`) takes a **non-`'static`** `&LogMetadata`, which is exactly how the Unreal sink ships runtime log level/category/message. For `mm_log` — where `level`/`target`/`message` are all runtime values — the shim builds a `LogMetadata` on the stack and calls `log_interop`, so **no per-log metadata is leaked** and the awkward `LogMetadata<'a>` lifetime parameter (`rust/tracing/src/logs/events.rs:5-13`, which also carries an `AtomicU32 level_filter`) never has to be coerced to `'static`. Message bodies go through `format_args!("{}", msg)`. Interning/leaking is reserved for metrics only.

**Spans are out of scope.** This extension emits **logs and metrics only**; there is no span FFI (`mm_span_begin`/`mm_span_end`) and no span Python API. Span support — which would require constructing and leaking `&'static SpanLocation` (`rust/tracing/src/spans/events.rs:4-10`) and pairing it with the dispatch scope API (`dispatch::on_begin_named_scope`/`on_end_named_scope`, `rust/tracing/src/dispatch.rs:275,284`) — is deferred to Future Work.

Metrics are handled by this same interner — there is no need for a dynamic per-measurement property API. The set of distinct metrics is small and bounded, so any dimension a metric needs is either baked into a bounded set of metric names or carried as an interned property string; both are bounded and safe to leak. Metric emission is therefore just `int_metric`/`float_metric` over interned `(name, unit)`, identical in spirit to interned log targets.

### Component 2 — Blender add-on (Python)
A standard add-on packaged as a pip-installable wheel that bundles the prebuilt cdylib per platform and binds it via `ctypes`/`cffi`. The native worker thread is an OS thread inside the loaded library, outside the GIL, so it keeps flushing even while Python is blocked. `mm_shutdown` is wired to `atexit` and add-on unregister.

Captured signals:
- **User actions** — a persistent modal operator records operator invocations and discrete input events (key/mouse-button, area/region), throttling continuous motion. Supplement with `bpy.msgbus` for property edits and `bpy.app.handlers` for load/save, undo/redo, render begin/end, frame change, depsgraph updates. There is no single "all actions" hook; coverage is high but not 100%, and a modal operator can be suspended in some states — document this.
- **Performance metrics** — redraw/eval time, frame time, peak memory, undo-stack depth, `.blend` size, render durations, modifier eval cost, emitted from handlers.
- **Process/session fingerprint** (process properties at `mm_init`): Blender version + build hash, OS, GPU/driver, enabled add-ons + versions, a per-launch UUID. These are the dimensions that make stability analysis possible.

**Cardinality and privacy rules (enforced in the add-on):**
- Metric properties / tags must be **low-cardinality and bounded** (operator identity, bounded categories, status). Never use unbounded values (session IDs, per-asset names, timestamps) as metric dimensions.
- High-cardinality or project-identifying values (scene/asset/project names, file paths) must **not** be emitted as dimensions, and verbose operator parameters are gated behind an off-by-default flag. Hash any identifier that must be correlatable. This satisfies the "no private/internal names in telemetry" constraint.

### Component 3 — Crash capture
Phased, gated on evidence:
- **Phase 1 (cheap, pure Python): harvest on next launch.** On startup the add-on scans for Blender's own `*.crash.txt`/debug output from a prior abnormal exit and ships it as a CRITICAL log keyed to the prior session's fingerprint. The last user actions before the crash come from the normal telemetry stream already ingested under that fingerprint — no parallel local store. This reuses the native backtrace Blender already writes — no native crash code required. It tells us how lossy the cheap path is.
  - **Concurrent-launch safety + dedup via atomic rename.** Artists run several Blender instances at once, so two launches can see the same crash file simultaneously. Each harvester **claims** a crash file with an atomic rename (e.g. `*.crash.txt` → `*.crash.txt.claimed`) before uploading; only the instance that wins the rename ships it, so a crash is never reported twice. This rename is the only local bookkeeping — there is no sent-marker store.
  - **Best-effort upload.** If the upload fails after the claim, that crash report is simply lost — no retry queue, no recovery, no re-scan of claimed files. Accepting this loss is what keeps the harvester stateless beyond the rename; it is consistent with Phase 1 being a cheap measurement of how lossy the free path is.
- **Phase 2 (native, only if Phase 1 proves too lossy): minidumps.** Add an out-of-process Crashpad handler that captures a full minidump and uploads immediately. This is justified by Phase-1 data, not built upfront.
- **In-flight event loss is a tracing-crate problem, not a Blender one.** If Phase-1 data shows we are losing the last user actions before a crash — events buffered in the sink that never shipped — the fix belongs in the shared `micromegas-tracing`/`telemetry-sink` layer (e.g. tighter flush cadence or a flush-on-fatal-signal path), so every producer benefits. We deliberately do **not** build a parallel durable-breadcrumb store in the C ABI to paper over sink reliability.

## Implementation Steps

### Phase 1 — Native SDK foundation ✅ COMPLETE
1. ✅ Created `rust/capi/` crate (`micromegas-capi`), `crate-type = ["cdylib","staticlib","rlib"]`. Depends on `micromegas-telemetry-sink` and `micromegas-tracing` directly (not through `public`) since the C ABI needs dispatch internals not re-exported from the public crate. The workspace `members = ["*"]` glob auto-includes it — no `rust/Cargo.toml` edit needed.
2. ✅ Implemented `mm_init`/`mm_shutdown` over `TelemetryGuardBuilder`/`TelemetryGuard`, storing the guard behind the opaque `MmHandle`. Null `cfg` returns null. Empty `sink_url` string (non-null pointer) explicitly suppresses env-var pickup, enabling test isolation from `MICROMEGAS_TELEMETRY_URL`.
3. ✅ Implemented `mm_log` over `dispatch::log_interop` (stack-built `LogMetadata`, no leak). Metric interner: `OnceLock<Mutex<HashMap<(name,unit), &'static StaticMetricMetadata>>>` with `Box::leak` on first use. `mm_metric_i`/`mm_metric_f`/`mm_flush` implemented over `dispatch::*`. No span FFI.
4. ✅ C header hand-authored at `rust/capi/include/micromegas.h`; `cbindgen.toml` provided for regeneration. 8 Rust smoke tests in `rust/capi/tests/smoke_test.rs` — all pass.

### Phase 2 — Blender add-on (logs + metrics) ✅ COMPLETE
5. ✅ Python binding in `blender/micromegas_blender/binding.py` — ctypes wrapper loading `libmicromegas_capi.so` or `micromegas_capi.dll` from the add-on's `lib/` subdirectory.
6. ✅ Add-on scaffolding in `blender/micromegas_blender/__init__.py`: `bl_info`, `register`/`unregister`, `atexit` → `mm_shutdown`, process-fingerprint properties (Blender version, build hash, OS, session UUID, add-on version), 30 s periodic flush timer.
7. ✅ Modal recorder (`recorder.py`) + `bpy.msgbus` + `bpy.app.handlers` wiring (`handlers.py`) for user actions and lifecycle. Performance metrics: `blender.eval_ms`, `blender.render_duration_s`, `blender.blend_size_mb`, `blender.rss_mb`, `blender.frame`.
8. ✅ Cardinality/privacy: metric names are bounded (operator type, area type, status); no per-asset names, file paths, or session IDs as metric dimensions. Verbose operator parameters are not captured by default.
9. ⏳ Wheel packaging (vendoring per-platform cdylib) — not implemented; wheel build script is a follow-up packaging task.

### Phase 3 — Crash capture (Phase 1 strategy) ✅ COMPLETE
10. ✅ Startup harvester in `blender/micromegas_blender/crash_harvester.py`: scans `/tmp/*.crash.txt` (Linux) or `%TEMP%\*.crash.txt` (Windows); atomic rename for dedup; ships as FATAL log; best-effort (no retry on upload failure). Wired to `bpy.app.handlers.load_factory_startup_post`.
11. ⏳ Evaluate loss — Phase-2 Crashpad is a separate future initiative.

## Files Created
- `rust/capi/Cargo.toml`
- `rust/capi/src/lib.rs`
- `rust/capi/cbindgen.toml`
- `rust/capi/include/micromegas.h`
- `rust/capi/tests/smoke_test.rs`
- `blender/micromegas_blender/__init__.py`
- `blender/micromegas_blender/binding.py`
- `blender/micromegas_blender/recorder.py`
- `blender/micromegas_blender/handlers.py`
- `blender/micromegas_blender/crash_harvester.py`
- `mkdocs/docs/native/index.md`
- `mkdocs/docs/blender/index.md`
- `mkdocs/mkdocs.yml` (updated nav)

## Remaining / Follow-up
- **Wheel packaging:** build script to bundle the pre-built cdylib into an installable `.zip`. This is a CI/distribution concern, not a code concern.
- **End-to-end test against live server:** run `blender --background --python` to emit synthetic events and confirm rows appear via `micromegas-query`.
- **Crash-path test:** force abnormal exit, restart Blender, confirm harvester ships the prior crash file.
- **Phase-2 crash capture (Crashpad):** pursued only if Phase-1 data shows unacceptable loss.

## Files to Create / Modify
- ~~Create `rust/capi/Cargo.toml`, `rust/capi/src/lib.rs`, `rust/capi/cbindgen.toml`, generated `rust/capi/include/micromegas.h`, `rust/capi/tests/`.~~ DONE
- ~~No `rust/Cargo.toml` edit needed: the workspace `members = ["*", …]` glob auto-includes `rust/capi/` and no `exclude` pattern matches it.~~ Confirmed.
- ~~Create the Blender add-on tree in a new top-level `blender/` directory (mirroring `unreal/`): Python package, ctypes binding, modal recorder, handlers, packaging.~~ DONE (packaging TBD)
- ~~Docs: new page under `mkdocs/docs/` for native/embedded integration and the Blender add-on.~~ DONE

## Trade-offs
- **Wrap `public` vs `telemetry-sink`/`tracing` directly.** Chose `public` (default features) because it is the stable user-facing surface and the feature gating means it carries no query-engine weight. Reaching into internal crates would couple the FFI to internal boundaries with no footprint benefit.
- **Rust core + C ABI vs a fresh C++ reimplementation (Unreal-style).** Chose Rust core. The protocol (transit/CBOR/LZ4), the transport, and the background-thread sink already exist and are exercised by every Rust service; a C++ reimplementation would be a second copy of the wire format to keep byte-compatible forever. The C++ path only wins if a hard constraint forbids a Rust staticlib in the native build pipeline (see Open Questions). The `staticlib` artifact leaves the door open for Unreal to converge onto this ABI later.
- **Native SDK vs OTLP via the OpenTelemetry Python SDK.** Micromegas accepts OTLP (`/ingestion/otlp/v1/*`), which would be a faster pure-Python on-ramp. Rejected as the foundation because (a) it runs through an external SDK we cannot harden — OTLP's async batching loses in-flight events on a native crash, and the fix for that has to live in the producer/transport layer, which with the native SDK is `micromegas-tracing` (ours to improve) — and (b) the native SDK is reusable across all native studio tools, not just Blender. OTLP remains a reasonable fallback if native packaging stalls.
- **ctypes/cffi (plain cdylib) vs a CPython extension module.** Chose plain cdylib + ctypes so builds are per-platform, not per-Python-ABI, and work against stock Blender's bundled interpreter without a custom build.
- **Crashpad now vs harvest-first.** Deferred Crashpad behind Phase-1 harvesting so the native investment is justified by measured loss rather than a hunch; Blender already writes a native backtrace we can collect for free.

## Dependencies
- New build-time: `cbindgen` (header generation). Phase 2 native crash work (deferred) would add a Crashpad dependency.
- Python add-on: `ctypes`/`cffi` (stdlib/standard); no heavy runtime deps.

## Documentation
- New mkdocs page: embedding Micromegas in a native/non-Rust application via `micromegas-capi` (init/emit/shutdown, threading model, string-cardinality contract).
- New mkdocs page: the Blender add-on (install, configuration, what is captured, privacy/cardinality guarantees).

## Testing Strategy
- **Rust C ABI:** unit tests in `rust/capi/tests/` plus a C smoke test linking the staticlib; init → log/metric → shutdown against a local ingestion server started via `local_test_env/ai_scripts/start_services.py`. Verify rows land via `micromegas-query`.
- **Add-on:** run Blender headless (`blender --background --python`) to exercise the binding, emit synthetic actions/metrics, and confirm ingestion. Manual interactive session to validate the modal recorder and handler coverage.
- **Crash path:** force an abnormal exit, restart, confirm the harvester ships the prior crash file keyed to the right session fingerprint.
- **Privacy:** test that the dimension allowlist drops disallowed/high-cardinality keys and that verbose params stay off by default.

## Resolved Decisions
- **Rust-core C ABI is settled.** The team runs stock public Blender binaries, so there is no native build pipeline on the consuming side. The cdylib is built in the micromegas repo's existing Rust CI and shipped prebuilt; artists need no toolchain. The staticlib artifact remains only for future native-linked consumers (e.g. Unreal convergence), not Blender.
- **Semantic capture is `bpy`-only.** With no custom Blender build, C-level operator hooking is not available; user-action capture is limited to what the Python API exposes (modal recorder + `bpy.msgbus` + `bpy.app.handlers`).
- **No dynamic-metric-property API needed.** Metric needs are limited; the small, bounded set of metrics is handled by the same string interner as logs (dimensions baked into bounded metric names or interned property strings), so plain `int_metric`/`float_metric` suffice.
- **Direct ingestion, no relay.** The ingestion server is reachable from all machines, so the add-on connects directly via `TelemetryGuardBuilder`; no store-and-forward/relay layer is needed.
- **Auth via API key in an env var.** The add-on uses `.with_auth_from_env()`, which reads `MICROMEGAS_INGESTION_API_KEY` — no auth code to write. Deployment provisions that env var on artist machines (system-wide or via the launcher). The key is write-only ingestion access.
- **Platform matrix: two targets.** x64 Windows (`x86_64-pc-windows-msvc` → `.dll`) and x64 WSL/Linux (`x86_64-unknown-linux-gnu` → `.so`). No macOS or arm64.
- **Add-on location: new top-level `blender/` directory**, mirroring `unreal/`.

## Future Work (out of scope)
- **Spans / scoped timings:** a span FFI (`mm_span_begin`/`mm_span_end`) and Python API, requiring leaked `&'static SpanLocation` paired with `dispatch::on_begin_named_scope`/`on_end_named_scope`. This extension ships logs + metrics only; spans are deferred.
- **Native crash capture (Phase 2):** out-of-process Crashpad minidumps with immediate upload. Pursued only if Phase-1 harvesting proves too lossy. Any loss of the last actions *before* the crash is addressed by hardening flush behavior in `micromegas-tracing`/`telemetry-sink`, not by a local store in the C ABI.
- **Symbol server:** a symbol/PDB store keyed by Blender build hash, required to re-symbolize the minidumps from the Phase 2 work above. Deferred with that work.
