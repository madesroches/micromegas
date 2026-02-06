# Response Compression for analytics-web-srv

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/781
**Status**: Complete

## Problem
Corporate proxy kills connections when generating large Perfetto traces (~255 MB body size), causing `net::ERR_CONNECTION_CLOSED`.

## What was done

1. **Gzip compression** — `tower-http` `CompressionLayer` on the router, all endpoints compressed transparently
2. **Client-side trace generation** — eliminated the `generate_trace` server endpoint entirely; the client now queries `perfetto_trace_chunks()` directly via `streamQuery` and concatenates the binary chunks client-side
3. **Shared download helper** — `triggerTraceDownload` in `perfetto-trace.ts` used by both `PerfettoExportCell` and `PerformanceAnalysisPage`
4. **Abort signal support** — both trace fetch callers wire `AbortController` for cancellation on re-trigger and unmount
5. **Dead code cleanup** — removed orphaned `queries.rs`, unused types (`GenerateTraceRequest`, `BinaryStartMarker`, `ProgressUpdate`), and the `generateTrace` API function
6. **postMessage fix** — suppressed console errors during Perfetto handshake by using `'*'` targetOrigin for non-sensitive PING messages

### Files changed
- `analytics-web-app/src/lib/perfetto-trace.ts` — new: `fetchPerfettoTrace` + `triggerTraceDownload`
- `analytics-web-app/src/lib/__tests__/perfetto-trace.test.ts` — new: unit tests
- `analytics-web-app/src/lib/screen-renderers/cells/PerfettoExportCell.tsx` — uses new API + abort signal
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/PerfettoExportCell.test.tsx` — updated mocks
- `analytics-web-app/src/routes/PerformanceAnalysisPage.tsx` — uses new API + abort signal
- `analytics-web-app/src/lib/api.ts` — removed `generateTrace`
- `analytics-web-app/src/lib/perfetto.ts` — postMessage fix
- `analytics-web-app/src/types/index.ts` — removed unused types
- `rust/analytics-web-srv/src/main.rs` — removed generate_trace/get_trace_info endpoints, added CompressionLayer
- `rust/analytics-web-srv/src/queries.rs` — deleted (orphaned)
- `rust/Cargo.toml` — added `compression-gzip` feature to `tower-http`
