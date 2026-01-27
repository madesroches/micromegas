# Move variable URL ownership to notebook layer

**Status: DONE**

## Context

ScreenPage currently extracts notebook variables from URL params and passes them down as props. This makes ScreenPage aware of notebook-specific concerns (which params are "reserved" vs "variables"). The notebook layer should own this.

Additionally, ScreenPage's save handler contains inline URL cleanup logic for both time params and variable params. This cleanup logic is a library concern — any screen type that uses URL-based state should be able to call the same function. ScreenPage should not be a URL broker.

## Constraint

Keep the imperative save-cleanup logic. No reactive effects. Config remains source of truth.

## Design principle

Each layer cleans up what it owns. The renderer is the natural coordination point since it knows what kind of screen it is.

1. **Time cleanup** — a shared utility (`cleanupTimeParams`). Strips `from`/`to` params that match the newly saved config.
2. **Variable cleanup** — a notebook-specific utility (`cleanupVariableParams`). Strips variable params that now match saved cell defaults.

ScreenPage's `handleSave` does the API call, updates state, and returns the saved config — no URL navigation. The renderer wraps `onSave`, receives the saved config, and performs all URL cleanup in a single `setSearchParams` call. This guarantees one navigation per save, regardless of how many cleanup concerns apply.

Non-notebook renderers call a default save wrapper that handles time cleanup only. NotebookRenderer extends it with variable cleanup. No new props or callback direction changes needed — just a richer return type on the existing `onSave`.

## Implementation

### 1. `url-cleanup-utils.ts` — shared post-save URL cleanup

New file at `src/lib/url-cleanup-utils.ts`. Contains time cleanup (universal) and the default save wrapper.

```ts
export const RESERVED_URL_PARAMS = new Set(['from', 'to', 'type'])

export function cleanupTimeParams(
  params: URLSearchParams,
  savedConfig: ScreenConfig,
): void
// Mutates params: removes `from`/`to` if they match savedConfig's time range.

export function useDefaultSaveCleanup(
  onSave: (() => Promise<ScreenConfig>) | null,
  setSearchParams: SetURLSearchParams,
): (() => Promise<void>) | null
// Returns a wrapped handleSave that calls onSave, then applies time cleanup
// via a single setSearchParams call. Returns null if onSave is null.
```

`RESERVED_URL_PARAMS` lives here because it's a routing concern, not a notebook concern. `notebook-utils.ts` imports it for variable name validation.

### 2. `notebook-utils.ts` — add `cleanupVariableParams`

- Import `RESERVED_URL_PARAMS` from `url-cleanup-utils`
- Remove the local `RESERVED_URL_PARAMS` definition (use the shared one)
- Add `cleanupVariableParams`:

```ts
export function cleanupVariableParams(
  params: URLSearchParams,
  savedConfig: ScreenConfig,
): void
// Mutates params: removes variable URL params that match saved cell defaults.
// Only touches non-reserved params.
```

This is a pure function that mutates a `URLSearchParams` object — no navigation, no hooks. Composable with `cleanupTimeParams` inside a single `setSearchParams` call.

### 3. `useNotebookVariables.ts` — own URL access

- Import `useSearchParams` from react-router-dom
- Import `RESERVED_URL_PARAMS` from `url-cleanup-utils`
- Remove `configVariables`, `onVariableChange`, `onVariableRemove` parameters
- Extract variables from `searchParams` internally (filter out `RESERVED_URL_PARAMS`)
- Implement set/remove internally using `setSearchParams` with functional updaters

New signature:
```ts
export function useNotebookVariables(
  cells: CellConfig[],
  savedCells: CellConfig[] | null | undefined,
): UseNotebookVariablesResult
```

### 4. `NotebookRenderer.tsx` — simplify call, own all cleanup

- Stop destructuring `urlVariables`, `onUrlVariableChange`, `onUrlVariableRemove` from props
- Update `useNotebookVariables` call to new 2-param signature
- Wrap `onSave` via `useMemo` to perform both time and variable cleanup in one navigation:

```ts
const handleSave = useMemo(() => {
  if (!onSave) return null
  return async () => {
    const savedConfig = await onSave()
    if (savedConfig) {
      setSearchParams(prev => {
        const next = new URLSearchParams(prev)
        cleanupTimeParams(next, savedConfig)
        cleanupVariableParams(next, savedConfig)
        return next
      })
    }
  }
}, [onSave, setSearchParams])
```

