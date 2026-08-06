# Property Timeline Object Flattening Plan

## Overview

Object-valued properties in the per-process metric property timeline (e.g. CloudWatch `Dimensions`, a nested `{DBInstanceIdentifier: "..."}` object) currently render as the literal string `[object Object]`, because the timeline-building code calls `String(value)` on whatever value sits at the selected property key. The fix: when a property's value is an object, expand it into multiple string-valued properties (dot-separated key paths) at parse time, instead of trying to render the object as a single scalar. This makes each nested field (e.g. `Dimensions.DBInstanceIdentifier`) independently selectable and readable in the property picker and timeline, and applies generically to any object-valued property, not just `Dimensions`.

## Current State

Properties arrive as a JSONB column, exposed to the frontend as a JSON string, and parsed with `JSON.parse` in three places that each build a `Map<number, Record<string, unknown>>` of per-timestamp property objects:

1. `analytics-web-app/src/lib/property-utils.ts:26` — `extractPropertiesFromRows`, shared by:
   - `analytics-web-app/src/lib/screen-renderers/cells/PropertyTimelineCell.tsx:52,55`
   - `analytics-web-app/src/routes/perf-analysis/PerformanceMetricsChart.tsx:127,204`
2. `analytics-web-app/src/hooks/useMetricsData.ts:109` — inline `JSON.parse(String(propsStr))`, used by `PerformanceMetricsChart.tsx` (via `useMetricsData`).
3. `analytics-web-app/src/routes/ProcessMetricsPage.tsx:278` — inline `JSON.parse(String(row.properties))`, this page's own copy of the same logic.

All three feed a getter (`createPropertyTimelineGetter` in `property-utils.ts:45`, or the near-identical inline copies in `useMetricsData.ts:143` and `ProcessMetricsPage.tsx:186`) that, for a selected property name, does:

```ts
const value = props[propertyName]
if (value !== undefined && value !== null) {
  rows.push({ time, value: String(value) })
}
```

When `value` is a plain object (e.g. `Dimensions`), `String(value)` produces `"[object Object]"`. That string is stored as the segment value and rendered verbatim by `PropertyTimeline.tsx:322` (segment label) and `:356` (tooltip).

The available-keys list (`availableKeys` in `extractPropertiesFromRows`, and the equivalent `availablePropertyKeys` memos in `useMetricsData.ts:128` and `ProcessMetricsPage.tsx:177`) is derived via `Object.keys(props)` on the same raw per-timestamp objects, so today it lists `Dimensions` as one opaque key with no way to drill into its contents.

On the ingestion side (`rust/analytics/src/lakehouse/otel/attrs.rs:39-76`), OTel `KvlistValue` attributes are already recursively converted into real nested JSON objects — this is a display-only bug, not a data problem.

## Design

Add a single shared utility, `flattenProperties`, to `analytics-web-app/src/lib/property-utils.ts`, and call it at each of the three JSON-parsing sites, right after `JSON.parse`, before the result is stored in the `rawData` / `rawPropertiesData` map. This keeps the fix in one place conceptually (one function, three call sites) rather than duplicating flattening logic.

```ts
/**
 * Expands object-valued properties into dot-separated leaf entries with
 * string values (e.g. `Dimensions: {DBInstanceIdentifier: "foo"}` becomes
 * `"Dimensions.DBInstanceIdentifier": "foo"`). Non-object values (including
 * arrays) pass through unchanged at the top level, preserving existing
 * scalar-formatting behavior in the timeline getters.
 */
export function flattenProperties(props: Record<string, unknown>): Record<string, unknown> {
  const result: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(props)) {
    if (isPlainObject(value)) {
      flattenObjectInto(value, key, result)
    } else {
      result[key] = value
    }
  }
  return result
}

function flattenObjectInto(obj: Record<string, unknown>, prefix: string, result: Record<string, unknown>): void {
  for (const [key, value] of Object.entries(obj)) {
    const fullKey = `${prefix}.${key}`
    if (isPlainObject(value)) {
      flattenObjectInto(value, fullKey, result)
    } else {
      result[fullKey] = Array.isArray(value) ? JSON.stringify(value) : String(value)
    }
  }
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}
```

Design notes:
- Recursion handles arbitrarily nested objects (e.g. `Dimensions.Nested.Key`), not just one level.
- Arrays are treated as opaque leaves, not expanded — the issue is scoped to object values, and there's no natural key to expand an array element under. A nested array gets `JSON.stringify`'d rather than `String()`'d, to avoid a lesser version of the same unreadable-output problem (`String([1,2])` → `"1,2"` loses structure for non-primitive elements).
- Top-level (non-nested) scalar values are left as their original type (`unknown`), unchanged — `createPropertyTimelineGetter` and its duplicates already handle scalar→string conversion for the getter's return value, and existing tests rely on that behavior with numbers (see `property-utils.test.ts:188`). Only values produced by expanding an object are pre-stringified, since those are the "set of string→string properties" this feature introduces.
- No changes needed in `PropertyTimeline.tsx` or `PropertyTimelineData`/`PropertySegment` types — by the time a value reaches those, it's already a plain string via the existing getter path.

