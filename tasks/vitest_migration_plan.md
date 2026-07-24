# Jest → Vitest Migration Plan (analytics-web-app)

**Issue**: [#1345](https://github.com/madesroches/micromegas/issues/1345)
**Goal**: Replace Jest with Vitest as the test runner for `analytics-web-app`, so tests are parsed by the same engine that builds the app and ESM-only dependencies stop requiring per-package carve-outs.

**Success condition**: `yarn test` runs 56 suites / 1167 tests green under Vitest, `yarn lint`, `yarn type-check` and `yarn build` all pass, `build/analytics_web_ci.py` completes, and no Jest/Babel package remains in `package.json`'s `dependencies`/`devDependencies`, including the now-unreachable `@babel/core` `resolutions` pin (Design §2).

## Overview

`analytics-web-app` builds with Vite 8 but tests with Jest 30 through `ts-jest` + `babel-jest`. Every ESM-only dependency has to be hand-carved out of Jest's CommonJS pipeline (`jest.config.js:23-25` already does this for `d3-dsv`; `react-router@8` in #1347 needs the same plus a `babel-plugin-transform-import-meta` shim). Vitest reads the app's own `vite.config.ts`, so that class of problem disappears: the alias set, the React plugin, and `import.meta` support are already configured for the build and are reused verbatim.

**Sequencing note (also covers the tables in Design §5, §7, §9):** as of this writing, #1347 is still open — `jest.config.js` carries only the `d3-dsv` carve-out and `package.json:41` still pins `react-router-dom@^7.18.0` — but both issues state #1347 lands first as a security fix. #1347's scope is the `react-router-dom` → `react-router` rename across 29 files, including the `jest.mock('react-router-dom')` sites; that rename is #1347's job, not this plan's. If #1347 has landed by the time this migration starts, read `react-router` everywhere this plan says `react-router-dom` (the §5 hoisted-factory rows tied to router mocks, all `react-router-dom` rows in the §7 table, and the two `react-router-dom` mentions in §9).

This is **not** a performance change. Measured baseline on this branch: **56 suites, 1167 tests, 6.66 s**. There is no wall-clock target and no regression gate.

## Current State

### Jest configuration (`analytics-web-app/jest.config.js`)

| Line | Setting | Purpose |
|---|---|---|
| 3-6 | `testEnvironment: 'jsdom'`, `testEnvironmentOptions.url` | jsdom at `http://localhost:3000` |
| 7 | `setupFilesAfterEnv: ['<rootDir>/src/test-setup.ts']` | polyfills + two global module mocks |
| 9 | `'\\.css$' → src/__mocks__/styleMock.js` | 4 CSS imports exist in `src/` (`main.tsx:9`, `ui/DateTimePicker.tsx:5-6`, `XYChart.tsx:3`) |
| 10 | `'^@/(.*)$' → src/$1` | duplicates `vite.config.ts:45` |
| 11-13 | stubs for `react-markdown`, `remark-gfm`, `@radix-ui/react-dropdown-menu` | ESM-only / jsdom-hostile packages |
| 15-22 | `ts-jest` (ESM) + `babel-jest` with `@babel/preset-env` | the transform pipeline being deleted |
| 23-25 | `transformIgnorePatterns: node_modules/(?!(d3-dsv)/)` | the ESM treadmill |
| 27-31 | `testMatch` (3 patterns) | see "Test discovery" below |
| 32-35 | `collectCoverageFrom` | `src/**/*.{ts,tsx}` minus `.d.ts` |

`package.json:12-14` — `test: jest`, `test:watch: jest --watch`, `test:coverage: jest --coverage`.
`.eslintrc.json` ignores `jest.config.js`.

### Test discovery — a silent-loss trap

56 files match `testMatch`. All are named `*.test.ts(x)` **except one**:

- `src/lib/__tests__/arrow-ipc-fixtures.ts` — exports fixture helpers (lines 103, 154, 200) **and** contains its own `describe`/`it` blocks (lines 212-266).

Vitest's default `include` is `['**/*.{test,spec}.?(c|m)[jt]s?(x)']`, which would silently drop that file and its 3 tests (56 → 55 suites). The `include` patterns must mirror `testMatch`, not be left at the default.

`src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts` is **not** in a `__tests__` directory and is correctly not collected.

### Jest API surface (measured across `src/` + `tests/`)

| API | Uses | Notes |
|---|---|---|
| `jest.fn` | 271 | mechanical → `vi.fn` |
| `jest.mock` | 64 | mechanical name, but see hoisting/`require` below |
| `jest.Mock` (type) | 30 in 8 files | **not** in the issue's table; needs `import type { Mock } from 'vitest'` |
| `jest.resetAllMocks` | 13 | mechanical |
| `jest.clearAllMocks` | 12 | mechanical |
| `jest.MockedFunction` (type) | 7 | → `MockedFunction` from `vitest` |
| `jest.restoreAllMocks` | 5 | mechanical |
| `jest.requireActual` | 5 | → `await vi.importActual` (async — changes shape) |
| `jest.requireMock` | 1 | → `await vi.importMock` (async) |
| `jest.useFakeTimers` / `useRealTimers` / `advanceTimersByTime` | 3 | mechanical (`VariableCell.test.tsx:16,20,201`) |
| `jest.spyOn` | 1 real use | `MapHoverTooltip.test.tsx:65-67` (`const spy = jest\n  .spyOn(HTMLElement.prototype, 'getBoundingClientRect')...`) is a real call, line-broken across `jest` and `.spyOn`; the 2 other grep hits (`test-setup.ts:58`, `table-utils.test.tsx:18`) are comments only |

No snapshots (`__snapshots__` / `*.snap`: none), so no snapshot-format churn.

### Dependency reality check

`@vitejs/plugin-react@6` uses Rolldown/Oxc, **not** Babel — its only dependency is `@rolldown/pluginutils`. Confirmed with `yarn why`:

- `@babel/core` — consumers are `@jest/transform`, `jest-config`, `jest-snapshot`, `istanbul-lib-instrument`, plus the direct devDep. Nothing in the build path, and `@vitest/coverage-v8`'s own dependency tree (`magicast` → `@babel/parser` / `@babel/types`, `@bcoe/v8-coverage`, `ast-v8-to-istanbul`, the `istanbul-*` packages, `obug`, `std-env`, `tinyrainbow`, `@vitest/utils`) does not depend on it either — so once the Jest packages and the direct devDep are gone, `@babel/core` is unreachable, and the `resolutions` pin protects nothing.
- `magicast` (pulled in by `@vitest/coverage-v8@4.1.10`) — depends on `@babel/parser@^7.29.0` and `@babel/types@^7.29.0`. This is coverage-v8's actual transitive Babel surface; neither package is pinned in `resolutions` today, and there's no known CVE currently forcing one.
- `@babel/preset-env` — direct devDep only.
- `@babel/preset-react` — direct devDep only, and **not referenced by `jest.config.js` at all** (dead today).
- `@babel/plugin-transform-modules-systemjs` (in `resolutions`) — reachable only via `@babel/preset-env`; becomes dead.
- `browserslist` + `baseline-browser-mapping` — still required by `autoprefixer@10.5.0`. **Keep both**, and keep their `resolutions` pins.
- `jsdom@26.1.0` — currently only reachable via `jest-environment-jsdom`. Vitest 4 declares `jsdom: '*'` as a permissive **peer** (no compatibility signal) and does not bundle it, so it must become a direct devDep. Latest is `29.1.1` as of this writing, three majors ahead — pinning to today's transitive `^26.1.0` is intended to preserve current jsdom behaviour while swapping the runner. However, `vitest@4.1.10`'s own `devDependencies` pin `jsdom: ^27.4.0` — i.e. the version Vitest 4 is actually developed and tested against is a major above `^26.1.0` — so this pairing is untested upstream, not merely conservative. See the corresponding Risks table row; bumping further to `29.x` remains a separate follow-up, but bumping to a Vitest-tested `^27.x` is an in-scope fallback if the `^26.1.0` pairing misbehaves.

### Registry / compatibility

- `vitest@4.1.10`, `@vitest/coverage-v8@4.1.10`; peers `vite ^6 || ^7 || ^8` (app has `vite@8.0.16`), engines `node ^20 || ^22 || >=24`. `.nvmrc` pins `20` — fine.
- `@testing-library/jest-dom@6.9.1` (installed) already exposes a `./vitest` export.
- No other workspace uses Vitest; `grafana/` stays on Jest via the Grafana plugin toolchain. This introduces a second runner to the monorepo — acceptable, the two toolchains are unrelated.

### CI

`build/analytics_web_ci.py` runs `yarn install / type-check / lint / test / build`; `.github/workflows/analytics-web-app.yml` just calls that script. **No CI change needed** — Vitest's `configDefaults` already fall back to run-once mode in CI and when stdin isn't a TTY, which covers both the script and GitHub Actions. The script is still `vitest run` so that an interactive developer running `yarn test` locally lands in run-once mode too, matching `jest`'s default.

`tsconfig.json:35-42` excludes `src/**/*.test.ts(x)`, the `__tests__` directories, and `src/test-setup.ts` — but **not** `__test-utils__` or `__mocks__` directories, which have no matching exclude pattern and so are type-checked today (confirmed with `tsc --noEmit --listFilesOnly`, which includes `src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts` and all three `src/__mocks__/*` files). So `yarn type-check` skips `*.test.ts(x)` / `__tests__` content both today and after the migration, but does check `__test-utils__` / `__mocks__` content in both cases. See "Types" below and Phase 3 step 9.

## Design

### 1. Config lives in `vite.config.ts`

A sibling `vitest.config.ts` would *replace* `vite.config.ts` rather than extend it (Vitest only falls back to `vite.config.ts` when no `vitest.config.*` exists), forcing duplication of the alias set and the React plugin — which defeats the point of the migration. So add a `test` block to the existing config.

To keep `vitest` out of the production build's module graph, declare the types with a triple-slash reference instead of importing `defineConfig` from `vitest/config`:

```ts
/// <reference types="vitest/config" />
import { defineConfig, loadEnv } from 'vite'
```

Then inside the returned object (alongside `resolve`, `build`, `server`):

```ts
test: {
  globals: true,
  environment: 'jsdom',
  environmentOptions: {
    jsdom: { url: 'http://localhost:3000' },
  },
  setupFiles: ['./src/test-setup.ts'],
  // Mirrors jest.config.js testMatch. Must stay explicit: the Vitest default
  // would drop src/lib/__tests__/arrow-ipc-fixtures.ts, which holds 3 tests.
  include: [
    'src/**/__tests__/**/*.{ts,tsx}',
    'src/**/*.{test,spec}.{ts,tsx}',
    'tests/**/*.{ts,tsx}',
  ],
  // Test-only stubs; merged with resolve.alias, so '@' is inherited.
  alias: {
    'react-markdown': path.resolve(__dirname, './src/__mocks__/react-markdown.tsx'),
    'remark-gfm': path.resolve(__dirname, './src/__mocks__/remark-gfm.ts'),
    '@radix-ui/react-dropdown-menu': path.resolve(
      __dirname,
      './src/__mocks__/@radix-ui/react-dropdown-menu.tsx'
    ),
  },
  coverage: {
    provider: 'v8',
    include: ['src/**/*.{ts,tsx}'],
    exclude: [
      'src/**/*.d.ts',
      'src/**/__tests__/**',
      'src/**/__mocks__/**',
      'src/**/__test-utils__/**',
      'src/lib/datafusion-wasm/**',
    ],
  },
},
```

Notes:
- `test.alias` is merged with `resolve.alias`, so the `@/` mapping is inherited from `vite.config.ts:45` — the duplicate in `jest.config.js:10` disappears. Keeping the stubs in `test.alias` (not `resolve.alias`) is what prevents them leaking into the production build.
- Vite string alias keys match exactly or as a directory prefix. None of the three packages is imported with a subpath here, so the object form is safe; if a subpath import appears later, switch `test.alias` to the array-of-regex form (`{ find: /^react-markdown$/, replacement: … }`).
- The `\\.css$ → styleMock.js` mapping is dropped: Vitest returns an empty module for CSS imports by default (`css: false`). `src/__mocks__/styleMock.js` is deleted.
- `globals: true` is load-bearing beyond avoiding 55 import edits: `@testing-library/react`'s auto-`cleanup` registers itself through the global `afterEach`. Without globals, DOM would leak across tests in every render-based suite.
- The `wasm-content-type` and `log-base-path` plugins only implement `configureServer` and are inert under test. `loadWasmEngine` (`src/lib/wasm-engine.ts:11`) uses a lazy dynamic `import()` never reached from a test, and `optimizeDeps.exclude` already lists the package.

### 2. Dependency changes

Remove from `devDependencies`: `@babel/core`, `@babel/preset-env`, `@babel/preset-react`, `@types/jest`, `babel-jest`, `jest`, `jest-environment-jsdom`, `ts-jest`. `@babel/preset-react` is verified dead independent of Jest: it's not referenced by `jest.config.js`, nothing in `yarn.lock` depends on it transitively, and `@vitejs/plugin-react`'s only dependency is `@rolldown/pluginutils` — no build path touches it. Remove it in this pass.
Add to `devDependencies`: `jsdom@^26.1.0` (today's transitive version, pinned deliberately to preserve current jsdom behaviour during the runner swap — see Current State → Dependency reality check; note this is a major below the `^27.4.0` Vitest 4.1.10 itself is developed against, so treat an environment-setup failure as an expected possibility, not a surprise — the in-scope fallback is bumping to a Vitest-tested `^27.x`; bumping further to the current `29.1.1` remains a separate follow-up), `vitest@^4.1.10`, `@vitest/coverage-v8@^4.1.10`.
Remove from `resolutions`: `@babel/plugin-transform-modules-systemjs` (unreachable once `@babel/preset-env` is gone), `@babel/core` (also unreachable once the Jest packages and the direct devDep are gone — `@vitest/coverage-v8`'s Babel surface is `magicast` → `@babel/parser`/`@babel/types`, neither of which pulls in `@babel/core`; see Current State → Dependency reality check).
**Keep** in `resolutions`: `browserslist` / `baseline-browser-mapping`, which `autoprefixer` still needs.

Scripts (`package.json:12-14`):

```json
"test": "vitest run",
"test:watch": "vitest",
"test:coverage": "vitest run --coverage"
```

`vitest run` (not bare `vitest`) is used so interactive local `yarn test` does not enter watch mode. (Vitest already falls back to run-once mode in CI and when stdin isn't a TTY, so `build/analytics_web_ci.py` and GitHub Actions would not hang either way — the explicit `run` is about matching `jest`'s default for a developer typing `yarn test` at a terminal.)

Delete `jest.config.js` and drop it from `.eslintrc.json` `ignorePatterns`.

### 3. Types

`@types/jest` currently injects the `jest` global namespace repo-wide (tsconfig has no `types` allowlist, so every `@types/*` package is ambient). Removing it makes any leftover `jest.Mock` / `jest.MockedFunction` reference an editor error — useful pressure to finish the rename, but it also removes the ambient `describe`/`it`/`expect` declarations that `globals: true` needs.

`vitest/globals` is not an `@types/*` package, so it is not picked up automatically. But `yarn tsc --noEmit --listFilesOnly` shows the only test-adjacent files in the checked program are the three `src/__mocks__/*` files and `src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts` — none of which references `describe`/`it`/`expect`/`vi`. `tsconfig.json:35-42` **excludes** `src/**/*.test.ts(x)`, the `__tests__` directories, and `src/test-setup.ts` — the only file that will reference `vi` — from that same program. A `/// <reference>` inside a `.d.ts` only augments the program that contains it, so no new ambient file is needed today: nothing in the checked program uses a Vitest global.

