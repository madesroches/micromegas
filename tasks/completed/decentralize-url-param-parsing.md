# Decouple ScreenPage from useScreenConfig

**Status: DONE**

## Context

The medium-term plan is to move to notebooks only, retiring built-in custom screen types. So we avoid refactoring built-in page infrastructure — just decouple ScreenPage/notebooks from it.

## Problem

`ScreenPage` uses `useScreenConfig` + `parseUrlParams` but barely uses the result — it only reads `urlConfig.type`. Time range and variables are already handled independently through raw `searchParams`. The hook adds unnecessary indirection for notebooks.

## What was done

All five steps have been completed across two commits:

1. **6299b6c** — Decouple ScreenPage from useScreenConfig (steps 1-5)
2. **d0b0f2a** — Move variable URL ownership from ScreenPage to notebook layer (follow-up refactor)

### Step 1: Remove `useScreenConfig` from ScreenPage — DONE

ScreenPage reads `type` directly from `searchParams.get('type')`. No `useScreenConfig`, no `createBuildUrl`, no `DEFAULT_CONFIG`.

### Step 2: Replace `isReservedParam` with a local set in ScreenPage — DONE (then superseded)

`SCREEN_PAGE_PARAMS` was introduced, then removed entirely in the follow-up refactor. ScreenPage no longer filters URL params at all — variable extraction moved to `useNotebookVariables`.

### Step 3: Define reserved params in shared location — DONE

`RESERVED_URL_PARAMS` now lives in `src/lib/url-cleanup-utils.ts` (shared routing concern). `notebook-utils.ts` imports it from there. The test file imports it from there too.

### Step 4: Remove notebook-specific code from url-params.ts — DONE

`url-params.ts` no longer contains `RESERVED_PARAMS`, `isReservedParam`, `ReservedParam`, or variable extraction. It only has `parseUrlParams` for built-in page config fields.

### Step 5: Remove `variables` from ScreenPageConfig — DONE

`ScreenPageConfig` in `screen-config.ts` no longer has a `variables` field.

## Files changed

| File | Change |
|------|--------|
| `src/routes/ScreenPage.tsx` | Removed `useScreenConfig`, reads `type` from `searchParams`; no variable handling |
| `src/routes/__tests__/ScreenPage.urlState.test.tsx` | Imports `RESERVED_URL_PARAMS` from `url-cleanup-utils` |
| `src/lib/screen-renderers/notebook-utils.ts` | Imports `RESERVED_URL_PARAMS` from `url-cleanup-utils`, uses it in validation |
| `src/lib/url-params.ts` | Removed variable extraction, `RESERVED_PARAMS`, `isReservedParam`, `ReservedParam` |
| `src/lib/__tests__/url-params.test.ts` | Removed variable-related tests |
| `src/lib/screen-config.ts` | Removed `variables` from `ScreenPageConfig` |
| `src/lib/url-cleanup-utils.ts` | New: canonical home for `RESERVED_URL_PARAMS` (added in follow-up) |

## Not changed

- Built-in pages and `useScreenConfig` / `parseUrlParams` left as-is — they're being retired with the move to notebooks.
- `useScreenConfig.test.tsx` — the hook is still used by built-in pages; its tests remain.
