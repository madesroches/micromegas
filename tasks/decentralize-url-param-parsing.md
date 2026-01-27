# Decouple ScreenPage from useScreenConfig

## Context

The medium-term plan is to move to notebooks only, retiring built-in custom screen types. So we avoid refactoring built-in page infrastructure — just decouple ScreenPage/notebooks from it.

## Problem

`ScreenPage` uses `useScreenConfig` + `parseUrlParams` but barely uses the result — it only reads `urlConfig.type`. Time range and variables are already handled independently through raw `searchParams`. The hook adds unnecessary indirection for notebooks.

## Plan

### Step 1: Remove `useScreenConfig` from ScreenPage

Replace:
```ts
const { config: urlConfig } = useScreenConfig(DEFAULT_CONFIG, buildUrl)
const typeParam = (urlConfig.type ?? null) as ScreenTypeName | null
```

With:
```ts
const typeParam = (searchParams.get('type') ?? null) as ScreenTypeName | null
```

Remove `createBuildUrl`, `DEFAULT_CONFIG`, and the `useScreenConfig` import. Popstate is already covered by React Router's `useSearchParams`.

### Step 2: Replace `isReservedParam` with a local set in ScreenPage

ScreenPage filters URL params to extract notebook variables. Replace the imported `isReservedParam` with a local constant:

```ts
const SCREEN_PAGE_PARAMS = new Set(['from', 'to', 'type'])
```

Use this in `urlVariables` computation and `handleSave`.

### Step 3: Remove notebook-specific code from url-params.ts

- Remove variable extraction block from `parseUrlParams`
- Remove `RESERVED_PARAMS`, `isReservedParam`, `ReservedParam` (no remaining consumers after Step 2)

### Step 4: Clean up notebook-utils.ts

Remove re-exports of `RESERVED_PARAMS`/`isReservedParam`. Update or remove `isReservedVariableName` and `getReservedNameError` to use ScreenPage's local set or inline the check.

## Files Changed

| File | Change |
|------|--------|
| `src/routes/ScreenPage.tsx` | Remove `useScreenConfig`, read `type` from `searchParams`, local `SCREEN_PAGE_PARAMS` set |
| `src/lib/url-params.ts` | Remove variable extraction, `RESERVED_PARAMS`, `isReservedParam`, `ReservedParam` |
| `src/lib/screen-renderers/notebook-utils.ts` | Remove reserved-param re-exports, update variable name validation |
| `src/lib/__tests__/url-params.test.ts` | Remove variable-related tests |
| `src/routes/__tests__/ScreenPage.urlState.test.tsx` | Update tests for removed `useScreenConfig` |
| `src/lib/screen-config.ts` | Remove `variables` from `ScreenPageConfig` if unused |

## Not Changed

Built-in pages and `useScreenConfig` / `parseUrlParams` left as-is — they're being retired with the move to notebooks.

## Validation

- `yarn type-check` passes
- `yarn test` passes
- `yarn lint` passes
- Manual: open a notebook, verify variables sync to URL, browser back/forward works