Decision: skip adding a dedicated ambient file. If a future edit adds a `vi`/`describe`/`it`/`expect` reference to a checked-in file, fold `/// <reference types="vitest/globals" />` into the existing `src/vite-env.d.ts` (which already carries `/// <reference types="vite/client" />` and is part of the same program) rather than creating a new file. Test files remain outside the type-checked program before and after this migration, so `yarn type-check` / CI behavior is unchanged; only editor-level hints for `describe`/`it`/`expect` inside test files are affected, and a `tsconfig.test.json` with `"types": ["vitest/globals"]` that brings test files into a checked program would address that as a separable follow-up, not part of this migration.

Type-level renames:
- `jest.MockedFunction<typeof f>` → `MockedFunction<typeof f>` with `import type { MockedFunction } from 'vitest'` (7 uses, 6 files).
- `jest.Mock` → `Mock` with `import type { Mock } from 'vitest'` (30 uses, 8 files: `auth.test.tsx`, `AuthGuard.test.tsx`, `notebook-cell-view.test.ts`, `table-utils.test.tsx`, `PerformanceAnalysisPage.test.tsx`, `HorizontalGroupCell.test.tsx`, `useCellExecution.test.ts`).

### 4. Mechanical renames

`jest.fn` → `vi.fn`, `jest.mock` → `vi.mock`, `jest.clearAllMocks` → `vi.clearAllMocks`, `jest.resetAllMocks` → `vi.resetAllMocks`, `jest.restoreAllMocks` → `vi.restoreAllMocks`, `jest.useFakeTimers` / `useRealTimers` / `advanceTimersByTime` → `vi.*`. Jest's config defaults for `clearMocks` / `resetMocks` / `restoreMocks` are all `false`, matching Vitest's defaults, so no behavioural drift from the config side.

