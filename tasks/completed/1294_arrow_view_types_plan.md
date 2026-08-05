# Arrow View Types (Utf8View / BinaryView) in the Web App Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1294

## Overview

Notebook cells fail with `Unrecognized type: "undefined" (24)` whenever a query returns an Arrow
`Utf8View` column — which modern DataFusion produces from ordinary string functions (`LEFT(...)`,
`replace(...)`). The cause is entirely client-side: `analytics-web-app` pinned `apache-arrow@^21.1.0`,
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
2. **Client-side cells — whole buffer.** `lib/screen-renderers/useCellExecution.ts` calls
   `tableFromIPC(ipcBytes)` at four sites over two different byte sources: `:214,252` decode
   uncompressed output from the in-browser `micromegas-datafusion-wasm` crate (DataFusion 54.1,
   `rust/datafusion-wasm/Cargo.toml:20`, uncompressed because `rust/datafusion-wasm/src/lib.rs:183`
   uses `StreamWriter::try_new`), while `:234,271` decode the result of `fetchQueryIPC(...)`, which
   collects the same framed `/query-stream` server response as path 1 and is therefore
   LZ4_FRAME-compressed exactly like `stream_query.rs:296-297`. Both call shapes run the same string
   functions and therefore produce the same view types; the wasm half is reachable with no server
   involved, the `fetchQueryIPC` half is not.

### Type predicates that don't know about view types

`analytics-web-app/src/lib/arrow-utils.ts`:

- `isStringType` (`:139-141`) — `DataType.isUtf8(t) || DataType.isLargeUtf8(t)`. Deliberately does
  **not** unwrap dictionaries (documented at `:129`); callers wrap with `unwrapDictionary`.
- `isBinaryType` (`:159-166`) — `isBinary || isLargeBinary || isFixedSizeBinary`, and **does**
  unwrap dictionaries internally.

Consumers, all of which mis-handle a view column today:

| Site | Effect of the gap |
| --- | --- |
| `arrow-utils.ts:235,237` / `:301` | color-by column rejected as unsupported |
| `arrow-utils.ts:282` | `Utf8View` X-axis rejected by `validateChartColumns`'s validity check — `detectXAxisMode` (`:171-177`) already defaults unrecognized types to `'categorical'`, so it isn't the classification that fails here |
| `lib/screen-renderers/cells/SwimlaneCell.tsx:89,91` | lane color-by falls through |
| `components/map/overlay.ts:236,238` | marker color-by falls through |
| `lib/screen-renderers/table-utils.tsx:730,782` | `BinaryView` misses the ASCII-preview branch in `formatCell` (`:782-795`) and falls to `String(value)` → `"97,98,99"`; the `:730` tooltip picks the wrong branch too |

Plain table rendering of a `Utf8View` column is already fine — `formatCell` falls through to
`String(value)`, which is correct for a string.

Note that 21.2.0 *does* expose `DataType.isUtf8View` / `isBinaryView` static predicates
(`node_modules/apache-arrow/type.d.ts:50,53`), implemented as `x?.typeId === Type.Utf8View` /
`Type.BinaryView` — the same `typeId` check the fix below needs, already written. No custom helper
or `Type` import required.

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

The dependency bump already landed on this branch in commit `b4aef408a` ("Bump apache-arrow to
21.2.0 for Utf8View/BinaryView decode support (#1294)"): `package.json` + `yarn.lock` moved to
`apache-arrow@21.2.0`. `yarn test` (64 files / 1273 tests) and `yarn type-check` pass, and the tree
shrinks by 20 packages / ~468 KiB. No source change yet.

## Design

Three independent, small changes; nothing about the wire protocol, the streaming framing, or the
Rust services changes.

### 1. Dependency bump

`apache-arrow@^21.1.0` → `^21.2.0`. This alone fixes both decode paths, since both go through
arrow-js's IPC reader. 21.2.0 also *builds* view vectors (`vectorFromArray([...], new Utf8View())`
round-trips through `tableToIPC`/`tableFromIPC` — verified), which is what lets the fixtures below be
generated in-process instead of checking in binary blobs.

### 2. View-aware type predicates

In `arrow-utils.ts`, extend the two existing predicates with arrow-js's own view-type statics — no
new helpers, no new import:

```
isStringType(t) => DataType.isUtf8(t) || DataType.isLargeUtf8(t) || DataType.isUtf8View(t)
isBinaryType(t) => { const i = unwrapDictionary(t)
                     return DataType.isBinary(i) || DataType.isLargeBinary(i)
                            || DataType.isFixedSizeBinary(i) || DataType.isBinaryView(i) }
```

`DataType.isUtf8View` / `DataType.isBinaryView` are the same `typeId` check a hand-rolled helper
would need, already implemented and exported — consistent with every other predicate in this file,
which all go through `DataType.isX`.

Every consumer in the table above then picks up view support with no further edits, because they all
route through these two predicates (open/closed: one definition, six call sites unchanged).

Dictionary-wrapped view types (`Dictionary<_, Utf8View>`) are not something DataFusion emits here;
`isStringType` keeps its documented no-unwrap contract and callers keep wrapping with
`unwrapDictionary` as they do today.

### 3. Out of scope, deliberately

`RunEndEncoded` (id 22), `ListView` (id 25), and `LargeListView` (id 26) have no entry at all in
arrow-js 21.2.0's `Type` enum (`node_modules/apache-arrow/enum.d.ts:55-57` jumps from
`LargeList = 21` straight to `BinaryView = 23`) and are not produced by the functions in this issue;
no attempt to handle them.

