# Silence jsdom `navigation not implemented` console.error in AuthGuard test

## Overview

The `AuthGuard.test.tsx` "retry button" test clicks Retry, which calls
`window.location.reload()` directly. jsdom prints an 80-line
`Error: Not implemented: navigation (except hash changes)` stack trace via
`console.error` every time `yarn test` touches AuthGuard. The test still
passes — this is purely test-output noise. Fix by extending the existing
`navigation.ts` wrapper with a `reloadPage()` helper and using it in
`AuthGuard.tsx`, so the test can mock it the same way it already mocks
`navigateTo`.

## Current State

- `AuthGuard.tsx:60` calls `window.location.reload()` inline on the Retry
  button:
  ```tsx
  <button onClick={() => window.location.reload()} ...>
  ```
- The project already wraps the equivalent `window.location.assign` in
  `analytics-web-app/src/lib/navigation.ts`:
  ```ts
  /** Thin wrapper around window.location for testability (jsdom 26 freezes location methods) */
  export function navigateTo(url: string): void {
    window.location.assign(url)
  }
  ```
- `AuthGuard.tsx:24` already uses `navigateTo()` for the login redirect.
- `AuthGuard.test.tsx:10-12` mocks the wrapper:
  ```ts
  const mockNavigateTo = jest.fn()
  jest.mock('@/lib/navigation', () => ({
    navigateTo: (...args: unknown[]) => mockNavigateTo(...args),
  }))
  ```
- `test-setup.ts:58` explicitly documents the convention: "In jsdom 26+,
  `window.location` cannot be replaced. Use `jest.spyOn` in individual
  tests" — and `navigation.ts` exists so callers don't have to.
- The retry test at `AuthGuard.test.tsx:134-168` clicks Retry and only
  asserts the button is still in the document; it doesn't actually verify
  that reload was called.

## Design

Extend the existing wrapper module rather than introducing a one-off
spy in the test. This keeps the production code consistent (no direct
`window.location.*` calls in components) and reuses the test's existing
mock surface.

### Changes

1. **`navigation.ts`** — add a second wrapper:
   ```ts
   /** Thin wrapper around window.location for testability (jsdom 26 freezes location methods) */
   export function reloadPage(): void {
     window.location.reload()
   }
   ```

2. **`AuthGuard.tsx`** — import and call `reloadPage` on the Retry button:
   ```tsx
   import { navigateTo, reloadPage } from '@/lib/navigation'
   ...
   <button onClick={reloadPage} ...>
   ```

3. **`AuthGuard.test.tsx`** — extend the existing `jest.mock` to include
   `reloadPage`, and replace the weak "button still in document" assertion
   with a real check that reload was invoked:
   ```ts
   const mockNavigateTo = jest.fn()
   const mockReloadPage = jest.fn()
   jest.mock('@/lib/navigation', () => ({
     navigateTo: (...args: unknown[]) => mockNavigateTo(...args),
     reloadPage: () => mockReloadPage(),
   }))
   ...
   retryButton.click()
   expect(mockReloadPage).toHaveBeenCalled()
   ```

## Implementation Steps

1. Add `reloadPage()` to `analytics-web-app/src/lib/navigation.ts`.
2. Update `analytics-web-app/src/components/AuthGuard.tsx` to import and
   use `reloadPage` for the Retry button's `onClick`.
3. Update `analytics-web-app/src/components/__tests__/AuthGuard.test.tsx`:
   - Add `mockReloadPage` alongside `mockNavigateTo`.
   - Add `reloadPage` to the `jest.mock('@/lib/navigation', ...)` factory.
   - In the "retry button" test, assert `mockReloadPage` was called
     instead of just re-asserting the button is in the document.
4. Run `yarn test src/components/__tests__/AuthGuard.test.tsx` and confirm
   the 80-line jsdom stack trace is gone.
5. Run `yarn lint` and `yarn type-check`.

## Files to Modify

- `analytics-web-app/src/lib/navigation.ts`
- `analytics-web-app/src/components/AuthGuard.tsx`
- `analytics-web-app/src/components/__tests__/AuthGuard.test.tsx`

## Trade-offs

- **Wrapper helper vs. per-test `jest.spyOn`** — the issue suggests
  stubbing `window.location.reload` inside the test. That works but
  duplicates the very pattern `navigation.ts` was created to avoid, and
  the issue itself flags the jsdom-26 read-only-property foot-gun. A
  wrapper is one line longer in prod code and removes the foot-gun
  entirely.
- **Tightening the retry assertion** is a small bonus: the existing test
  doesn't actually verify reload was triggered. Once `reloadPage` is
  mockable, asserting it was called is free.

## Testing Strategy

- `yarn test src/components/__tests__/AuthGuard.test.tsx` — all six tests
  still pass, console output is clean (no `Error: Not implemented:
  navigation` stack trace).
- Full `yarn test` to confirm no other test relied on the inline
  `window.location.reload()` call path.
- `yarn lint` and `yarn type-check`.

## Open Questions

None. The approach mirrors an established project pattern; no behavior
change in production.