A single `sed`-style pass over the 56 test files plus `src/test-setup.ts` covers ~360 of the ~400 call sites. Everything below is what the pass must **not** touch.

**Line-broken `jest` references — a naive `jest.` → `vi.` regex misses these six sites** because the identifier and the member access are split across lines:
- `src/components/map/__tests__/MapHoverTooltip.test.tsx:65-67` — `const spy = jest\n  .spyOn(HTMLElement.prototype, 'getBoundingClientRect')` (the one real `jest.spyOn` call, see the API surface table above)
- `src/lib/__tests__/maps-catalog.test.ts:155,239,290` — `jest\n  .fn()` (3 sites)
- `src/routes/__tests__/MapsPage.test.tsx:81,116` — `jest\n  .fn()` (2 sites)

The mechanical pass must match a bare `jest` identifier regardless of trailing whitespace/newline before the `.`, not just the literal string `jest.`.

### 5. `vi.hoisted()` — 12 factory sites

`vi.mock` calls are hoisted above the test file's imports, and the factory runs during the mocked module's first resolution — before top-level `const`s are initialised. Twelve factories currently close over an outer binding and need it moved into `vi.hoisted()`:

| File | Line | Bindings |
|---|---|---|
| `src/test-setup.ts` | 50 | `mockNavigate` |
| `src/components/__tests__/AuthGuard.test.tsx` | 11 | `mockNavigateTo`, `mockReloadPage` |
| `src/components/__tests__/DataSourceSelector.test.tsx` | 10 | `getDataSourceList` |
| `src/hooks/__tests__/useScreenConfig.test.tsx` | 10 | `mockNavigate` |
| `src/hooks/__tests__/useStreamQuery.test.ts` | 9 | `mockStreamQuery` |
| `src/lib/__tests__/auth.test.tsx` | 10 | `mockNavigateTo` |
| `src/lib/screen-renderers/__tests__/NotebookRenderer.test.tsx` | 9 | `mockStreamQuery` |
| `src/lib/screen-renderers/__tests__/useCellExecution.test.ts` | 26 | `mockFetchQueryIPC`, `mockStreamQuery` |
| `src/lib/screen-renderers/__tests__/useNotebookVariables.test.tsx` | 29 | `mockInitialSearch` — combined with its §7 row below; see the merged conversion note after the §7 table |
| `src/routes/__tests__/PerformanceAnalysisPage.test.tsx` | 96, 129 | `mockExecute`, `executeStreamQuery` |
| `src/routes/__tests__/ScreenPage.urlState.test.tsx` | 20 | `mockNavigate` |

