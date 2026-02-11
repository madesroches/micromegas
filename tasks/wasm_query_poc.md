# WASM Query POC Plan

## Status: Runtime Bug Found — `std::time` Panic

The DataFusion WASM compilation spike is **successful** — all code compiles and the WASM binary builds end-to-end. However, `wasm-bindgen-test` integration tests revealed a **runtime panic**: DataFusion internally calls `std::time::Instant::now()`, which is not implemented on `wasm32-unknown-unknown`.

In release mode (`panic=abort` + LTO), this panic is stripped to just "unreachable executed". The debug-mode tests give the real message:

```
panicked at library/std/src/sys/pal/wasm/../unsupported/time.rs:31:9:
time not implemented on this platform
```

**Root cause:** DataFusion uses `std::time::Instant` for query execution metrics. The `wasm32-unknown-unknown` target has no time implementation — it's a bare WASM platform with no OS. Fix: add the `web-time` crate which provides `Instant`/`SystemTime` backed by `performance.now()` / `Date.now()`, and patch DataFusion's dependency on `std::time` via Cargo.

### Spike Results

| Question | Result |
|----------|--------|
| DataFusion compiles to WASM? | **Yes** |
| Feature flags needed? | `default-features = false`, `sql`, `nested_expressions` + `getrandom/wasm_js` |
| WASM binary size (raw, after wasm-opt -Os)? | **24 MB** |
| Gzipped size? | **5.9 MB** |
| wasm-bindgen JS glue? | Works, auto-generated types match our API |
| Build pipeline (build.py)? | End-to-end: cargo build → wasm-bindgen → wasm-opt → copy to web app |
| TypeScript integration? | Clean — `tsc --noEmit` passes, all 664 frontend tests pass |
| Backend integration? | Clean — `cargo build` + all 20 backend tests pass |
| Runtime (WASM in browser)? | **Panics** — `std::time::Instant::now()` not available on `wasm32-unknown-unknown` |
| WASM integration tests? | 7 tests via `wasm-bindgen-test`, all reproduce the time panic |

### What's Left to Validate (runtime)

- [ ] Fix `std::time` panic (patch with `web-time` crate)
- [ ] IPC bytes from server → `register_table` in browser
- [ ] `execute_sql` against registered table in browser
- [ ] IPC output from WASM → `tableFromIPC` → rendered table
- [ ] Lazy-loading latency in browser
- [ ] Aggregates, joins, window functions in WASM DataFusion
- [ ] Single-threaded query performance for typical workloads

## Goal

De-risk the notebook queries feature by building a standalone "Local Query" screen type that validates the full DataFusion WASM stack — compilation, IPC ingestion, local SQL execution, IPC output — without touching any existing notebook code.

## What It Validates

1. DataFusion compiles to `wasm32-unknown-unknown` and runs in the browser
2. Arrow IPC bytes from the server register correctly as MemTables
3. Local SQL execution against registered tables works
4. IPC output from WASM deserializes into arrow-js Tables for rendering
5. The `fetchQueryIPC()` path (raw IPC bytes, no JS-side Arrow decoding) works
6. Lazy-loading the WASM module via Vite works
7. Bundle size and load time are acceptable

If this POC works, all the hard unknowns for the notebook queries plan are resolved. What remains is UI/UX integration work (cell config migration, source toggle, execution context wiring) — lower risk.

## Screen Design

A two-panel screen: **source query** (remote, fetches from server and registers in WASM) and **local query** (executes against registered data in WASM).

```
┌─────────────────────────────────────────────────┐
│  Local Query Screen                             │
├─────────────────────────────────────────────────┤
│  Source: [table name: "data"]                   │
│  ┌─────────────────────────────────────────┐    │
│  │ SELECT time, host, level, msg           │    │
│  │ FROM log_entries                        │    │
│  │ WHERE time BETWEEN '$begin' AND '$end'  │    │
│  │ LIMIT 1000                              │    │
│  └─────────────────────────────────────────┘    │
│  [Fetch & Register]     rows: 1000  (2.3 MB)   │
│                                                 │
│  Local Query:                                   │
│  ┌─────────────────────────────────────────┐    │
│  │ SELECT host, count(*) as cnt            │    │
│  │ FROM data                               │    │
│  │ GROUP BY host ORDER BY cnt DESC         │    │
│  └─────────────────────────────────────────┘    │
│  [Run]                                          │
│                                                 │
│  ┌─────────────────────────────────────────┐    │
│  │ host          │ cnt                     │    │
│  │───────────────┼─────────────────────────│    │
│  │ web-01        │ 342                     │    │
│  │ web-02        │ 218                     │    │
│  │ db-01         │ 87                      │    │
│  └─────────────────────────────────────────┘    │
└─────────────────────────────────────────────────┘
```

