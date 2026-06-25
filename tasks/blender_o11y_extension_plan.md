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
        ├── durable breadcrumb ring buffer (mmap'd file)   [phase 2]
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
void mm_breadcrumb(mm_handle*, const char* kind, const char* payload); // phase 2: durable
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
- **Phase 1 (cheap, pure Python): harvest on next launch.** On startup the add-on scans for (a) Blender's own `*.crash.txt`/debug output from a prior abnormal exit and (b) an unsent local breadcrumb file, ships them as a CRITICAL log keyed to the prior session's fingerprint, and marks them sent. This reuses the native backtrace Blender already writes — no native crash code required. It tells us how lossy the cheap path is.
- **Phase 2 (native, only if Phase 1 proves too lossy): durable breadcrumbs + minidumps.** Move the breadcrumb ring buffer into an mmap'd file owned by `micromegas-capi` (each `mm_breadcrumb` is a cheap memory write that survives a hard crash because the OS flushes dirty pages). Add an out-of-process Crashpad handler that captures a full minidump and uploads immediately. This is justified by Phase-1 data, not built upfront.

## Implementation Steps

### Phase 1 — Native SDK foundation
1. Create `rust/capi/` crate (`micromegas-capi`), `crate-type = ["cdylib","staticlib"]`, dep `micromegas` default features. No `rust/Cargo.toml` edit needed — the `members = ["*"]` glob auto-includes it.
2. Implement `mm_init`/`mm_shutdown` over `TelemetryGuardBuilder`/`TelemetryGuard`, storing the guard behind the opaque handle.
3. Implement `mm_log` over `dispatch::log_interop` (stack-built `LogMetadata`, no leak); implement the metadata interner (leaked `&'static StaticMetricMetadata` cached by string key) and `mm_metric_i`/`mm_metric_f`/`mm_flush` over the `dispatch::*` functions. (Spans are out of scope — no span FFI.)
4. Generate a C header (cbindgen) and add a minimal C smoke test that inits, logs, and shuts down against a local ingestion server.

### Phase 2 — Blender add-on (logs + metrics)
5. Python binding module (`ctypes`/`cffi`) loading the bundled cdylib; map config + emit functions.
6. Add-on scaffolding (`bl_info`, register/unregister, `atexit` → `mm_shutdown`), process-fingerprint properties at init.
7. Modal recorder + `bpy.msgbus` + `bpy.app.handlers` wiring for actions and lifecycle; performance-metric emitters.
8. Cardinality/privacy filter layer (allowlist of dimensions; verbose-params flag off by default).
9. Wheel packaging that vendors the per-platform cdylib.

### Phase 3 — Crash capture (Phase 1 strategy above)
10. Startup harvester for Blender's crash file + local breadcrumb file → CRITICAL log keyed to prior-session fingerprint, with a sent-marker.
11. Evaluate loss; if warranted, schedule native mmap breadcrumbs + Crashpad as a follow-up (separate plan).

## Files to Create / Modify
- Create `rust/capi/Cargo.toml`, `rust/capi/src/lib.rs`, `rust/capi/cbindgen.toml`, generated `rust/capi/include/micromegas.h`, `rust/capi/tests/`.
- No `rust/Cargo.toml` edit needed: the workspace `members = ["*", …]` glob auto-includes `rust/capi/` and no `exclude` pattern matches it (verify `capi` is not added to `exclude`).
- Create the Blender add-on tree (location TBD — likely a new top-level `blender/` directory mirroring `unreal/`): Python package, ctypes binding, modal recorder, handlers, packaging.
- Docs: new page under `mkdocs/docs/` for native/embedded integration and the Blender add-on.

## Trade-offs
- **Wrap `public` vs `telemetry-sink`/`tracing` directly.** Chose `public` (default features) because it is the stable user-facing surface and the feature gating means it carries no query-engine weight. Reaching into internal crates would couple the FFI to internal boundaries with no footprint benefit.
- **Rust core + C ABI vs a fresh C++ reimplementation (Unreal-style).** Chose Rust core. The protocol (transit/CBOR/LZ4), the transport, and the background-thread sink already exist and are exercised by every Rust service; a C++ reimplementation would be a second copy of the wire format to keep byte-compatible forever. The C++ path only wins if a hard constraint forbids a Rust staticlib in the native build pipeline (see Open Questions). The `staticlib` artifact leaves the door open for Unreal to converge onto this ABI later.
- **Native SDK vs OTLP via the OpenTelemetry Python SDK.** Micromegas accepts OTLP (`/ingestion/otlp/v1/*`), which would be a faster pure-Python on-ramp. Rejected as the foundation because (a) OTLP's async batching loses in-flight breadcrumbs on a native crash — the exact data crash RCA needs — and (b) the native SDK is reusable across all native studio tools, not just Blender. OTLP remains a reasonable fallback if native packaging stalls.
- **ctypes/cffi (plain cdylib) vs a CPython extension module.** Chose plain cdylib + ctypes so builds are per-platform, not per-Python-ABI, and work against stock Blender's bundled interpreter without a custom build.
- **Crashpad now vs harvest-first.** Deferred Crashpad behind Phase-1 harvesting so the native investment is justified by measured loss rather than a hunch; Blender already writes a native backtrace we can collect for free.

## Dependencies
- New build-time: `cbindgen` (header generation). Phase 2 native crash work (deferred) would add Crashpad and an mmap dependency.
- Python add-on: `ctypes`/`cffi` (stdlib/standard); no heavy runtime deps.

## Documentation
- New mkdocs page: embedding Micromegas in a native/non-Rust application via `micromegas-capi` (init/emit/shutdown, threading model, string-cardinality contract).
- New mkdocs page: the Blender add-on (install, configuration, what is captured, privacy/cardinality guarantees).

## Testing Strategy
- **Rust C ABI:** unit tests in `rust/capi/tests/` plus a C smoke test linking the staticlib; init → log/metric → shutdown against a local ingestion server started via `local_test_env/ai_scripts/start_services.py`. Verify rows land via `micromegas-query`.
- **Add-on:** run Blender headless (`blender --background --python`) to exercise the binding, emit synthetic actions/metrics, and confirm ingestion. Manual interactive session to validate the modal recorder and handler coverage.
- **Crash path:** force an abnormal exit, restart, confirm the harvester ships the prior crash file + breadcrumbs keyed to the right session fingerprint.
- **Privacy:** test that the dimension allowlist drops disallowed/high-cardinality keys and that verbose params stay off by default.

## Resolved Decisions
- **Rust-core C ABI is settled.** The team runs stock public Blender binaries, so there is no native build pipeline on the consuming side. The cdylib is built in the micromegas repo's existing Rust CI and shipped prebuilt; artists need no toolchain. The staticlib artifact remains only for future native-linked consumers (e.g. Unreal convergence), not Blender.
- **Semantic capture is `bpy`-only.** With no custom Blender build, C-level operator hooking is not available; user-action capture is limited to what the Python API exposes (modal recorder + `bpy.msgbus` + `bpy.app.handlers`).
- **No dynamic-metric-property API needed.** Metric needs are limited; the small, bounded set of metrics is handled by the same string interner as logs (dimensions baked into bounded metric names or interned property strings), so plain `int_metric`/`float_metric` suffice.
- **Direct ingestion, no relay.** The ingestion server is reachable from all machines, so the add-on connects directly via `TelemetryGuardBuilder`; no store-and-forward/relay layer is needed.
- **Auth via API key in an env var.** The add-on uses `.with_auth_from_env()`, which reads `MICROMEGAS_INGESTION_API_KEY` — no auth code to write. Deployment provisions that env var on artist machines (system-wide or via the launcher). The key is write-only ingestion access.
- **Platform matrix: two targets.** x64 Windows (`x86_64-pc-windows-msvc` → `.dll`) and x64 WSL/Linux (`x86_64-unknown-linux-gnu` → `.so`). No macOS or arm64.

## Open Questions
1. **Add-on location in the repo** — new top-level `blender/` directory (mirroring `unreal/`) vs elsewhere.

## Future Work (out of scope)
- **Spans / scoped timings:** a span FFI (`mm_span_begin`/`mm_span_end`) and Python API, requiring leaked `&'static SpanLocation` paired with `dispatch::on_begin_named_scope`/`on_end_named_scope`. This extension ships logs + metrics only; spans are deferred.
- **Native crash capture (Phase 2):** mmap'd durable breadcrumb ring buffer + out-of-process Crashpad minidumps with immediate upload. Pursued only if Phase-1 harvesting proves too lossy.
- **Symbol server:** a symbol/PDB store keyed by Blender build hash, required to re-symbolize the minidumps from the Phase 2 work above. Deferred with that work.