Pattern (`NotebookRenderer.test.tsx:8-11`):

```ts
const { mockStreamQuery } = vi.hoisted(() => ({ mockStreamQuery: vi.fn() }))
vi.mock('@/lib/arrow-stream', () => ({
  streamQuery: (...args: unknown[]) => mockStreamQuery(...args),
}))
```

`vi.fn()` used *inside* a factory body (e.g. the `lucide-react` and `@dnd-kit/*` stubs) needs no change — `vi` is injected by the hoisting transform.

### 6. `require()` inside factories — 5 sites + the helper itself

Vitest runs test files as ESM; `require` is not defined. The shared cell-registry mock is pulled in with `require()` at 5 call sites, and the helper itself uses top-level `require`:

- `src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts:10` — `const React = require('react')` → `import React from 'react'`
- `…cell-registry-mock.ts:14` — `const { substituteMacros, DEFAULT_SQL } = require('../notebook-utils')` → static `import`
- `…cell-registry-mock.ts:6` — the usage doc-comment needs updating to the new form

`cell-registry-mock.ts` sits under `__test-utils__`, which — unlike `__tests__` and `*.test.ts(x)` — is not excluded by `tsconfig.json` (see Current State → CI above), so it is already part of the `tsc --noEmit` program today. Its two `require()` calls are currently implicitly `any`; converting them to typed static imports puts this ~330-line helper under `strict: true` for the first time. Budget for `yarn type-check` to newly surface errors here (see Phase 3 step 9).