The source query is a normal remote SQL query (uses time range, data source, variable substitution — all existing infrastructure). The local query runs entirely in WASM.

## Config

```typescript
interface LocalQueryConfig {
  timeRangeFrom?: string
  timeRangeTo?: string
  dataSource?: string
  // Source query: fetches from server, registers result in WASM
  sourceSql: string
  sourceTableName: string
  // Local query: executes against WASM context
  localSql: string
  [key: string]: unknown
}
```

Default config:

```json
{
  "timeRangeFrom": "now-5m",
  "timeRangeTo": "now",
  "sourceSql": "SELECT process_id, exe, start_time, username, computer\nFROM processes\nLIMIT 100",
  "sourceTableName": "data",
  "localSql": "SELECT * FROM data LIMIT 10"
}
```

## Implementation

### Step 1: DataFusion WASM Crate

New crate at `rust/datafusion-wasm/`. This crate **must not** be a member of the main `rust/Cargo.toml` workspace — it targets `wasm32-unknown-unknown` and would break normal `cargo build`/`cargo test`. It has its own `Cargo.toml` with independent dependency versions.

```toml
[package]
name = "datafusion-wasm"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
arrow = { version = "57.2", default-features = false, features = ["ipc"] }
datafusion = { version = "52.1", default-features = false, features = ["nested_expressions", "sql"] }
getrandom = { version = "0.3", features = ["wasm_js"] }
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"

[profile.release]
lto = true
opt-level = "s"
```

**Feature flags resolved:** DataFusion's `default-features = false` disables parquet, compression codecs, and other features that depend on C libraries and won't compile on WASM. The minimal set discovered during the spike:
- `sql` + `nested_expressions` on DataFusion
- `ipc` on Arrow (default-features off to avoid parquet/compression)
- `getrandom` with `wasm_js` feature — required because DataFusion transitively pulls in `getrandom` (via `uuid` → `rand`), and `wasm32-unknown-unknown` has no OS-level entropy source. The `wasm_js` feature routes to `crypto.getRandomValues()`.

**Single-threaded execution constraint:** DataFusion internally uses `tokio::spawn` for parallel partition execution. On `wasm32-unknown-unknown` there is no multi-threaded tokio runtime — `wasm-bindgen-futures` provides `spawn_local` for single-threaded async. DataFusion falls back to single-partition execution, so queries will work but won't match native performance. This is acceptable for the POC.

Minimal API — only what the POC needs:

```rust
#[wasm_bindgen]
pub struct WasmQueryEngine {
    ctx: SessionContext,
}

#[wasm_bindgen]
impl WasmQueryEngine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self { ... }

    /// Register Arrow IPC stream bytes as a named table.
    /// `ipc_bytes` must be a complete Arrow IPC stream (magic + schema + batches + EOS).
    /// Returns the number of rows registered.
    pub fn register_table(&self, name: &str, ipc_bytes: &[u8]) -> Result<u32, JsValue> { ... }

    /// Execute SQL, return Arrow IPC stream bytes.
    pub async fn execute_sql(&self, sql: &str) -> Result<Vec<u8>, JsValue> { ... }

    /// Reset (deregister all tables).
    /// Uses interior mutability via SessionContext's Arc<RwLock>.
    pub fn reset(&self) { ... }
}
```

No `register_as` parameter on `execute_sql`, no `read_table_ipc` — those are notebook features. The POC only needs register + execute.

Build:

```bash
cd rust/datafusion-wasm
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/datafusion_wasm.wasm --out-dir pkg --target web
wasm-opt pkg/datafusion_wasm_bg.wasm -Os -o pkg/datafusion_wasm_bg.wasm
```

**This is the spike.** If DataFusion doesn't compile to WASM, we stop here and evaluate DuckDB-WASM as the fallback (see notebook queries plan).

### Step 2: WASM Build Integration

Wire the WASM artifact from Step 1 into the analytics-web-app build pipeline.

**Build script** (`rust/datafusion-wasm/build.py`): automates the `cargo build` → `wasm-bindgen` → `wasm-opt` pipeline from Step 1 and copies the output to a known location. This script is run manually (not on every `yarn dev`) — the `.wasm` + JS glue are checked into `analytics-web-app/src/lib/datafusion-wasm/` so that frontend devs don't need the WASM toolchain installed.

**Package resolution**: add a path dependency in the web app so that `import('datafusion-wasm')` resolves locally:

