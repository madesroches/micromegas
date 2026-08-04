# Arrow View Types (Utf8View / BinaryView) in the Web App Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1294

## Overview

Notebook cells fail with `Unrecognized type: "undefined" (24)` whenever a query returns an Arrow
`Utf8View` column — which modern DataFusion produces from ordinary string functions (`LEFT(...)`,
`replace(...)`). The cause is entirely client-side: `analytics-web-app` pins `apache-arrow@^21.1.0`,
which has no notion of Arrow type ids 23/24. Support was merged into arrow-js on 2025-11-19
([apache/arrow-js#320](https://github.com/apache/arrow-js/pull/320)) and shipped in **21.2.0 on
2026-07-21**, so the decode fix is a dependency bump. A second, separate defect survives the bump:
`arrow-utils.ts`'s type predicates don't know the view types either, so a `Utf8View` column would
decode but then be classified as neither numeric nor categorical (silently rejected by charts,
swimlane color-by, and map color-by), and a `BinaryView` column would render as comma-joined byte
numbers instead of the ASCII preview. This plan does both, and adds regression coverage over the two
independent decode paths.

## Current State

### The failure

`analytics-web-app/package.json:28` pins `"apache-arrow": "^21.1.0"`. In the installed 21.1.0, the
`Type` enum (`node_modules/apache-arrow/enum.d.ts:53-54`) stops at `LargeBinary = 19` /
`LargeUtf8 = 20`; there is no `BinaryView = 23` / `Utf8View = 24`, no visitor entry, and no loader,
so the type visitor throws on the schema message before any data is touched. Reproduced directly
against IPC fixtures matching what the server emits (pyarrow-authored `string_view` + `binary_view`
columns, with both inline ≤12-byte values and out-of-line values that populate variadic data
buffers):

| Fixture | apache-arrow 21.1.0 | apache-arrow 21.2.0 |
| --- | --- | --- |
| uncompressed IPC stream | `Unrecognized type: "undefined" (24)` | decodes, values correct |
| LZ4_FRAME-compressed IPC stream | same failure | decodes, values correct |

The LZ4 row is the one that matters for the server path: `IpcWriteOptions` in
`rust/analytics-web-srv/src/stream_query.rs:296-298` sets `CompressionType::LZ4_FRAME`
unconditionally, and view types add variadic buffers that the compression path has to carry.

### The two decode paths (both affected)

1. **Server queries — streaming.** `rust/analytics-web-srv/src/stream_query.rs` frames each IPC
   message in a JSON envelope (`{"type":"schema","size":N}` / `{"type":"batch",...}`);
   `analytics-web-app/src/lib/arrow-stream.ts:238` feeds those bytes into
   `RecordBatchReader.from(ipcByteStream())`, relying on arrow-js to carry dictionary state across
   batches. LZ4 is decoded via the codec registered in `arrow-compression.ts:10`.
2. **Client-side cells — whole buffer.** `lib/screen-renderers/useCellExecution.ts:214,234,252,271`
   calls `tableFromIPC(ipcBytes)` on output from the in-browser `micromegas-datafusion-wasm` crate
   (DataFusion 54.1, `rust/datafusion-wasm/Cargo.toml:20`), which runs the same string functions and
   therefore produces the same view types. This path is reachable with no server involved.

### Type predicates that don't know about view types

`analytics-web-app/src/lib/arrow-utils.ts`:

- `isStringType` (`:139-141`) — `DataType.isUtf8(t) || DataType.isLargeUtf8(t)`. Deliberately does
  **not** unwrap dictionaries (documented at `:129`); callers wrap with `unwrapDictionary`.
- `isBinaryType` (`:159-166`) — `isBinary || isLargeBinary || isFixedSizeBinary`, and **does**
  unwrap dictionaries internally.

Consumers, all of which mis-handle a view column today:

| Site | Effect of the gap |
| --- | --- |
| `arrow-utils.ts:174` (`detectXAxisMode`-style classification) | `Utf8View` → neither `'numeric'` nor `'categorical'` |
| `arrow-utils.ts:235,237` / `:301` | color-by column rejected as unsupported |
| `arrow-utils.ts:282` | `Utf8View` X-axis rejected by the chart validity check |
| `lib/screen-renderers/cells/SwimlaneCell.tsx:89,91` | lane color-by falls through |
| `components/map/overlay.ts:236,238` | marker color-by falls through |
| `lib/screen-renderers/table-utils.tsx:730,782` | `BinaryView` misses the ASCII-preview branch in `formatCell` (`:782-795`) and falls to `String(value)` → `"97,98,99"`; the `:730` tooltip picks the wrong branch too |

Plain table rendering of a `Utf8View` column is already fine — `formatCell` falls through to
`String(value)`, which is correct for a string.

Note that 21.2.0 exposes **no** `DataType.isUtf8View` / `isBinaryView` static predicates (verified:
the set of `View`-matching keys on `DataType` is empty), only the `Utf8View` / `BinaryView`
constructors and the `Type` enum entries. Detection must go through `typeId`.

### What is *not* a workaround for this bug

The issue's scope mentions removing `arrow_cast(..., 'Utf8')` workarounds. The `arrow_cast` calls
already in the repo are **unrelated** and must stay:

- `src/routes/perf-analysis/queries.ts:24,26,28`, `lib/screen-renderers/notebook-utils.ts:158,160,162`,
  and the mirrored doc example at `mkdocs/docs/web-app/notebooks/cell-types.md:624-628` cast
  `property_get(...)`, which returns `Dictionary(_, Utf8)` (`rust/datafusion-extensions/src/properties/property_get.rs:87-90`),
  so `concat` gets plain strings. `blocks.stream_id` is already `DataType::Utf8`
  (`rust/analytics/src/lakehouse/blocks_view.rs:240`), making that one a no-op cast.

No `arrow_cast(..., 'Utf8')` view-type workaround exists in the repo to remove.

### Working-tree state

The dependency bump has already been trialled in the working tree (uncommitted):
`package.json` + `yarn.lock` moved to `apache-arrow@21.2.0`. `yarn test` (64 files / 1273 tests) and
`yarn type-check` pass, and the tree shrinks by 20 packages / ~468 KiB. No source change yet.

## Design

Three independent, small changes; nothing about the wire protocol, the streaming framing, or the
Rust services changes.

### 1. Dependency bump

`apache-arrow@^21.1.0` → `^21.2.0`. This alone fixes both decode paths, since both go through
arrow-js's IPC reader. 21.2.0 also *builds* view vectors (`vectorFromArray([...], new Utf8View())`
round-trips through `tableToIPC`/`tableFromIPC` — verified), which is what lets the fixtures below be
generated in-process instead of checking in binary blobs.

### 2. View-aware type predicates

In `arrow-utils.ts`, add two narrow helpers next to the existing predicates and fold them in, keeping
each predicate's current dictionary semantics unchanged:

```
import { DataType, Type } from 'apache-arrow'   // Type is a new import here

isUtf8ViewType(t)   => t.typeId === Type.Utf8View
isBinaryViewType(t) => t.typeId === Type.BinaryView

isStringType(t) => isUtf8(t) || isLargeUtf8(t) || isUtf8ViewType(t)
isBinaryType(t) => { const i = unwrapDictionary(t)
                     return isBinary(i) || isLargeBinary(i) || isFixedSizeBinary(i)
                            || isBinaryViewType(i) }
```

`typeId` rather than `instanceof Utf8View` — it matches how arrow-js's own `DataType.isX` statics
work, survives duplicated module instances, and needs no value import of the type classes.

Every consumer in the table above then picks up view support with no further edits, because they all
route through these two predicates (open/closed: one definition, six call sites unchanged).

Dictionary-wrapped view types (`Dictionary<_, Utf8View>`) are not something DataFusion emits here;
`isStringType` keeps its documented no-unwrap contract and callers keep wrapping with
`unwrapDictionary` as they do today.

### 3. Out of scope, deliberately

`ListView` / `LargeListView` (ids 22/23-adjacent) are still unimplemented in arrow-js 21.2.0 and are
not produced by the functions in this issue; no attempt to handle them.

## Implementation Steps

1. **Bump the dependency.** `analytics-web-app/`: `yarn up apache-arrow@^21.2.0` (already present in
   the working tree — verify `package.json:28` reads `^21.2.0` and `yarn.lock` resolves 21.2.0).
2. **Make the predicates view-aware.** `src/lib/arrow-utils.ts`: add `Type` to the `apache-arrow`
   import, add `isUtf8ViewType` / `isBinaryViewType`, extend `isStringType` (`:139-141`) and
   `isBinaryType` (`:159-166`). Update the doc comments on both to name the view types.
3. **Fixtures.** `src/lib/__tests__/arrow-ipc-fixtures.ts`: add a `createViewTypeFramedIpc(...)`
   alongside `createDictionaryFramedIpc` (`:103`) and `createPlainFramedIpc` (`:154`), reusing the
   existing `splitIpcMessages` framing. Build the table with `vectorFromArray(values, new Utf8View())`
   and a `BinaryView` column, and include **both** a short inline value (≤12 bytes) and a long
   out-of-line value (>12 bytes, which forces a variadic data buffer) plus a null — that trio is what
   distinguishes a real decode from a lucky one.
4. **Streaming-path test.** New `src/lib/__tests__/arrow-stream-view-types.test.ts` (or a
   `describe` block in `arrow-stream-dictionary.test.ts`, which already has the mock-fetch/
   `createMockStream` harness): drive `streamQuery` over the step-3 fixture split across chunk
   boundaries, assert the schema frame arrives with `Utf8View`/`BinaryView` fields and that the batch
   values round-trip. This is the direct regression test for the reported error.
5. **Whole-buffer-path test.** In `src/lib/screen-renderers/__tests__/useCellExecution.test.ts`, add a
   case whose mocked wasm result is view-typed IPC, asserting `tableFromIPC` succeeds — the
   `datafusion-wasm` path is not covered by step 4 and cannot be fixed server-side.
6. **Predicate tests.** `src/lib/__tests__/arrow-utils.test.ts`: `isStringType(new Utf8View())` and
   `isBinaryType(new BinaryView())` true; the column classification at `:174` returns `'categorical'`
   for `Utf8View`; the chart-validity check at `:282` accepts a `Utf8View` X column.
7. **`formatCell` test.** `src/lib/screen-renderers/__tests__/table-utils.test.tsx`: a `BinaryView`
   column formats as the ASCII preview with length, not `"97,98,99"`.
8. **CHANGELOG.** Add a `## Unreleased` → `**Web App:**` bullet: view-type decode fix via the
   arrow-js bump, plus the predicate extension, referencing #1294.

## Files to Modify

- `analytics-web-app/package.json` — dependency bump (done, uncommitted)
- `analytics-web-app/yarn.lock` — lockfile (done, uncommitted)
- `analytics-web-app/src/lib/arrow-utils.ts` — view-aware `isStringType` / `isBinaryType` + helpers
- `analytics-web-app/src/lib/__tests__/arrow-ipc-fixtures.ts` — view-type fixture
- `analytics-web-app/src/lib/__tests__/arrow-stream-view-types.test.ts` — new (streaming path)
- `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts` — predicate + classification cases
- `analytics-web-app/src/lib/screen-renderers/__tests__/useCellExecution.test.ts` — wasm path case
- `analytics-web-app/src/lib/screen-renderers/__tests__/table-utils.test.tsx` — `formatCell` case
- `CHANGELOG.md` — Unreleased / Web App entry

No Rust changes. No changes to `arrow-stream.ts`, `arrow-compression.ts`, or `stream_query.rs`.

## Trade-offs

- **Migrate to `@uwdata/flechette` (issue #1307, now closed as not planned).** Verified that
  flechette 2.5.0 does decode both view types and has `setCompressionCodec` for LZ4. Rejected as a
  fix for this bug: 57 files in `analytics-web-app` import `apache-arrow` with incompatible APIs (no
  `DataType.isX` predicates — types are plain objects; different `Table`/`Column` shape;
  `tableFromArrays`/`vectorFromArray`/`tableToIPC` differences), and flechette decodes **whole
  buffers only** — `arrow-stream.ts:238`'s `RecordBatchReader.from(asyncIterable)` and its
  cross-batch dictionary state (covered by `arrow-stream-dictionary.test.ts`) would have to be
  reimplemented. The issue's premise — that arrow-js had gone 9+ months without a release — expired
  when 21.2.0 shipped. Still defensible later on bundle-size grounds, on its own merits.
- **`datafusion.optimizer.expand_views_at_output = true` server-side.** One config line; coerces
  `Utf8View` → `LargeUtf8` and `BinaryView` → `LargeBinary`
  (`datafusion-common-54.1.0/src/config.rs:1284-1287`), both decodable even by 21.1.0. Rejected: it
  cannot reach the in-browser `datafusion-wasm` path (its own `SessionContext` is built at
  `rust/datafusion-wasm/src/lib.rs:43`), it changes the schema every FlightSQL client sees —
  including pyarrow and the CLI, which decode views fine today — and it forces a copy. Fixing the
  decoder fixes all consumers at once; this fixes one at everyone's expense.
- **Cast view → `Utf8`/`Binary` per batch in `stream_query.rs`.** Confines the schema change to the
  web path, but still misses the wasm path and adds a per-batch copy on the hot streaming path.
- **`yarn patch` on 21.1.0.** Was the only option before 21.2.0 existed; pointless now.

## Documentation

`CHANGELOG.md` only. No documentation page describes the web app's supported Arrow types or an
`arrow_cast` view-type workaround (greps over `doc/` and `mkdocs/docs/` for `Utf8View`/`string_view`
found nothing relevant; `analytics-web-app/README.md` doesn't mention `apache-arrow`). The
`arrow_cast` occurrences in `mkdocs/docs/web-app/notebooks/cell-types.md:624-628` are the
dictionary-unwrapping swimlane example described in Current State and must be left alone.

## Testing Strategy

- **Gate**: `python3 build/analytics_web_ci.py` from the repo root — `yarn install` → `type-check` →
  `lint` → `test` → `build`, byte-for-byte what `.github/workflows/analytics-web-app.yml` runs.
  Baseline before the change is 64 files / 1273 tests green on 21.2.0, so any new red is from this
  branch's source edits.
- **Automated regression**: steps 4-7 above. Step 4 covers the exact reported failure over the real
  framing and chunk boundaries; step 5 covers the path a server-side fix could not have reached.
- **Manual repro (required)**: with services up (`python3 local_test_env/ai_scripts/start_services.py`)
  run the issue's query in a notebook cell —
  `SELECT LEFT(replace(msg, chr(10), ' '), 32) AS name FROM log_entries` — and confirm it renders
  with no `arrow_cast`. Before the fix this is the reported error; after, it should show strings.
- **Manual check on classification**: chart or swimlane-color-by that same `Utf8View` column to
  confirm step 2 took effect (a decode-only fix would leave the column silently unsupported).
- **Cross-client no-regression**: the same query through `micromegas-query` should be unchanged —
  this plan touches no server behavior, which is precisely the argument against the
  `expand_views_at_output` alternative.

## Open Questions

None blocking. One thing to keep an eye on: whether other Arrow types DataFusion 54 can return
(`RunEndEncoded`, `ListView`) show up in practice — arrow-js 21.2.0 still doesn't decode them, and
they would surface as the same `Unrecognized type` error. Out of scope here; worth its own issue if
it ever appears.