Call sites become async factories with a dynamic import:

```ts
vi.mock('../cell-registry', async () => {
  const { createCellRegistryMock } = await import('../__test-utils__/cell-registry-mock')
  return createCellRegistryMock({ withRenderers: true, withEditors: true })
})
```

Sites: `NotebookRenderer.test.tsx:98`, `useCellExecution.test.ts:66`, `notebook-utils.test.ts:18`, `CellContainer.test.tsx:6`, `HorizontalGroupCell.test.tsx:74`. The two `eslint-disable … no-require-imports` comments in the helper come out with the `require`s.

### 7. `requireActual` / `requireMock` — 6 async conversions

`await vi.importActual(...)` / `await vi.importMock(...)` are async, so the factory becomes `async`:

| File | Line | Module |
|---|---|---|
| `src/test-setup.ts` | 51 | `react-router-dom` |
| `src/hooks/__tests__/useScreenConfig.test.tsx` | 11 | `react-router-dom` |
| `src/routes/__tests__/ScreenPage.urlState.test.tsx` | 21 | `react-router-dom` |
| `src/lib/screen-renderers/__tests__/useNotebookVariables.test.tsx` | 30 | `react-router-dom` — combined with its §5 row above; see below |
| `src/lib/screen-renderers/__tests__/table-utils.test.tsx` | 22 | `../notebook-utils` |
| `src/lib/__tests__/arrow-utils.test.ts` | 114 | `jest.requireMock('apache-arrow')` → top-level `await vi.importMock('apache-arrow')` (test files are ESM, so top-level `await` is available) |

**`useNotebookVariables.test.tsx` is one combined conversion, not two independent one-liners.** Its `react-router-dom` factory (lines 29-61) both needs `vi.hoisted()` for the outer `mockInitialSearch` binding (Design §5) *and* `await vi.importActual('react-router-dom')` in place of `jest.requireActual` (Design §7) — but the factory also calls `useState`, `useMemo`, `useRef`, and `useCallback`, imported from `react` at line 16. Hoisted `vi.mock` factories run before the file's own top-level imports are initialised, so those bindings cannot be closed over directly. The factory must become `async` and pull them in itself:

```ts
const { mockInitialSearch, setMockInitialSearch } = vi.hoisted(() => ({
  mockInitialSearch: { current: '' },
  setMockInitialSearch: (v: string) => { mockInitialSearch.current = v },
}))
vi.mock('react-router-dom', async () => {
  const { useState, useMemo, useRef, useCallback } = await import('react')
  const actual = await vi.importActual('react-router-dom')
  return {
    ...actual,
    useSearchParams: (): [URLSearchParams, SetSearchParamsFn] => {
      const [raw, setRaw] = useState(mockInitialSearch.current)
      // ...unchanged body, reading/writing mockInitialSearch.current instead of the bare outer `let`
    },
  }
})
```

### 8. Partial mocks become strict — the main expected source of failures

Under Jest's CJS interop, importing a name a partial factory did not return yields `undefined`. Vitest throws:

> `No "X" export is defined on the "Y" mock. Did you forget to return it from vi.mock?`

Every one of the 64 `vi.mock` factories that returns a subset of a module's exports is exposed to this, but only when the importer actually pulls in the missing name at the **value** position — a name imported only as a type is erased by the transform and never triggers the runtime check (e.g. the `apache-arrow` factory at `arrow-utils.test.ts:6-99` omits only `Duration` — it returns `Timestamp` and `Table` classes directly — and `Duration` is used solely as an `apache-arrow` type import in `arrow-utils.ts:5`, so it never throws). Highest-risk factories, by breadth of the real module's API *and* confirmed value-position exposure:

- `@/lib/arrow-stream` — `NotebookRenderer.test.tsx:9-11` returns only `streamQuery`, omitting `fetchQueryIPC`. `NotebookRenderer.tsx` pulls in `useCellExecution.ts`, which imports `fetchQueryIPC` and calls it at `useCellExecution.ts:217`/`:254` — a certain first-run throw, not a hypothetical one.
- `apache-arrow` — `NotebookRenderer.test.tsx:14` (returns only `Table`), `useStreamQuery.test.ts`, `useCellExecution.test.ts`, `arrow-utils.test.ts`
- `lucide-react` — 5 files, each enumerating a fixed icon list; two are confirmed certain throws, not just breadth risk:
  - `CustomRange.test.tsx:15` returns only `Calendar`; `CustomRange.tsx:8` imports `@/components/ui/DateTimePicker`, which imports `Clock` (`:4`) and renders it at `:107`.
  - `NotebookRenderer.test.tsx:23-44` omits `Check`; `NotebookRenderer.tsx:35` imports `./NotebookSourceView`, which imports `Check` (`:2`) and renders it at `:85`.