## Implementation Steps

1. **Bump the dependency (already landed).** Done in commit `b4aef408a` — verify `package.json:28`
   reads `^21.2.0` and `yarn.lock` resolves 21.2.0; no further edit needed here.
2. **Make the predicates view-aware.** `src/lib/arrow-utils.ts`: extend `isStringType` (`:139-141`)
   with `DataType.isUtf8View(t)` and `isBinaryType` (`:159-166`) with `DataType.isBinaryView(inner)`
   — both already imported via `DataType`, no new import needed. Update the doc comments on both to
   name the view types. This changes the branch every call in `arrow-utils.test.ts` exercises through
   its hand-rolled `apache-arrow` mock, which has no `isUtf8View`/`isBinaryView` statics — see step 6
   for the required mock update, which must land together with this step so the existing suite keeps
   passing.
3. **Fixtures.** `src/lib/__tests__/arrow-ipc-fixtures.ts`: add
   `createViewTypeIpc({ compressed }: { compressed: boolean }) => { raw, chunks }` alongside
   `createDictionaryFramedIpc` (`:103`) and `createPlainFramedIpc` (`:154`) — the `compressed` flag
   picks LZ4_FRAME vs. no compression for that single call, so step 4 and step 5 each call it twice
   (once with `compressed: false`, once with `compressed: true`) to get all four artifacts they need.
   The returned `chunks` reuse the existing `splitIpcMessages` framing for the streaming-path test
   (step 4), while `raw` is what step 5 needs directly, since `combineChunks` concatenates the JSON
   frame lines together with the message bytes and the result is not valid input to `tableFromIPC`.
   Build the table with
   `vectorFromArray(values, new Utf8View())` and a `BinaryView` column, and include **both** a short
   inline value (≤12 bytes) and a long out-of-line value (>12 bytes, which forces a variadic data
   buffer) plus a null — that trio is what distinguishes a real decode from a lucky one. Emit through
   `RecordBatchStreamWriter.writeAll(table, compressed ? { compressionType: CompressionType.LZ4_FRAME } : {}).toUint8Array(true)`
   — the constructor only takes an options object, not the table; `writeAll` is the static entry point
   that builds and finishes the writer, and is verified working end-to-end through the existing
   `splitIpcMessages` framing. Register a single codec on `compressionRegistry` covering both
   directions, `{ encode: lz4js.compress, decode: lz4js.decompress }`, **inside the fixture factory
   function itself** (not at module scope): `compressionRegistry.set()` replaces the whole entry
   rather than merging, and `arrow-stream.ts:11`'s `import './arrow-compression'` registers a
   decode-only codec — if the fixture registered at module scope, whichever import runs last would
   win, and a test file that imports `../arrow-stream` after `./arrow-ipc-fixtures` would get the
   decode-only entry and hit `Codec for compression type "LZ4_FRAME" has invalid encode method` when
   the writer tries to encode. Registering inside the factory (called from each test, or from a
   `beforeEach`/`beforeAll`) guarantees the fixture's `{ encode, decode }` pair always applies last,
   regardless of import order. arrow-js computes the variadic-buffer length prefixes itself;
   `lz4js.compress` and `lz4js.decompress` are already declared in `src/types/lz4js.d.ts`. The
   `compressed: true` call matches what `stream_query.rs:296-297` actually produces; the
   `compressed: false` call covers the plain-IPC case (the uncompressed wasm output in step 5).