### Call sites

1. `property-utils.ts` `extractPropertiesFromRows` (line 26): change
   `rawData.set(row.time, props)` to `rawData.set(row.time, flattenProperties(props))`, and keep deriving `availableKeys` from the (now flattened) stored object. This single change covers `PropertyTimelineCell.tsx` and `PerformanceMetricsChart.tsx` for free, since both go through `extractPropertiesFromRows`.
2. `useMetricsData.ts` (line 109): wrap the parsed value —
   `propsMap.set(time, flattenProperties(JSON.parse(String(propsStr))))`. Import `flattenProperties` from `@/lib/property-utils`.
3. `ProcessMetricsPage.tsx` (line 278): same change —
   `propsMap.set(time, flattenProperties(JSON.parse(String(row.properties))))`. Import `flattenProperties` from `@/lib/property-utils`.

No signature or type changes are needed to `createPropertyTimelineGetter`, `aggregateIntoSegments`, `ExtractedPropertyData`, or the `Map<number, Record<string, unknown>>` types used across these files — `flattenProperties` is `Record<string, unknown> -> Record<string, unknown>`, so it slots into the existing pipeline without touching downstream signatures.

## Implementation Steps

1. Add `flattenProperties` (plus the private `flattenObjectInto` / `isPlainObject` helpers) to `analytics-web-app/src/lib/property-utils.ts`, exported alongside the existing functions.
2. Update `extractPropertiesFromRows` in `property-utils.ts` to flatten each row's parsed properties before storing them in `rawData`.
3. Update `useMetricsData.ts:109` to flatten the parsed properties before `propsMap.set`.
4. Update `ProcessMetricsPage.tsx:278` to flatten the parsed properties before `propsMap.set`.
5. Add unit tests to `property-utils.test.ts` for `flattenProperties`: flat passthrough for scalars, one level of object expansion (the `Dimensions` case from the issue), multi-level nesting, array values left as opaque (stringified) leaves, and an `extractPropertiesFromRows` test asserting a `Dimensions` object produces `Dimensions.<key>` entries in `availableKeys` instead of `[object Object]`.
6. Manually verify: start services, ingest or query existing CloudWatch-streamed metrics with a `Dimensions` property, open the process metrics page, select the flattened dimension key in the property picker, and confirm the timeline segment/tooltip shows the actual dimension value instead of `[object Object]`.

## Files to Modify

- `analytics-web-app/src/lib/property-utils.ts` — add `flattenProperties` + helpers; call it in `extractPropertiesFromRows`.
- `analytics-web-app/src/hooks/useMetricsData.ts` — call `flattenProperties` after `JSON.parse`.
- `analytics-web-app/src/routes/ProcessMetricsPage.tsx` — call `flattenProperties` after `JSON.parse`.
- `analytics-web-app/src/lib/__tests__/property-utils.test.ts` — new tests for `flattenProperties` and updated `extractPropertiesFromRows` behavior.

## Trade-offs

- **Where to flatten**: flattening at parse time (chosen) vs. inside the per-key getter (`createPropertyTimelineGetter` and its duplicates). Flattening at parse time is necessary because the property *picker* needs to list `Dimensions.DBInstanceIdentifier` as a selectable key — that list comes from `Object.keys()`/`availableKeys` over the raw parsed map, which only sees flattened names if flattening happens before that enumeration. Flattening inside the getter would fix the tooltip/segment text but leave the picker showing an unselectable `Dimensions` entry.
- **Not consolidating the three duplicate `getPropertyTimeline`/parsing implementations**: `useMetricsData.ts` and `ProcessMetricsPage.tsx` each hand-roll logic nearly identical to `property-utils.ts`'s `createPropertyTimelineGetter`/`extractPropertiesFromRows`, inside effects with different streaming/incremental-update wiring. Consolidating them into a single shared implementation would be a larger, riskier refactor than this bug fix calls for. This plan keeps the fix minimal and DRY only for the *new* logic (one `flattenProperties` function, three call sites) — full consolidation of the pre-existing duplication is a separate potential cleanup, not in scope here.
- **Arrays not expanded**: expanding array elements into indexed keys (`Tags[0]`, `Tags[1]`) was considered but rejected — the issue is specifically about object values, arrays don't have a stable/meaningful per-element key, and `JSON.stringify` on an array leaf is already a readable, non-lossy fallback.

## Testing Strategy

- Unit tests in `property-utils.test.ts` (see Implementation Steps #5) covering `flattenProperties` directly and its integration into `extractPropertiesFromRows`.
- Existing tests for `createPropertyTimelineGetter` and `aggregateIntoSegments` are unaffected (no signature changes) and should continue to pass unmodified.
- Manual verification against real or synthetic CloudWatch `Dimensions` data via the process metrics page, per Implementation Steps #6.

## Open Questions

- None — the fix is scoped and low-risk. If a broader consolidation of the three duplicated parsing/getter implementations is wanted, that should be tracked as a separate follow-up issue rather than folded into this fix.