- `@dnd-kit/core` / `@dnd-kit/sortable` / `@dnd-kit/utilities` — `NotebookRenderer.test.tsx:47-83`, `HorizontalGroupCell.test.tsx`; the `@dnd-kit/sortable` factory omits `horizontalListSortingStrategy`, used in a value position at `cells/HorizontalGroupCell.tsx:172`
- `@/components/layout`, `@/lib/auth` — `MapsPage.test.tsx`, `PerformanceAnalysisPage.test.tsx`
- `@/lib/data-sources-api` — 4 call sites, factories return 1 of 8 real exports
- `@/lib/api` — 3 call sites
- `@/lib/arrow-utils` — 1 of 13 real exports returned; confirmed certain throw at `log-utils.test.ts:10`, which returns only `timestampToDate` — `log-utils.tsx:8` imports `./table-utils`, which imports and calls `isTimeType`, `isNumericType`, `isBinaryType`, `isDurationType`, `durationToMs` (`table-utils.tsx:14-21`, used at `:730`, `:757`, `:762-763`, `:767`, `:782`)
- `../cell-registry` — 5 helper-based sites (Design §6) plus one independent inline factory at `notebook-cell-view.test.ts:14`, which is safe (it returns only `getCellTypeMetadata`, the only name `notebook-cell-view.ts:3` imports)

(`@/lib/config` exports exactly `getConfig` and `appLink`; all three factories mocking it return both, so despite its earlier billing here it carries zero strict-mock exposure.)

Fix per occurrence, in preference order: (a) add the missing export to the factory when the point of the mock is a narrow stub; (b) spread `await vi.importActual(...)` when the mock only means to override one or two names. This is per-error work, not a sweep — budget the bulk of the migration time here.

### 9. `src/test-setup.ts` specifics

- `import '@testing-library/jest-dom'` → `import '@testing-library/jest-dom/vitest'`.
- The two global `jest.mock` calls (`@/lib/config` at line 43, `react-router-dom` at line 50) become `vi.mock`, with `vi.hoisted()` for `mockNavigate` and an async factory for the `react-router-dom` `importActual` spread. `vi.mock` in a setup file applying to every test file is verified against the 4.1.10 implementation, not documented behavior — the docs (`docs/api/vi.md`) actually recommend using `vi.mock` / `vi.hoisted` only inside test files, and never affirm that setup-file mocks apply repo-wide. It does work in 4.1.10: the hoisting transform has no setup-file exemption, `experimental.viteModuleRunner` defaults to `true`, and the mock registry is a plain `Map.set` shared process-wide — but that's an implementation fact with a config dependency, not a documented guarantee, so the empirical check in Phase 2 step 6 is load-bearing, not a sanity check. The real, documented constraint is different — **a module already imported by the setup file is cached before the mock is registered and cannot then be mocked.** `src/test-setup.ts` today imports only `@testing-library/jest-dom`, `util`, and `stream/web`, so neither `@/lib/config` nor `react-router-dom` is pre-imported and both mocks are safe; keep it that way as new setup-file imports are added. The 4 test files which re-mock `react-router-dom` themselves keep winning either way: their `vi.mock` is hoisted within a file that runs after setup, so the last registration wins — same precedence as Jest today.
- `process.env.NODE_ENV = 'development'` (line 39) can be dropped: nothing in `src/` reads `NODE_ENV`, Vitest runs in mode `test` so `import.meta.env.DEV` is `true`, and React resolves to its development build because `process.env.NODE_ENV !== 'production'` at runtime. One-line revert if anything regresses.
- The `TextEncoder` / `TextDecoder` / web-streams polyfills (lines 2-3, 10-20) are almost certainly redundant under Node 20/22 + Vitest's jsdom environment, but they are idempotent — **leave them alone in this change** to keep the diff to one concern. Switch the specifiers to `node:util` / `node:stream/web` while touching the file. The `@ts-expect-error` directives above them may become "unused directive" errors, but `tsconfig.json` excludes `src/test-setup.ts`, so `tsc --noEmit` will not see it.
- The `DOMRect` polyfill (lines 22-35) stays.
- Update the trailing comment (lines 57-58) — it references `jest testEnvironmentOptions.url` and `jest.spyOn`.

### 10. Stale comments referencing Jest config

`src/components/__tests__/CellContainer.test.tsx:23` and `src/lib/screen-renderers/__tests__/NotebookRenderer.test.tsx:46` both say "mocked via moduleNameMapper in jest.config.js" → point them at `test.alias` in `vite.config.ts`.

`src/lib/screen-renderers/__tests__/table-utils.test.tsx:18-19` also survive the mechanical rename as plain comments — "…identical output. `jest.spyOn`" and "can't be used here — this repo's Jest runs in ESM mode and module-namespace…" — and would otherwise match Phase 5 step 16's cleanup grep. The technical claim still holds under Vitest; reword "this repo's Jest" to "this repo's test runner" (or similar) without changing the underlying reasoning.