```json
// analytics-web-app/package.json
"dependencies": {
  "datafusion-wasm": "file:src/lib/datafusion-wasm"
}
```

Alternatively, use a Vite alias to point `datafusion-wasm` at the built `pkg/` directory. Either way, `optimizeDeps.exclude` must list `datafusion-wasm` so Vite doesn't try to bundle the `.wasm` binary.

**Prerequisites**: developers modifying the WASM crate need `wasm-bindgen-cli`, `wasm-opt`, and the `wasm32-unknown-unknown` rustup target. Document these in the crate's README.

### Step 3: Vite Lazy Loading

Add the WASM module to the analytics-web-app.

```typescript
// lib/wasm-engine.ts — lazy-loads the WASM module
let enginePromise: Promise<typeof import('datafusion-wasm')> | null = null

export async function loadWasmEngine() {
  if (!enginePromise) {
    enginePromise = import('datafusion-wasm').then(async (mod) => {
      await mod.default()  // initialize WASM
      return mod
    })
  }
  return enginePromise
}
```

Vite config: add `wasm()` plugin or configure `optimizeDeps.exclude` for the WASM package. The WASM binary is lazy-loaded only when the local query screen is opened.

### Step 4: fetchQueryIPC

Add to `arrow-stream.ts`:

```typescript
export async function fetchQueryIPC(
  params: StreamQueryParams,
  signal?: AbortSignal,
): Promise<Uint8Array> {
  // Same HTTP setup as streamQuery (POST /api/query-stream)
  // Parse JSON frames, collect raw IPC message bytes (schema + batches)
  // Assemble into a valid Arrow IPC stream format:
  //   magic ("ARROW1") + schema message + batch messages + EOS continuation (0xFFFFFFFF + 0x00000000)
  // Return complete IPC stream as Uint8Array
}
```

**IPC format contract:** `fetchQueryIPC` returns a complete Arrow IPC stream — not individual messages or raw concatenated bytes. This matches what `arrow::ipc::reader::StreamReader` expects on the Rust/WASM side for `register_table`. The server's JSON-framed protocol sends individual IPC messages (one schema frame + N batch frames); `fetchQueryIPC` assembles them into a valid stream by adding the IPC magic prefix and EOS footer.

This is a standalone addition — no changes to existing `streamQuery()` or `executeStreamQuery()`.

### Step 5: Screen Type Registration

**Backend** (`screen_types.rs`):

Add `LocalQuery` variant to `ScreenType` enum, `FromStr`, `all()`, `as_str()`, `info()`, `default_config()`.

**Frontend** (`screens-api.ts`):

Add `'local_query'` to `ScreenTypeName` union.

### Step 6: LocalQueryRenderer

New file: `screen-renderers/LocalQueryRenderer.tsx`

```typescript
export function LocalQueryRenderer({
  config, onConfigChange, savedConfig,
  timeRange, rawTimeRange, onSave, onSaveRef, dataSource,
}: ScreenRendererProps) {
  const localConfig = config as unknown as LocalQueryConfig

  // WASM engine — loaded lazily, lives for component lifetime
  const [engine, setEngine] = useState<WasmQueryEngine | null>(null)
  useEffect(() => {
    loadWasmEngine().then(mod => setEngine(new mod.WasmQueryEngine()))
    return () => { /* engine is GC'd */ }
  }, [])

  // Source query state
  const [sourceStatus, setSourceStatus] = useState<'idle'|'loading'|'ready'|'error'>('idle')
  const [sourceRowCount, setSourceRowCount] = useState(0)
  const [sourceError, setSourceError] = useState<string>()

  // Local query state
  const [localResult, setLocalResult] = useState<Table | null>(null)
  const [localStatus, setLocalStatus] = useState<'idle'|'loading'|'done'|'error'>('idle')
  const [localError, setLocalError] = useState<string>()

  // AbortController for cancelling in-flight source fetches
  const abortRef = useRef<AbortController | null>(null)
  useEffect(() => () => abortRef.current?.abort(), [])

  // Fetch source data → register in WASM
  const fetchAndRegister = useCallback(async () => {
    if (!engine) return
    abortRef.current?.abort()
    const controller = new AbortController()
    abortRef.current = controller
    setSourceStatus('loading')
    try {
      const sql = substituteTimeRange(localConfig.sourceSql, timeRange)
      const ipcBytes = await fetchQueryIPC(
        { sql, begin: timeRange.begin, end: timeRange.end, dataSource },
        controller.signal
      )
      // register_table returns row count — no need to decode IPC on JS side
      const rowCount = engine.register_table(localConfig.sourceTableName, ipcBytes)
      setSourceRowCount(rowCount)
      setSourceStatus('ready')
    } catch (e) {
      if (!controller.signal.aborted) {
        setSourceError(e.message)
        setSourceStatus('error')
      }
    }
  }, [engine, localConfig.sourceSql, localConfig.sourceTableName, timeRange, dataSource])

  // Execute local query against WASM
  const executeLocal = useCallback(async () => {
    if (!engine) return
    setLocalStatus('loading')
    try {
      const ipcBytes = await engine.execute_sql(localConfig.localSql)
      const table = tableFromIPC(ipcBytes)
      setLocalResult(table)
      setLocalStatus('done')
    } catch (e) {
      setLocalError(e.message)
      setLocalStatus('error')
    }
  }, [engine, localConfig.localSql])

  // ... SQL editors, error display (inline below each editor), result table, save handling ...
}

registerRenderer('local_query', LocalQueryRenderer)
```