Pass `handleSave` (instead of `onSave`) to `<SaveFooter>`. One `setSearchParams` call, one navigation, both cleanup concerns composed.

### 5. `ScreenRendererProps` in `index.ts`

- Remove `urlVariables`, `onUrlVariableChange`, `onUrlVariableRemove`
- Change `onSave` return type from `(() => Promise<void>) | null` to `(() => Promise<ScreenConfig>) | null`

### 6. `shared.tsx` — widen `SaveFooterProps`

- Widen `onSave` type to accept `(() => Promise<void>) | (() => Promise<unknown>) | null` so both wrapped (returns void) and raw (returns ScreenConfig) save handlers work.

### 7. `ScreenPage.tsx`

- Remove `SCREEN_PAGE_PARAMS` export
- Remove `urlVariables` memo
- Remove `handleUrlVariableChange` and `handleUrlVariableRemove` callbacks
- Remove the three variable props from `<Renderer>` call
- Remove all URL cleanup logic from `handleSave` (both time and variable)
- Return `configToSave` at end of `handleSave`
- `handleSave` becomes: API call, update state, return saved config. No navigation, no URL cleanup.

### 8. Non-notebook renderers

Each renderer is responsible for its own post-save cleanup. Non-notebook renderers call `useDefaultSaveCleanup(onSave, setSearchParams)` to wrap `onSave` with time-param cleanup — one line per renderer. ScreenPage is not involved in any URL cleanup.

Renderers updated: `TableRenderer`, `MetricsRenderer`, `ProcessListRenderer`, `LogRenderer`.

If a renderer needs custom post-save cleanup in the future, it wraps `onSave` the same way NotebookRenderer does — composing `cleanupTimeParams` with its own logic in a single `setSearchParams` call.

### 9. Tests

- Replace `SCREEN_PAGE_PARAMS` local copy with import of `RESERVED_URL_PARAMS` from `url-cleanup-utils`
- Keep integration tests that verify time range changes and variable changes don't clobber each other
- Update variable tests to check hook re-render state (since `setSearchParams` doesn't go through `navigate`)
- Add unit tests for `cleanupTimeParams` (time-only cleanup, pure function)
- Add unit tests for `cleanupVariableParams` (variable-only cleanup, pure function)
- Add composition test: both cleanup functions applied to same `URLSearchParams` in one pass

## Files changed

| File | Change |
|------|--------|
| `src/lib/url-cleanup-utils.ts` | New: `RESERVED_URL_PARAMS`, `cleanupTimeParams`, `useDefaultSaveCleanup` |
| `src/lib/screen-renderers/notebook-utils.ts` | Import shared `RESERVED_URL_PARAMS`, add `cleanupVariableParams` |
| `src/lib/screen-renderers/useNotebookVariables.ts` | Own URL access via `useSearchParams` with functional updaters |
| `src/lib/screen-renderers/NotebookRenderer.tsx` | Use new hook signature, wrap `onSave` with both time + variable cleanup |
| `src/lib/screen-renderers/index.ts` | Remove 3 variable props, change `onSave` return type to `Promise<ScreenConfig>` |
| `src/lib/screen-renderers/shared.tsx` | Widen `SaveFooterProps.onSave` type for wrapped handlers |
| `src/lib/screen-renderers/TableRenderer.tsx` | Add `useDefaultSaveCleanup`, pass `handleSave` to `SaveFooter` |
| `src/lib/screen-renderers/MetricsRenderer.tsx` | Add `useDefaultSaveCleanup`, pass `handleSave` to `SaveFooter` |
| `src/lib/screen-renderers/ProcessListRenderer.tsx` | Add `useDefaultSaveCleanup`, pass `handleSave` to `SaveFooter` |
| `src/lib/screen-renderers/LogRenderer.tsx` | Add `useDefaultSaveCleanup`, pass `handleSave` to `SaveFooter` |
| `src/routes/ScreenPage.tsx` | Remove all variable and cleanup logic, return saved config from `handleSave` |
| `src/routes/__tests__/ScreenPage.urlState.test.tsx` | Update integration tests, add cleanup utility unit tests |

## Verification

- `yarn type-check` — pass
- `yarn test` — 334 tests pass (17 suites)
- `yarn lint` — clean
- Manual: open notebook, change variable → URL updates. Save → URL cleans up (both time and variables in one navigation). Back/forward works. Time range changes don't lose variables.