## Implementation Steps

### Phase 1 — Toolchain swap
1. `package.json`: apply the dependency and script changes from Design §2; run `yarn install` and confirm a clean, warning-free install.
2. `vite.config.ts`: add the triple-slash reference and the `test` block from Design §1.
3. No ambient globals file to add (Design §3) — skip.
4. Delete `jest.config.js` and `src/__mocks__/styleMock.js`; remove `jest.config.js` from `.eslintrc.json` `ignorePatterns`.
5. Confirm discovery before touching test bodies: `yarn vitest list --filesOnly` must report 56 files, including `src/lib/__tests__/arrow-ipc-fixtures.ts`. (`--filesOnly` is required at this point in the sequence — plain `vitest list` imports each test file and `setupFiles`, and at Phase 1 they still contain `jest.*` calls that throw `ReferenceError: jest is not defined`.)

### Phase 2 — Mechanical pass
6. Rewrite `src/test-setup.ts` per Design §9 and get **one** small suite green end-to-end first (`src/lib/__tests__/units.test.ts` — no mocks) to validate config, then one that depends on the global setup mocks without overriding them locally (`src/routes/__tests__/MapsPage.test.tsx` — no local `react-router-dom` mock, and it does reach `@/lib/config`; see Testing Strategy item 7). `src/hooks/__tests__/useScreenConfig.test.tsx` is not a valid check here — its own `jest.mock('react-router-dom', …)` fully overrides the setup-file mock, and it is converted later in Phase 3 (its `vi.hoisted`/`importActual` needs are covered by steps 10-11).
7. Apply the `jest.*` → `vi.*` rename across the 56 test files (Design §4).
8. Apply the type renames (Design §3) — `Mock`, `MockedFunction` imported from `vitest`.

### Phase 3 — Non-mechanical pass
9. Convert `src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts` to ESM imports and update its doc comment; convert the 5 `require()`-in-factory call sites (Design §6). This file is type-checked (not excluded by `tsconfig.json`), so the switch to typed static imports puts it under `strict: true` for the first time — budget time for `yarn type-check` errors here, not just at Phase 4 step 14.
10. Convert the 12 `vi.hoisted()` sites (Design §5).
11. Convert the 6 `importActual` / `importMock` sites (Design §7).

### Phase 4 — Converge
12. `yarn test` and work the failure list, which is expected to be dominated by strict partial mocks (Design §8). Fix per Design §8's preference order.
13. Update the stale comments (Design §10).
14. `yarn lint`, `yarn type-check`, `yarn build`, then `python3 build/analytics_web_ci.py` for the full CI path.
15. Update documentation (see below).

### Phase 5 — Cleanup verification
16. `grep -rn 'jest' analytics-web-app/src analytics-web-app/tests analytics-web-app/package.json` returns only `@testing-library/jest-dom` hits.
17. `yarn test:coverage` produces a report over `src/**/*.{ts,tsx}`.

## Files to Modify

**Delete**
- `analytics-web-app/jest.config.js`
- `analytics-web-app/src/__mocks__/styleMock.js`

**Create**
- None (see Design §3 — no ambient globals file is needed for this migration).

**Config / metadata**
- `analytics-web-app/package.json` (deps, resolutions, scripts)
- `analytics-web-app/vite.config.ts` (`test` block + type reference)
- `analytics-web-app/.eslintrc.json` (`ignorePatterns`)
- `analytics-web-app/yarn.lock`

**Test infrastructure**
- `analytics-web-app/src/test-setup.ts`
- `analytics-web-app/src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts`

**Test files** — all 56 matched by `testMatch` (55 under `src/**/__tests__/`, 1 at `tests/lib/screens-api.test.ts`).

**Docs**
- `analytics-web-app/README.md:94`
- `mkdocs/docs/contributing.md:116,205`
- `mkdocs/docs/development/build.md:120`

**Unchanged, deliberately**
- `build/analytics_web_ci.py` and `.github/workflows/analytics-web-app.yml` — both go through `yarn test`.
- `analytics-web-app/tsconfig.json` — kept as-is; test files stay outside the type-checked program (see Design §3). A `types` allowlist or a second tsconfig is a separable follow-up, not needed here.
- `grafana/` — stays on Jest.

## Trade-offs

**`test` block in `vite.config.ts` vs. a sibling `vitest.config.ts`.** A `vitest.config.ts` takes precedence over `vite.config.ts` rather than layering on it, so it would have to restate the alias set and the React plugin — the exact duplication the migration is meant to remove. `mergeConfig` is awkward here because `vite.config.ts` default-exports a function of `{ mode }`. Cost of the chosen approach: test config sits in the build config file. Mitigated by the triple-slash type reference, which keeps `vitest` out of the build's runtime module graph.

**`globals: true` vs. explicit imports in 55 files.** Explicit imports are the tidier long-term shape and are what a greenfield Vitest project would do, but they add 55 files of churn to a migration whose diff is already wide, and RTL's auto-cleanup depends on a global `afterEach`. Chosen: `globals: true`, matching the issue's scope. A later change can move to explicit imports independently.