Register in `init.ts`:
```typescript
import './LocalQueryRenderer'
```

### Step 7: Result Display

Reuse the existing table rendering components from the Table screen. The local query result is a standard arrow-js `Table` — same as what every other renderer displays.

## What We Learn

| Question | How the POC answers it | Status |
|---|---|---|
| Does DataFusion compile to WASM? | Step 1 — the spike | **Yes** |
| What's the WASM binary size? | Step 1 — measure gzipped output | **24 MB raw, 5.9 MB gzipped** |
| What DataFusion feature flags are needed? | Step 1 — iterative feature flag discovery | **Resolved** (see Cargo.toml above) |
| Does DataFusion run in WASM? | `wasm-bindgen-test` integration tests | **No** — `std::time` panic (fix: `web-time` crate) |
| Does IPC ingestion work? | Step 4+6 — `fetchQueryIPC` → `register_table` | Blocked on time fix |
| Does local SQL execution work? | Step 6 — `execute_sql` against registered table | Blocked on time fix |
| Does IPC output deserialize correctly? | Step 6 — `tableFromIPC` on WASM output | Blocked on time fix |
| What's the latency? | Step 6 — measure register + execute + deserialize | Blocked on time fix |
| Does Vite lazy-loading work? | Step 3 — WASM module loads on demand | Pending runtime test |
| Does the build pipeline work? | Step 2 — WASM artifact flows into web app | **Yes** |
| What DataFusion features work in WASM? | Manual testing — try aggregates, joins, window functions | Blocked on time fix |
| Single-threaded perf acceptable? | Step 6 — measure query times for typical workloads | Blocked on time fix |

## Scope Boundaries

**In scope:**
- DataFusion WASM crate with `register_table` + `execute_sql`
- `fetchQueryIPC` in arrow-stream.ts
- `local_query` screen type (backend + frontend)
- Basic two-panel UI with source + local SQL editors
- Table result display

**Out of scope (stays in notebook queries plan):**
- Cell config V2 migration
- Polymorphic Query type
- `useCellExecution` changes
- Source toggle UI in notebook cells
- `register_as` / `read_table_ipc` on the engine
- UDFs in WASM
- Arrow FFI output path
- Multiple source tables (POC has one)

## Files

**New:**
- `rust/datafusion-wasm/Cargo.toml` + `src/lib.rs` + `README.md` (toolchain prereqs)
- `rust/datafusion-wasm/build.py` — WASM build script
- `analytics-web-app/src/lib/datafusion-wasm/` — built WASM artifact + JS glue (output of build.py, checked in)
- `analytics-web-app/src/lib/wasm-engine.ts`
- `analytics-web-app/src/lib/screen-renderers/LocalQueryRenderer.tsx`

**Modified:**
- `rust/Cargo.toml` — add `rust/datafusion-wasm` to `workspace.exclude`
- `rust/analytics-web-srv/src/screen_types.rs` — add LocalQuery variant
- `rust/analytics-web-srv/tests/screen_types_tests.rs` — add LocalQuery test coverage
- `analytics-web-app/package.json` — add `datafusion-wasm` path dependency
- `analytics-web-app/tsconfig.json` — add `datafusion-wasm` path alias
- `analytics-web-app/vite.config.ts` — WASM content-type middleware, Vite alias, `optimizeDeps.exclude`
- `analytics-web-app/src/lib/arrow-stream.ts` — add `fetchQueryIPC()`
- `analytics-web-app/src/lib/screen-renderers/init.ts` — import LocalQueryRenderer
- `analytics-web-app/src/lib/screens-api.ts` — add `'local_query'` to ScreenTypeName