4. **Streaming-path test.** New `src/lib/__tests__/arrow-stream-view-types.test.ts` (or a
   `describe` block in `arrow-stream-dictionary.test.ts`, which already has the mock-fetch/
   `createMockStream` harness): drive `streamQuery` over both `createViewTypeIpc({ compressed: false }).chunks`
   and `createViewTypeIpc({ compressed: true }).chunks` split across chunk boundaries, assert the
   schema frame arrives with `Utf8View`/
   `BinaryView` fields and that the batch values round-trip through the LZ4 codec registered by the
   step-3 fixture (which wraps the same `lz4js.decompress` `arrow-compression.ts:10` uses for decode —
   note the fixture's `compressionRegistry.set()` call replaces that production registration for the
   duration of the test, since `set()` overwrites rather than merges). This is the direct regression
   test for the reported error over the exact framing and compression the server uses.
5. **Whole-buffer-path test.** In `src/lib/__tests__/arrow-ipc-fixtures.test.ts` (or alongside it),
   add two cases that call the real `tableFromIPC` directly on the raw (unframed) IPC bytes returned
   by step 3's fixture — one over `createViewTypeIpc({ compressed: false }).raw`, one over
   `createViewTypeIpc({ compressed: true }).raw` — and assert the decode
   succeeds and values round-trip in both. This must live where `apache-arrow` is not mocked:
   `useCellExecution.test.ts:35-65` has a blanket `vi.mock('apache-arrow', …)` whose
   `tableFromIPC: () => new MockTable([{}])` ignores its input entirely, so a case added there would
   assert nothing about view decoding. Together the two cases cover both whole-buffer call shapes in
   `useCellExecution.ts` — uncompressed `datafusion-wasm` output (`:214,252`) and LZ4-compressed
   server output collected via `fetchQueryIPC` (`:234,271`) — neither of which is covered by step 4's
   streaming-reader test, and neither of which can be fixed server-side; the assertion has to run
   against the real library to mean anything.
6. **Mock update, then predicate tests.** `src/lib/__tests__/arrow-utils.test.ts`: first extend the
   hand-rolled `apache-arrow` mock (`:6-99`) — add `Utf8View: 14, BinaryView: 15` to its `TypeId` map,
   `static isUtf8View`/`isBinaryView` to `MockDataType` mirroring the existing statics, and
   `createUtf8ViewType`/`createBinaryViewType` factories exported alongside the others in `__test__`.
   Without this the step-2 predicate change throws `DataType.isUtf8View is not a function` in every
   existing test whose type falls through the `||` chain (int, float, decimal, bool, dictionary,
   etc.) — the mock has no such statics today. Then add cases using that mock's own factory
   convention (the mock exports no `Utf8View`/`BinaryView` classes, so `new Utf8View()` is not
   constructible here): `isStringType(__test__.createUtf8ViewType())` and
   `isBinaryType(__test__.createBinaryViewType())` true; the chart-validity check at `:282` accepts a
   `Utf8View` X column; the color-kind checks at `:235,237`/`:301` accept a `Utf8View`/`BinaryView`
   color-by column — these are the assertions that actually flip with the fix (`detectXAxisMode`'s
   `'categorical'` default already passes today, with or without it).
7. **`formatCell` test.** `src/lib/screen-renderers/__tests__/table-utils.test.tsx`: a `BinaryView`
   column formats as the ASCII preview with length, not `"97,98,99"`.
8. **CHANGELOG.** Add a `## Unreleased` → `**Web App:**` bullet: view-type decode fix via the
   arrow-js bump, plus the predicate extension, referencing #1294.

## Files to Modify

- `analytics-web-app/package.json` — dependency bump (done, commit `b4aef408a`)
- `analytics-web-app/yarn.lock` — lockfile (done, commit `b4aef408a`)
- `analytics-web-app/src/lib/arrow-utils.ts` — view-aware `isStringType` / `isBinaryType`
- `analytics-web-app/src/lib/__tests__/arrow-ipc-fixtures.ts` — view-type fixture (compressed + uncompressed)
- `analytics-web-app/src/lib/__tests__/arrow-ipc-fixtures.test.ts` — whole-buffer `tableFromIPC` cases, uncompressed (wasm-path) and compressed (server-path via `fetchQueryIPC`), real library
- `analytics-web-app/src/lib/__tests__/arrow-stream-view-types.test.ts` — new (streaming path)
- `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts` — predicate + classification cases
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

**Follow-up.** Watch whether other Arrow types DataFusion 54 can return (`RunEndEncoded`, `ListView`,
`LargeListView`) show up in practice — arrow-js 21.2.0 still doesn't decode them (see Design §3), and
they would surface as the same `Unrecognized type` error. Out of scope here; worth its own issue if
it ever appears.

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

None.