**Keeping the `react-markdown` / `remark-gfm` stubs.** Under Vitest these packages would load natively — the stubs exist because Jest could not require them. But `MarkdownCell` tests assert against the stub's simplified HTML output, so removing them means rewriting assertions. Out of scope; noted as a follow-up.

**Explicit `include` patterns vs. Vitest's default.** Mirroring `testMatch` is slightly less conventional than the default `*.test.*` glob, but the default silently drops `arrow-ipc-fixtures.ts`'s 3 tests. Silent test loss is worse than an unconventional glob. The alternative — splitting that file's tests into `arrow-ipc-fixtures.test.ts` — is a reasonable follow-up but is orthogonal churn here.

**Second test runner in the monorepo.** `grafana/` gets Jest from the Grafana plugin toolchain and cannot easily move. Two runners for two unrelated toolchains is the accepted cost; the alternative (migrating `grafana/` too) is a much larger change against upstream-managed config.

## Documentation

- `analytics-web-app/README.md:94` — "Run Jest tests" → "Run Vitest tests"; add `test:watch` / `test:coverage` rows while there.
- `CLAUDE.md` / `AI_GUIDELINES.md` — the analytics-web-app sections list `yarn test` without naming the runner, so no edit is required. Worth a scan for a stale "Jest" mention at implementation time.
- `mkdocs/docs/contributing.md:116` — `yarn test               # Jest unit tests` → `# Vitest unit tests`.
- `mkdocs/docs/contributing.md:205` — `cd analytics-web-app && yarn test        # Jest unit tests` → `# Vitest unit tests`.
- `mkdocs/docs/development/build.md:120` — `yarn test           # Jest unit tests` → `# Vitest unit tests`.
- The checked-in generated site under `mkdocs/site/` regenerates from these sources; no direct edit needed there.

## Testing Strategy

The test suite *is* the artifact under test, so verification is parity against the recorded baseline:

1. **Discovery parity** — `yarn vitest list --filesOnly` reports 56 files. Compare against the `testMatch` file list before and after.
2. **Count parity** — `yarn test` reports **56 passed / 1167 passed**. Any lower number means silently skipped tests, not a fix. Zero skipped, zero todo.
3. **No accidental relaxations** — during Phase 4, resist making a failing assertion pass by loosening it. A strict-partial-mock error is a missing export in a factory, not a wrong expectation.
4. **Coverage runs** — `yarn test:coverage` completes and reports over `src/**/*.{ts,tsx}`.
5. **Build unaffected** — `yarn build` succeeds and `dist/` output is unchanged in shape (the `test` block must not leak into the bundle; spot-check that no `vitest` chunk appears).
6. **Full CI path** — `python3 build/analytics_web_ci.py` green; separately, confirm interactive `yarn test` at a terminal runs once and exits rather than entering watch mode.
7. **Global-setup mocks** — load-bearing, not a sanity check (Design §9): confirm with a test file that has no local `react-router-dom` mock (e.g. `src/routes/__tests__/MapsPage.test.tsx`) and one that does (`ScreenPage.urlState.test.tsx`) that setup-file mocks apply and that a per-file re-mock still takes precedence, matching Jest.

Wall-clock is recorded for information only (baseline 6.66 s); it is not a gate. If the run turns out dramatically slower, `pool: 'threads'` is the first knob.

## Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Strict partial mocks throw on missing exports | High — expect a double-digit failure count on first run | Design §8; the error message names the module and the export |
| Setup-file mock is defeated by an import-order cache hit (a mocked module gets imported by `test-setup.ts` itself before the `vi.mock` registers) | Low — `test-setup.ts` currently imports only `@testing-library/jest-dom`, `util`, `stream/web`; neither mocked module is among them | Keep setup-file imports minimal; re-check this invariant if a new import is added to `test-setup.ts`; confirmed in Phase 2 step 6 |
| Vitest default `include` silently drops `arrow-ipc-fixtures.ts` | Medium if `include` is left implicit | Explicit patterns + the Phase 1 step 5 discovery check |
| `test.alias` object form prefix-matches an unintended subpath import | Low | Switch to array-of-regex form |
| `magicast` (via `@vitest/coverage-v8`) pulls in unpinned `@babel/parser`/`@babel/types` | Low | No CVE currently forces a pin; add one to `resolutions` targeting these two packages if one surfaces — the old `@babel/core` pin does not cover them |
| `micromegas-datafusion-wasm` resolution under jsdom | Low — the import is lazy and never reached from a test | If it surfaces, add a `test.alias` stub or `test.server.deps.inline` |
| `jsdom@^26.1.0` is untested against `vitest@4.1.10` (Vitest's own `devDependencies` pin `jsdom: ^27.4.0`, a major ahead) | Medium — no upstream compatibility signal either way | In-scope fallback, not a strict follow-up: bump the `jsdom` devDep to a Vitest-tested `^27.x` if the jsdom environment fails to initialize or behaves inconsistently |

## Open Questions

None outstanding. The three questions from the previous draft are resolved: sequencing vs. #1347 is covered by the Overview's sequencing note (#1347 is unlanded as of this writing; read `react-router` for `react-router-dom` throughout if it lands first), test-file type-checking status quo is asserted in Design §3, and `@babel/preset-react` removal is folded into Design §2.
