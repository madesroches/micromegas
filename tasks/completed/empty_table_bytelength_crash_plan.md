# Fix: Empty Table byteLength Crash in Notebook Execution

## Overview

When a notebook cell query returns 0 rows, the app crashes with `TypeError: Cannot read properties of undefined (reading 'byteLength')`. The crash occurs in the status text builder that computes total byte size of query results by iterating Arrow RecordBatch internal buffers. When the WASM DataFusion engine (Rust arrow v57.3) produces IPC bytes for a 0-row result and the JS Apache Arrow library (v21.x) deserializes them, the resulting `Data` objects may contain undefined buffer entries, causing the `Data.byteLength` getter to crash.

## Current State

### Crash site

`analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts` lines 72-76:

```typescript
const totalBytes = state.data.reduce(
  (sum, t) =>
    sum + t.batches.reduce((s: number, b) => s + b.data.byteLength, 0),
  0,
)
```

The same pattern appears at lines 164-168 in `buildHgStatusText()`.

### Reproduction flow

1. A remote query returns 0 rows (schema only, ~648 bytes of IPC)
2. `fetchQueryIPC()` collects the IPC stream: schema message + EOS marker, no batch messages
3. `engine.register_table('Table', ipcBytes)` in the WASM engine registers a 0-row table (logs: `registered table 'Table': 0 rows`)
4. `tableFromIPC(ipcBytes)` creates a JS Arrow `Table`
5. The cell completes with `state.data = [table]` where `table.numRows === 0`
6. `buildStatusText()` enters the `state.data.length > 0` branch (line 70)
7. Inner reduce iterates `t.batches` — the 0-row table may contain a batch whose `Data.byteLength` getter crashes because its internal `buffers` or `children` array contains `undefined` entries

### Stack trace

```
TypeError: Cannot read properties of undefined (reading 'byteLength')
    at get byteLength (arrow-utils-*.js)     ← Arrow Data.byteLength getter
    at Array.reduce
    at get byteLength (arrow-utils-*.js)     ← recursive: Data iterates children
    at ScreenPage-*.js                       ← buildStatusText / buildHgStatusText
```

### Why the buffers are undefined

The Rust Arrow IPC writer (arrow v57.3) and the JS Apache Arrow reader (v21.x) differ in how they handle 0-row batches. When the IPC stream contains a schema but no record batches — or a 0-row batch with complex/nested types — the JS deserializer may create `Data` objects with uninitialized buffer slots. The `Data.byteLength` getter iterates all buffers and children via `reduce`, hitting `undefined.byteLength`.

### Related paths

- `useCellExecution.ts:79` — `new Table()` for empty streaming results (this path is safe: empty `batches` array)
- `useCellExecution.ts:199-200` — WASM `execute_and_register` → `tableFromIPC` (affected path)
- `useCellExecution.ts:218-220` — remote fetch → `register_table` + `tableFromIPC` (affected path)

## Design

### Approach: Safe byteLength accessor

Add a helper function that safely computes byte size for an Arrow Table, guarding against undefined `Data` buffers. Use it in both `buildStatusText` and `buildHgStatusText`.

```typescript
function safeTableByteLength(table: Table): number {
  if (table.numRows === 0) return 0
  return table.batches.reduce((sum, batch) => sum + batch.data.byteLength, 0)
}
```

The `numRows === 0` early return is the primary guard — if there are no rows, byte size is functionally meaningless (it's only displayed for user context). The crash only affects 0-row tables where the WASM/JS Arrow version mismatch produces uninitialized buffers, so no try/catch fallback is needed for tables with rows.

## Implementation Steps

1. **Add `safeTableByteLength` helper** to `notebook-cell-view.ts` (in the "Formatting helpers" section, ~line 36)

2. **Replace raw byteLength computation in `buildStatusText`** (line 72-76):
   ```typescript
   const totalBytes = state.data.reduce((sum, t) => sum + safeTableByteLength(t), 0)
   ```

3. **Replace raw byteLength computation in `buildHgStatusText`** (line 164-168):
   ```typescript
   totalBytes += state.data.reduce((sum, t) => sum + safeTableByteLength(t), 0)
   ```

4. **Update tests** in `__tests__/notebook-cell-view.test.ts`:
   - Add a test for `buildStatusText` with a 0-row table that has malformed batch data
   - Add a test for `buildHgStatusText` with a 0-row child cell
   - Update the `makeTable` helper to support a 0-row variant

5. **Update HorizontalGroupCell tests** if they use the same byteLength pattern (line 264 of `cells/__tests__/HorizontalGroupCell.test.tsx` — indirect, uses mocks, likely fine as-is)

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts` | Add `safeTableByteLength`, use in both status text builders |
| `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-cell-view.test.ts` | Add 0-row table crash regression tests |

## Trade-offs

### Alternative: Fix at the `tableFromIPC` call site

Instead of guarding the byte computation, we could intercept the result of `tableFromIPC` and return `new Table()` when the result has 0 rows. This would prevent the malformed `Data` from ever reaching the rest of the app.

**Rejected because**: The `Data` objects are internal to Apache Arrow and may be used elsewhere in the app without issue (e.g., rendering column headers for empty results). Replacing the Table wholesale could break schema-dependent features. The issue is specifically in `byteLength` computation.

### Alternative: Upgrade Apache Arrow JS

The buffer initialization behavior may be fixed in newer Arrow JS versions.

**Rejected as sole fix**: Version upgrades are risky (breaking API changes) and the byteLength code should be defensive regardless of library version.

## Testing Strategy

- [ ] Run existing tests: `cd analytics-web-app && yarn test`
- [ ] Add regression test: 0-row Table with `batches: [{ data: { get byteLength() { throw new TypeError() } } }]`
- [ ] Manual test: create a notebook with a query that returns 0 rows (e.g., `SELECT * FROM table WHERE 1=0`)
- [ ] Verify status text shows "0 rows (0 B)" instead of crashing
