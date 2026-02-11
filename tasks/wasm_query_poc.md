# WASM Query POC Plan

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

New crate at `rust/datafusion-wasm/`.

```toml
[package]
name = "datafusion-wasm"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
arrow = { version = "...", default-features = false, features = ["ipc", "ffi"] }
datafusion = { version = "...", default-features = false, features = ["sql"] }
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"

[profile.release]
opt-level = "s"
lto = true
```

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

    /// Register Arrow IPC bytes as a named table.
    pub fn register_table(&self, name: &str, ipc_bytes: &[u8]) -> Result<(), JsValue> { ... }

    /// Execute SQL, return IPC bytes.
    pub async fn execute_sql(&self, sql: &str) -> Result<Vec<u8>, JsValue> { ... }

    /// Reset (deregister all tables).
    pub fn reset(&self) { ... }
}
```

No `register_as` parameter on `execute_sql`, no `read_table_ipc` — those are notebook features. The POC only needs register + execute.

Build:

```bash
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/datafusion_wasm.wasm --out-dir pkg --target web
wasm-opt pkg/datafusion_wasm_bg.wasm -Os -o pkg/datafusion_wasm_bg.wasm
```

**This is the spike.** If DataFusion doesn't compile to WASM, we stop here and evaluate DuckDB-WASM as the fallback (see notebook queries plan).

### Step 2: Vite Integration

Add the WASM module to the analytics-web-app build.

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

### Step 3: fetchQueryIPC

Add to `arrow-stream.ts`:

```typescript
export async function fetchQueryIPC(
  params: StreamQueryParams,
  signal?: AbortSignal,
): Promise<Uint8Array> {
  // Same HTTP setup as streamQuery
  // Parse JSON frames, collect raw IPC bytes (skip JS Arrow decoding)
  // Return concatenated Uint8Array
}
```

This is a standalone addition — no changes to existing `streamQuery()` or `executeStreamQuery()`.

### Step 4: Screen Type Registration

**Backend** (`screen_types.rs`):

Add `LocalQuery` variant to `ScreenType` enum, `FromStr`, `all()`, `as_str()`, `info()`, `default_config()`.

**Frontend** (`screens-api.ts`):

Add `'local_query'` to `ScreenTypeName` union.

### Step 5: LocalQueryRenderer

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

  // Fetch source data → register in WASM
  const fetchAndRegister = useCallback(async () => {
    if (!engine) return
    setSourceStatus('loading')
    try {
      const sql = substituteTimeRange(localConfig.sourceSql, timeRange)
      const ipcBytes = await fetchQueryIPC(
        { sql, begin: timeRange.begin, end: timeRange.end, dataSource },
        abortController.signal
      )
      engine.register_table(localConfig.sourceTableName, ipcBytes)
      // Decode IPC to count rows (or get count from engine)
      const table = tableFromIPC(ipcBytes)
      setSourceRowCount(table.numRows)
      setSourceStatus('ready')
    } catch (e) {
      setSourceError(e.message)
      setSourceStatus('error')
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

  // ... SQL editors, result table, save handling ...
}

registerRenderer('local_query', LocalQueryRenderer)
```

Register in `init.ts`:
```typescript
import './LocalQueryRenderer'
```

### Step 6: Result Display

Reuse the existing table rendering components from the Table screen. The local query result is a standard arrow-js `Table` — same as what every other renderer displays.

## What We Learn

| Question | How the POC answers it |
|---|---|
| Does DataFusion compile to WASM? | Step 1 — the spike |
| What's the WASM binary size? | Step 1 — measure gzipped output |
| Does IPC ingestion work? | Step 3+5 — `fetchQueryIPC` → `register_table` |
| Does local SQL execution work? | Step 5 — `execute_sql` against registered table |
| Does IPC output deserialize correctly? | Step 5 — `tableFromIPC` on WASM output |
| What's the latency? | Step 5 — measure register + execute + deserialize |
| Does Vite lazy-loading work? | Step 2 — WASM module loads on demand |
| What DataFusion features work in WASM? | Manual testing — try aggregates, joins, window functions |

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
- `rust/datafusion-wasm/Cargo.toml` + `src/lib.rs`
- `analytics-web-app/src/lib/wasm-engine.ts`
- `analytics-web-app/src/lib/screen-renderers/LocalQueryRenderer.tsx`

**Modified:**
- `analytics-web-app/src/lib/arrow-stream.ts` — add `fetchQueryIPC()`
- `analytics-web-app/src/lib/screen-renderers/init.ts` — import LocalQueryRenderer
- `analytics-web-app/src/lib/screens-api.ts` — add `'local_query'` to ScreenTypeName
- `rust/analytics-web-srv/src/screen_types.rs` — add LocalQuery variant
- `analytics-web-app/vite.config.ts` — WASM plugin config (if needed)
