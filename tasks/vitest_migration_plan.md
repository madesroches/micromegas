# Jest → Vitest Migration Plan (analytics-web-app)

**Issue**: [#1345](https://github.com/madesroches/micromegas/issues/1345)
**Goal**: Replace Jest with Vitest as the test runner for `analytics-web-app`, so tests are parsed by the same engine that builds the app and ESM-only dependencies stop requiring per-package carve-outs.

**Success condition**: `yarn test` runs 56 suites / 1167 tests green under Vitest, `yarn lint`, `yarn type-check` and `yarn build` all pass, `build/analytics_web_ci.py` completes, and no Jest/Babel package remains in `package.json`'s `dependencies`/`devDependencies`, including the now-unreachable `@babel/plugin-transform-modules-systemjs`, `@babel/core` and `handlebars` `resolutions` pins (Design §2).

## Overview

`analytics-web-app` builds with Vite 8 but tests with Jest 30 through `ts-jest` + `babel-jest`. Every ESM-only dependency has to be hand-carved out of Jest's CommonJS pipeline (`jest.config.js:23-25` already does this for `d3-dsv`; `react-router@8` in #1347 needs the same plus a `babel-plugin-transform-import-meta` shim). Vitest reads the app's own `vite.config.ts`, so that class of problem disappears: the alias set, the React plugin, and `import.meta` support are already configured for the build and are reused verbatim.

**Sequencing note (also covers the tables in Design §5, §7, §9):** as of this writing, #1347 is still open — `jest.config.js` carries only the `d3-dsv` carve-out and `package.json:41` still pins `react-router-dom@^7.18.0` — but both issues state #1347 lands first as a security fix. #1347's scope is the `react-router-dom` → `react-router` rename across 29 files, including the `jest.mock('react-router-dom')` sites; that rename is #1347's job, not this plan's. If #1347 has landed by the time this migration starts, read `react-router` everywhere this plan says `react-router-dom` (the §5 hoisted-factory rows tied to router mocks, all `react-router-dom` rows in the §7 table, and the two `react-router-dom` mentions in §9). Whichever way it goes, Phase 5 step 19 re-checks #1347's scope against actual state before this PR opens — see "Re-evaluating #1347". Separately, the repo-root `package.json` also carries a `react-router` `resolutions` pin, but root `workspaces` is `["grafana", "typescript/*"]` — `analytics-web-app` is a separate Yarn project with its own lockfile and `resolutions` — so that root pin cannot reach it either way.

This is **not** a performance change. Measured baseline on this branch: **56 suites, 1167 tests**, roughly **5-7 s** (machine-dependent — 6.66 s and 4.6 s have both been measured across runs). There is no wall-clock target and no regression gate.

## Current State

### Jest configuration (`analytics-web-app/jest.config.js`)

| Line | Setting | Purpose |
|---|---|---|
| 3-6 | `testEnvironment: 'jsdom'`, `testEnvironmentOptions.url` | jsdom at `http://localhost:3000` |
| 7 | `setupFilesAfterEnv: ['<rootDir>/src/test-setup.ts']` | polyfills + two global module mocks |
| 9 | `'\\.css$' → src/__mocks__/styleMock.js` | 4 CSS imports exist in `src/` (`main.tsx:9`, `src/components/ui/DateTimePicker.tsx:5-6`, `src/components/XYChart.tsx:3`) |
| 10 | `'^@/(.*)$' → src/$1` | duplicates `vite.config.ts:45` |
| 11-13 | stubs for `react-markdown`, `remark-gfm`, `@radix-ui/react-dropdown-menu` | ESM-only / jsdom-hostile packages |
| 15-22 | `ts-jest` (ESM) + `babel-jest` with `@babel/preset-env` | the transform pipeline being deleted |
| 23-25 | `transformIgnorePatterns: node_modules/(?!(d3-dsv)/)` | the ESM treadmill |
| 26 | `extensionsToTreatAsEsm: ['.ts', '.tsx']` | needs no Vitest counterpart (Vitest is ESM natively); disappears with the file — this is the setting behind §10's "this repo's Jest runs in ESM mode" comment |
| 27-31 | `testMatch` (3 patterns) | see "Test discovery" below |
| 32-35 | `collectCoverageFrom` | `src/**/*.{ts,tsx}` minus `.d.ts` |

`package.json:12-14` — `test: jest`, `test:watch: jest --watch`, `test:coverage: jest --coverage`.
`.eslintrc.json` ignores `jest.config.js`.

### Test discovery — a silent-loss trap

56 files match `testMatch`. All are named `*.test.ts(x)` **except one**:

- `src/lib/__tests__/arrow-ipc-fixtures.ts` — exports fixture helpers (lines 103, 154, 200) **and** contains its own `describe`/`it` blocks (lines 212-276).

Vitest's default `include` is `['**/*.{test,spec}.?(c|m)[jt]s?(x)']`, which would silently drop that file and its 3 tests (56 → 55 suites). Rather than carry Jest's `testMatch` shape forward as a custom `include`, this plan splits that one file so the default glob is correct — see Design §1a and the Trade-offs.

`src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts` is **not** in a `__tests__` directory and is correctly not collected.

The other 55 files are already default-compatible: 54 are `*.test.ts(x)` under `src/**/__tests__/`, and `tests/` holds exactly one file, `tests/lib/screens-api.test.ts`.

### Jest API surface (measured across `src/` + `tests/`)

| API | Uses | Notes |
|---|---|---|
| `jest.fn` | 271 | mechanical → `vi.fn` |
| `jest.mock` | 64 grep hits / 63 real factories | mechanical name, but see hoisting/`require` below; the 64th hit is the doc comment at `cell-registry-mock.ts:6` |
| `jest.Mock` (type) | 30 in 7 files | **not** in the issue's table; needs `import type { Mock } from 'vitest'` |
| `jest.resetAllMocks` | 13 | mechanical |
| `jest.clearAllMocks` | 12 | mechanical |
| `jest.MockedFunction` (type) | 7 | → `MockedFunction` from `vitest` |
| `jest.restoreAllMocks` | 5 | mechanical |
| `jest.requireActual` | 5 | → `await importOriginal()` inside factories (async — changes shape); see Design §7 |
| `jest.requireMock` | 1 | module-level, not in a factory → `await import(...)`; see Design §7 |
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
- `browserslist` + `baseline-browser-mapping` — still required by `autoprefixer@10.5.0`. **Keep both** — keep the `baseline-browser-mapping` pin and the `browserslist` devDependency (only `baseline-browser-mapping` actually carries a `resolutions` pin; `browserslist` is a direct devDependency with none).
- `jsdom@26.1.0` — currently only reachable via `jest-environment-jsdom`. Vitest 4 declares `jsdom: '*'` as a permissive **peer** (no compatibility signal) and does not bundle it, so it must become a direct devDep. Latest is `29.1.1` as of this writing, three majors ahead — pinning to today's transitive `^26.1.0` is intended to preserve current jsdom behaviour while swapping the runner. However, `vitest@4.1.10`'s own `devDependencies` pin `jsdom: ^27.4.0` — i.e. the version Vitest 4 is actually developed and tested against is a major above `^26.1.0` — so this pairing is untested upstream, not merely conservative. See the corresponding Risks table row; bumping further to `29.x` remains a separate follow-up, but bumping to a Vitest-tested `^27.x` is an in-scope fallback if the `^26.1.0` pairing misbehaves.

### Registry / compatibility

- `vitest@4.1.10`, `@vitest/coverage-v8@4.1.10`; peers `vite ^6 || ^7 || ^8` (app has `vite@8.0.16`), engines `node ^20 || ^22 || >=24`. `.nvmrc` pins `20` — fine.
- `@testing-library/jest-dom@6.9.1` (installed) already exposes a `./vitest` export.
- No other workspace uses Vitest; `grafana/` stays on Jest via the Grafana plugin toolchain. This introduces a second runner to the monorepo — acceptable, the two toolchains are unrelated.

### CI

`build/analytics_web_ci.py` runs `yarn install / type-check / lint / test / build`; `.github/workflows/analytics-web-app.yml` just calls that script. **No CI change needed** — Vitest's `configDefaults` already fall back to run-once mode in CI and when stdin isn't a TTY, which covers both the script and GitHub Actions. The script is still `vitest run` so that an interactive developer running `yarn test` locally lands in run-once mode too, matching `jest`'s default.

`tsconfig.json:35-42` excludes `src/**/*.test.ts(x)`, the `__tests__` directories, and `src/test-setup.ts` — but **not** `__test-utils__` or `__mocks__` directories, which have no matching exclude pattern and so are type-checked today (confirmed with `tsc --noEmit --listFilesOnly`, which includes `src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts` and all three `src/__mocks__/*` files). So `yarn type-check` skips `*.test.ts(x)` / `__tests__` content both today and after the migration, but does check `__test-utils__` / `__mocks__` content in both cases. See "Types" below and Phase 3 step 10.

`tsconfig.json:35-42`'s `__tests__` exclude patterns only reach three directory levels deep (`src/__tests__`, `src/*/__tests__`, `src/*/*/__tests__`); the 4-deep `src/lib/screen-renderers/cells/__tests__` and `src/components/layout/TimeRangePicker/__tests__` stay out of the checked program only because every file in them is separately named `*.test.ts(x)`, which is excluded by its own pattern. The conclusion above holds today, but a future non-`*.test.*` file placed in one of these deep `__tests__` directories would silently enter the checked program.

### Coverage baseline

`yarn test:coverage` is already broken today, on this branch, independent of this migration — verified by running it: 55 of 56 suites fail with `TypeError: … minimatch is not a function` from `test-exclude/index.js:99`, reached via `babel-plugin-istanbul` → `@jest/transform`. Cause is the `minimatch: ^10.2.1` `resolutions` pin (kept in Design §2 "via ESLint"): `yarn why minimatch` shows `test-exclude@6.0.0` among its consumers, and v10 no longer exports a callable default. `@vitest/coverage-v8@4.1.10`'s own dependency tree (`obug`, `std-env`, `magicast`, `tinyrainbow`, `@vitest/utils`, `istanbul-reports`, `@bcoe/v8-coverage`, `ast-v8-to-istanbul`, `istanbul-lib-report`, `istanbul-lib-coverage`) has no `test-exclude` and no `minimatch`, and dropping Jest removes `test-exclude` from the tree entirely — so this migration incidentally *fixes* coverage rather than reproducing a working baseline.

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
  // No `include` — Vitest's default '**/*.{test,spec}.?(c|m)[jt]s?(x)' covers
  // all 56 suites once arrow-ipc-fixtures.ts is split (Design §1a).
  // Vitest 4's default `exclude` dropped `dist/`, `cypress/`, `.cache/` and
  // config-file patterns, leaving only node_modules + .git. analytics-web-app/dist/
  // holds no matching test file today (latent, not broken), but pin this
  // explicitly rather than rely on that staying true — a literal array avoids
  // reintroducing a `vitest/config` import for `configDefaults`.
  exclude: ['**/node_modules/**', '**/.git/**', '**/dist/**'],
  // Test-only stubs; merged with resolve.alias, so '@' is inherited.
  alias: {
    'react-markdown': path.resolve(__dirname, './src/__mocks__/react-markdown.tsx'),
    'remark-gfm': path.resolve(__dirname, './src/__mocks__/remark-gfm.ts'),
    '@radix-ui/react-dropdown-menu': path.resolve(
      __dirname,
      './src/__mocks__/@radix-ui/react-dropdown-menu.tsx'
    ),
    // Rejecting stub — reproduces today's swallowed loadWasmEngine() failure
    // deterministically instead of depending on jsdom's fetch support; see
    // the decision in the Notes below.
    'micromegas-datafusion-wasm': path.resolve(
      __dirname,
      './src/__mocks__/micromegas-datafusion-wasm.ts'
    ),
  },
  coverage: {
    provider: 'v8',
    include: ['src/**/*.{ts,tsx}'],
    exclude: [
      'src/**/*.d.ts',
      'src/**/__mocks__/**',
      'src/**/__tests__/**',
      'src/**/__test-utils__/**',
      'src/lib/datafusion-wasm/**',
      'src/main.tsx',
      'src/router.tsx',
      'src/components/layout/TimeRangePicker/types.ts',
    ],
  },
},
```

Notes:
- `vitest@4.1.10`'s `coverageConfigDefaults.exclude` is `[]` — there's no default list being replaced here. Vitest separately appends its own un-overridable exclusions (all `setupFiles`, all `test.include` patterns, config files, `**/node_modules/**`), which covers `src/test-setup.ts` and every `*.test.ts(x)` file — but `test.include` only matches files *named* `*.test.ts(x)`, so `src/lib/__tests__/arrow-ipc-fixtures.ts` (kept in place by Design §1a as a non-test fixture helper once its `describe` block moves out) is not auto-excluded; `src/**/__tests__/**` stays in the list above for exactly that reason. `src/main.tsx`, `src/router.tsx`, and `TimeRangePicker/types.ts` are added explicitly since none of the automatic exclusions reach them and they would otherwise show as 0%-covered noise in the report. The selection criterion for this explicit list: entry points, plus modules that export types only — a module that exports a runtime value (e.g. a `const`) stays in coverage even if it sits next to type-only siblings, which is why `src/components/map/modes/types.ts` (exports the runtime `MAP_MODE_LABELS` at `:31-34`, re-exported by `map/modes/index.ts:12`) is not on this list.
- `test.alias` is merged with `resolve.alias`, so the `@/` mapping is inherited from `vite.config.ts:45` — the duplicate in `jest.config.js:10` disappears. Keeping the stubs in `test.alias` (not `resolve.alias`) is what prevents them leaking into the production build. For a same-key entry — `micromegas-datafusion-wasm` is also aliased for the real build, at `vite.config.ts:46` — `test.alias` overrides that entry rather than merging with it (verified against vitest 4.1.10), so tests resolve the stub below, not the linked package.
- Vite string alias keys match exactly or as a directory prefix. None of the three packages is imported with a subpath here, so the object form is safe; if a subpath import appears later, switch `test.alias` to the array-of-regex form (`{ find: /^react-markdown$/, replacement: … }`).
- The `\\.css$ → styleMock.js` mapping is dropped: Vitest's default is `css: { include: [] }`, not `css: false` — but the resulting behavior is the same one described here (an empty module for every CSS import, including CSS pulled in from `node_modules`), so deleting `src/__mocks__/styleMock.js` stays safe.
- `globals: true` is load-bearing beyond avoiding 55 import edits: `@testing-library/react`'s auto-`cleanup` registers itself through the global `afterEach`. Without globals, DOM would leak across tests in every render-based suite.
- The `wasm-content-type` and `log-base-path` plugins only implement `configureServer` and are inert under test. `loadWasmEngine`, declared at `src/lib/wasm-engine.ts:8`, uses a lazy dynamic `import('micromegas-datafusion-wasm')` at `:10` — this import **is** reached from tests: `useWasmEngine.ts:20-23` calls it unconditionally inside a `useEffect` guarded only by `if (engine) return`, with `engine` initialised to `null` at `:17`; `NotebookRenderer.tsx:317` calls `useWasmEngine()`; and `NotebookRenderer.test.tsx` renders `NotebookRenderer` 26 times through the helper at `:123-131`, without mocking `@/lib/wasm-engine` or `./useWasmEngine`. Today the failure is silently swallowed — `wasm-engine.ts:15-18` nulls `enginePromise` and re-throws, and the actual swallow happens at `useWasmEngine.ts:29-33`, which sets `engineError` at `:31` — and no test asserts on the resulting `engineError`, which `NotebookRenderer.tsx:710-712` renders into the DOM — but without a stub, `resolve.alias` here resolves the specifier for real, to the linked package (`type: module`, `main: micromegas_datafusion_wasm.js`), so `mod.default()` actually runs wasm-bindgen init, which does `new URL('micromegas_datafusion_wasm_bg.wasm', import.meta.url)` and needs `fetch` under jsdom. The `micromegas-datafusion-wasm` entry in `test.alias` above is what keeps this from becoming a real per-test failure, not a contingency fallback.

  **Decision: the stub's `default()` rejects.** This reproduces today's behavior exactly — `loadWasmEngine()` rejects, `wasm-engine.ts:15-18` swallows it, `engine` stays `null`, `engineError` is set, and no execution path changes. Rationale: this migration is a runner swap, and the plan's established principle is one concern per change. A *resolving* stub would instead make `engine` non-null, which activates `useCellExecution.ts:374-379`'s auto-execution of every cell in every render (`if (!hasExecutedRef.current && cells.length > 0 && engine !== null) executeFromCell(0)`), plus an unawaited `engine.reset()` at `:315` (a missing method becomes an unhandled rejection), the `registerTable` closure at `:206-207`, and `engine?.deregister_table` at `:425`/`:450`, which the rename/delete tests do reach — silently switching on auto-execution across 26 renders is exactly the kind of scope creep the "one concern" principle rules out. A stub is still required rather than none at all: without it, Vitest resolves the real linked package and runs into the `fetch`-under-jsdom failure described above, which is jsdom-dependent rather than deterministic; the rejecting stub makes that failure deterministic and fast instead. Concretely: `src/__mocks__/micromegas-datafusion-wasm.ts` exports a `default` that is an async function which rejects (or throws) with a clear message (e.g. `"micromegas-datafusion-wasm is stubbed out in tests"`); no `WasmQueryEngine` class is needed on this path.

  **Documented alternative: a resolving stub.** If a later change wants the engine present in tests, the stub must implement `default()` plus a `WasmQueryEngine` class satisfying the four `NotebookQueryEngine` methods (`useCellExecution.ts:11-16`: `register_table`, `execute_and_register` returning `Promise<Uint8Array>`, `deregister_table`, `reset`) — mirror `createMockEngine` at `src/lib/screen-renderers/__tests__/useCellExecution.test.ts:71-79` — and must additionally verify: no new "not wrapped in act(...)" warnings (the baseline has zero), and no cells auto-executing that previously did not. Fixing the genuinely-broken wasm path (broken today, silently) is a follow-up, not this migration's job.

### 1a. Split `arrow-ipc-fixtures.ts` so the default `include` is correct

`src/lib/__tests__/arrow-ipc-fixtures.ts` is the only file in the repo that Vitest's default glob would miss — verified: it is the sole non-`*.test.*` file in any `__tests__` directory, and `tests/` holds only `screens-api.test.ts`. Two ways to keep its 3 tests:

- **Custom `include` mirroring `testMatch`** — encodes Jest's "every `.ts` under `__tests__` is a suite" convention, which has no Vitest equivalent. It also inverts the trap it guards: any future helper dropped into a `__tests__` directory silently becomes a suite.
- **Split the file** (chosen) — move the self-test block at lines 211-276 (comment at 211, `describe` at 212-276) into a new sibling `src/lib/__tests__/arrow-ipc-fixtures.test.ts` that imports the three helpers it exercises (`createDictionaryFramedIpc`, `createPlainFramedIpc`, `combineChunks`) from `./arrow-ipc-fixtures`. All three are already exported, and the fixtures module has exactly one other importer (`arrow-stream-dictionary.test.ts:10`), which is unaffected.

Suite and test counts are preserved exactly: `arrow-ipc-fixtures.ts` stops being a suite and `arrow-ipc-fixtures.test.ts` becomes one, so the parity gate stays **56 suites / 1167 tests**. The config then needs no `include` at all, and the runner's discovery rule matches the file-naming convention the other 55 files already follow.

### 2. Dependency changes

Remove from `devDependencies`: `@babel/core`, `@babel/preset-env`, `@babel/preset-react`, `@types/jest`, `babel-jest`, `jest`, `jest-environment-jsdom`, `ts-jest`. `@babel/preset-react` is verified dead independent of Jest: it's not referenced by `jest.config.js`, nothing in `yarn.lock` depends on it transitively, and `@vitejs/plugin-react`'s only dependency is `@rolldown/pluginutils` — no build path touches it. Remove it in this pass.
Add to `devDependencies`: `jsdom@^26.1.0` (today's transitive version, pinned deliberately to preserve current jsdom behaviour during the runner swap — see Current State → Dependency reality check; note this is a major below the `^27.4.0` Vitest 4.1.10 itself is developed against, so treat an environment-setup failure as an expected possibility, not a surprise — the in-scope fallback is bumping to a Vitest-tested `^27.x`; bumping further to the current `29.1.1` remains a separate follow-up), `vitest@4.1.10`, `@vitest/coverage-v8@4.1.10` (exact versions, not caret — `@vitest/coverage-v8@4.1.10`'s peer dependency on `vitest` is the literal string `4.1.10`, and any version skew between the two emits a Yarn `YN0060 INCOMPATIBLE_PEER_DEPENDENCY` warning, which collides with Phase 1 step 1's "clean, warning-free install").
Remove from `resolutions`: `@babel/plugin-transform-modules-systemjs` (unreachable once `@babel/preset-env` is gone), `@babel/core` (also unreachable once the Jest packages and the direct devDep are gone — `@vitest/coverage-v8`'s Babel surface is `magicast` → `@babel/parser`/`@babel/types`, neither of which pulls in `@babel/core`; see Current State → Dependency reality check), `handlebars` (per `yarn why handlebars`, its only consumer is `ts-jest@29.4.9`, which this pass deletes — same unreachable-pin reasoning as `@babel/core`).
**Keep** the `baseline-browser-mapping` pin in `resolutions` and the `browserslist` devDependency (not itself a `resolutions` pin), both of which `autoprefixer` still needs. The rest of `resolutions` was checked against `yarn why` and survives this pass for its own reasons: `tar` / `undici` via `node-gyp`, `ws` via the newly-direct `jsdom`, `ajv` / `js-yaml` / `minimatch` / `brace-expansion` / `flatted` via ESLint, `postcss` via tailwindcss/vite. Separately, the `rollup` pin is already dead today regardless of this migration — there is no `rollup` anywhere in `yarn.lock` (Vite 8 uses Rolldown) — so it is pre-existing and out of scope, not a live pin this migration is responsible for.

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

`vitest/globals` is not an `@types/*` package, so it is not picked up automatically. But `yarn tsc --noEmit --listFilesOnly` shows the only test-adjacent files in the checked program are the three `src/__mocks__/*` files and `src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts` — none of which references `describe`/`it`/`expect`/`vi`. `tsconfig.json:35-42` **excludes** `src/**/*.test.ts(x)`, the `__tests__` directories, and `src/test-setup.ts` — the only file that will reference `vi` — from that same program. (The `__tests__` exclude patterns only reach three directory levels deep; the 4-deep `__tests__` directories under `cells/` and `TimeRangePicker/` stay out of the program only because every file in them happens to be named `*.test.ts(x)` — see Current State → CI.) A `/// <reference>` inside a `.d.ts` only augments the program that contains it, so no new ambient file is needed today: nothing in the checked program uses a Vitest global.

Decision: skip adding a dedicated ambient file. If a future edit adds a `vi`/`describe`/`it`/`expect` reference to a checked-in file, fold `/// <reference types="vitest/globals" />` into the existing `src/vite-env.d.ts` (which already carries `/// <reference types="vite/client" />` and is part of the same program) rather than creating a new file. Test files remain outside the type-checked program before and after this migration, so `yarn type-check` / CI behavior is unchanged; only editor-level hints for `describe`/`it`/`expect` inside test files are affected, and a `tsconfig.test.json` with `"types": ["vitest/globals"]` that brings test files into a checked program would address that as a separable follow-up, not part of this migration.

Type-level renames:
- `jest.MockedFunction<typeof f>` → `MockedFunction<typeof f>` with `import type { MockedFunction } from 'vitest'` (7 uses, 6 files).
- `jest.Mock` → `Mock` with `import type { Mock } from 'vitest'` (30 uses, 7 files: `auth.test.tsx`, `AuthGuard.test.tsx`, `notebook-cell-view.test.ts`, `table-utils.test.tsx`, `PerformanceAnalysisPage.test.tsx`, `HorizontalGroupCell.test.tsx`, `useCellExecution.test.ts`).

### 4. Mechanical renames

`jest.fn` → `vi.fn`, `jest.mock` → `vi.mock`, `jest.clearAllMocks` → `vi.clearAllMocks`, `jest.resetAllMocks` → `vi.resetAllMocks`, `jest.restoreAllMocks` → `vi.restoreAllMocks`, `jest.useFakeTimers` / `useRealTimers` / `advanceTimersByTime` → `vi.*`. Jest's config defaults for `clearMocks` / `resetMocks` / `restoreMocks` are all `false`, matching Vitest's defaults — but that only covers *automatic* per-test resetting, which neither runner does unless a test calls one of the `*AllMocks` functions itself. The invariant that actually matters is the 13 **manual** `resetAllMocks` call sites: verified against `@vitest/spy@4.1.10`'s source and both projects' docs, Vitest's `mockReset()` **restores** the implementation originally passed to `vi.fn(impl)`, whereas Jest's **replaces it with a no-op**. `restoreAllMocks` does not carry this divergence: per the Vitest 4 migration guide, `vi.restoreAllMocks` "no longer resets the state of spies and only restores spies created manually with `vi.spyOn`," which aligns it with Jest — `@vitest/spy@4.1.10`'s `mockReset` behavior above is reached only via `mockReset`/`resetAllMocks`, not `restoreAllMocks`. It doesn't bite today — no file that calls `resetAllMocks` also *consumes* a `vi.fn(impl)` whose implementation matters — but that is the invariant to preserve, not the config-defaults point, so a future editor adding such a combination knows to re-check this divergence rather than relying on the (correct but irrelevant) config-defaults framing. The distinction is currently invisible rather than trivially true: `src/test-setup.ts:52-54` creates three `jest.fn(impl)` mocks (`useNavigate`, `useLocation`, `useSearchParams`) in every test file's registry, including the three that call `resetAllMocks` (`maps-catalog.test.ts`, `LogRenderer.test.tsx`, `PerformanceAnalysisPage.test.tsx`) — it's inert only because none of those three touches the `react-router-dom` hooks the setup file mocks.

Fake timers carry a similar narrow callout: Vitest's `toFake` default excludes `nextTick` and `queueMicrotask`, while Jest's default fakes both. The rename here is safe specifically because `VariableCell.test.tsx:201` uses a single synchronous `advanceTimersByTime(300)` inside `act()` with no microtask flush — a case where the two runners' fake-timer scopes don't diverge in practice.

A single `sed`-style pass over the 56 test files plus `src/test-setup.ts` covers ~360 of the ~400 call sites. Everything below is what the pass must **not** touch.

**Line-broken `jest` references — a naive `jest.` → `vi.` regex misses these six sites** because the identifier and the member access are split across lines:
- `src/components/map/__tests__/MapHoverTooltip.test.tsx:65-67` — `const spy = jest\n  .spyOn(HTMLElement.prototype, 'getBoundingClientRect')` (the one real `jest.spyOn` call, see the API surface table above)
- `src/lib/__tests__/maps-catalog.test.ts:155,239,290` — `jest\n  .fn()` (3 sites)
- `src/routes/__tests__/MapsPage.test.tsx:81,116` — `jest\n  .fn()` (2 sites)

The mechanical pass must match a bare `jest` identifier regardless of trailing whitespace/newline before the `.`, not just the literal string `jest.`.

**Type-position references — 37 sites the sweep must also leave alone:** 30 `jest.Mock` (28 `as jest.Mock` casts plus two property-type annotations, `onChange?: jest.Mock` / `onChildSelect?: jest.Mock`, at `HorizontalGroupCell.test.tsx:134,136`) and 7 `jest.MockedFunction<typeof f>` (Design §3's count). These are converted to `Mock` / `MockedFunction` by the type-rename step, not by this sweep — a bare-identifier regex would otherwise turn them into `vi.Mock` / `vi.MockedFunction`, which are not members of `vi`, and nothing downstream catches it: `tsconfig.json:35-42` excludes these test files from `tsc --noEmit`, `.eslintrc.json` sets no `parserOptions.project` (no type-aware lint rules), type annotations are erased at transform time so nothing throws at runtime, and Phase 5 step 17's `grep -rn 'jest'` sees `vi.Mock`, not `jest`. Sequencing: Implementation step 8 (type renames, Design §3) runs before step 9 (this sweep, Design §4), so by the time the sweep runs these 37 sites are already `Mock` / `MockedFunction` and the bare-`jest` pattern no longer matches them.

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

`cell-registry-mock.ts` sits under `__test-utils__`, which — unlike `__tests__` and `*.test.ts(x)` — is not excluded by `tsconfig.json` (see Current State → CI above), so it is already part of the `tsc --noEmit` program today. Its two `require()` calls are currently implicitly `any`; converting them to typed static imports puts this 326-line helper under `strict: true` for the first time. Budget for `yarn type-check` to newly surface errors here (see Phase 3 step 10).

Call sites become async factories with a dynamic import:

```ts
vi.mock('../cell-registry', async () => {
  const { createCellRegistryMock } = await import('../__test-utils__/cell-registry-mock')
  return createCellRegistryMock({ withRenderers: true, withEditors: true })
})
```

Sites: `NotebookRenderer.test.tsx:98`, `useCellExecution.test.ts:66`, `notebook-utils.test.ts:18`, `CellContainer.test.tsx:6`, `HorizontalGroupCell.test.tsx:72-78` (the `jest.mock(` call opens at `:72`). All seven `eslint-disable-next-line @typescript-eslint/no-var-requires` comments go stale once `require()` is gone and come out with it: the two `no-require-imports, no-var-requires` comments in the helper (`cell-registry-mock.ts:9,13`) plus one at each of the five call sites (`CellContainer.test.tsx:5`, `useCellExecution.test.ts:65`, `notebook-utils.test.ts:17`, `NotebookRenderer.test.tsx:97`, and `HorizontalGroupCell.test.tsx:73` — the last one sits *inside* the arrow-function body, unlike the other four, which precede the `require()` line directly). `yarn lint` is plain `eslint .` with no `--report-unused-disable-directives`, and Phase 5 step 17's grep only looks for `jest`, so nothing else catches a leftover disable comment — remove all seven as part of this conversion.

### 7. `requireActual` / `requireMock` — 6 async conversions

The Vitest idiom inside a factory is the `importOriginal` helper the factory receives as its argument, not a bare `vi.importActual` call — it is typed against the real module and needs no module specifier repeated:

```ts
vi.mock('../notebook-utils', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../notebook-utils')>()
  return { ...actual, evaluateTemplate: vi.fn(actual.evaluateTemplate) }
})
```

Five of the six sites below are inside `vi.mock` factories and take this form; the factory becomes `async`. Only `arrow-utils.test.ts:114` sits at module top level with no enclosing factory, so it stays a direct call:

| File | Line | Module |
|---|---|---|
| `src/test-setup.ts` | 51 | `react-router-dom` |
| `src/hooks/__tests__/useScreenConfig.test.tsx` | 11 | `react-router-dom` |
| `src/routes/__tests__/ScreenPage.urlState.test.tsx` | 21 | `react-router-dom` |
| `src/lib/screen-renderers/__tests__/useNotebookVariables.test.tsx` | 30 | `react-router-dom` — combined with its §5 row above; see below |
| `src/lib/screen-renderers/__tests__/table-utils.test.tsx` | 22 | `../notebook-utils` |
| `src/lib/__tests__/arrow-utils.test.ts` | 114 | `jest.requireMock('apache-arrow')` at module top level (not in a factory) → `await import('apache-arrow')`, which the hoisted `vi.mock` factory already intercepts; test files are ESM, so top-level `await` is available. The destructured `__test__` helper does not exist on the real module's types, so this needs a cast either way |

**`useNotebookVariables.test.tsx` is one combined conversion, not two independent one-liners.** Its `react-router-dom` factory (lines 29-61) both needs `vi.hoisted()` for the outer `mockInitialSearch` binding (Design §5) *and* `await importOriginal()` in place of `jest.requireActual` (Design §7) — but the factory also calls `useState`, `useMemo`, `useRef`, and `useCallback`, imported from `react` at line 16. Hoisted `vi.mock` factories run before the file's own top-level imports are initialised, so those bindings cannot be closed over directly. The factory must become `async` and pull them in itself:

```ts
const { mockInitialSearch, setMockInitialSearch } = vi.hoisted(() => ({
  mockInitialSearch: { current: '' },
  setMockInitialSearch: (v: string) => { mockInitialSearch.current = v },
}))
vi.mock('react-router-dom', async (importOriginal) => {
  const { useState, useMemo, useRef, useCallback } = await import('react')
  const actual = await importOriginal<typeof import('react-router-dom')>()
  return {
    ...actual,
    useSearchParams: (): [URLSearchParams, SetSearchParamsFn] => {
      const [raw, setRaw] = useState(mockInitialSearch.current)
      // ...unchanged body, reading/writing mockInitialSearch.current instead of the bare outer `let`
    },
  }
})
```

Converting the outer binding also reaches outside the factory: the test file's own bare `let mockInitialSearch = ''` (line 27) is reassigned in four places — `:94` (inside `beforeEach`), `:116`, `:154`, `:229` — each a plain `mockInitialSearch = '...'` assignment. Once the binding becomes `{ current: '' }` inside `vi.hoisted()`, every one of these four sites is a type error against the object and must be rewritten as `mockInitialSearch.current = '...'` (or a call to `setMockInitialSearch('...')`).

### 8. Partial mocks become strict — the main expected source of failures

Under Jest's CJS interop, importing a name a partial factory did not return yields `undefined`. Vitest throws:

> `No "X" export is defined on the "Y" mock. Did you forget to return it from vi.mock?`

The throw fires at **property-access time** on the mock's `Proxy` (`callFunctionMock`'s `get` trap in `vitest@4.1.10`'s bundled runner, `dist/chunks/startVitestModuleRunner.DB-7oCpn.js:319-337`), not at import time: Vitest deliberately overrides Vite's own eager missing-export check to a no-op for mocked modules (`processImport(exports) { return exports }` at `:526`, with the comment "Vite checks that the module has exports emulating the Node.js behaviour, but Vitest is more relaxed"), and Vite's `analyzeImportedModDifference` is only reached when `"externalize" in fetchResult`, which never holds for a factory-mocked module. So a value-position import of an omitted export is necessary but not sufficient for a throw — the importer's code path must also actually evaluate that reference during the test run, not merely import the module.

Every one of the 63 `vi.mock` factories that returns a subset of a module's exports is exposed to this, but only when the importer actually pulls in the missing name at the **value** position — a name imported only as a type is erased by the transform and never triggers the runtime check (e.g. the `apache-arrow` factory at `arrow-utils.test.ts:6-99` omits only `Duration` — it returns `Timestamp` and `Table` classes directly. `arrow-utils.ts:5` imports `Duration` via a value-syntax `import { … }`, not `import type` — but its only reference is the type assertion at `:97`, and specifier elision removes it from the emitted output because `tsconfig.json` sets `isolatedModules: true` with no `verbatimModuleSyntax`, so it never throws). The traced examples below demonstrate the mechanism end-to-end rather than predict which suites will fail — every traced chain in fact resolves to "does not fire" once the importer's actual code path is checked, each for its own reason (a state the suite never drives into, a registry-mock gap, an early return, absent props). That does not shrink the risk: it's evidence that verdicts have to be traced per case, and there are 63 factories, of which only a handful are traced here:

- `@/lib/arrow-stream` — `NotebookRenderer.test.tsx:9-11` returns only `streamQuery`, omitting `fetchQueryIPC` and `executeStreamQuery`. `NotebookRenderer.tsx` pulls in `useCellExecution.ts`, which imports `fetchQueryIPC` at a value position and calls it at `useCellExecution.ts:217`/`:254`, inside the `runQuery`/`runQueryAs` closures (`:209-274`) — real value-position exposure, but `NotebookRenderer.test.tsx`'s registry mock (line 98) omits `withSqlExecution`, so `meta.execute` resolves to `simpleExecuteStub` (`cell-registry-mock.ts:119`) and those closures are never entered by this test file, so it does not fire here. (`executeStreamQuery` has no importer in this test's reachable graph at all — it's used only by the perf-analysis route components — so its omission carries no known call path.)
- `apache-arrow` — `NotebookRenderer.test.tsx:14-20` returns only a `Table` class, omitting `tableFromIPC`. `useCellExecution.ts:2` imports `tableFromIPC` in a value position and calls it at `:214`, `:234`, `:252`, `:271`, inside the same `runQuery`/`runQueryAs` closures as the `fetchQueryIPC` bullet above; `NotebookRenderer.tsx:25` pulls in `useCellExecution`. The `:214` call site sits in the `isNotebookSource` (local WASM) branch, which does not call `fetchQueryIPC` first, so at the code-structure level it is independently reachable — not masked by the `fetchQueryIPC` gap already documented in the `@/lib/arrow-stream` bullet above. But the same registry-mock gating applies: `NotebookRenderer.test.tsx` never sets `withSqlExecution`, so neither closure is entered and this does not fire for this test file either. Same exposure class as the bullet above — real, but conditional on a code path this suite doesn't take. (`useStreamQuery.test.ts`, `useCellExecution.test.ts`, and `arrow-utils.test.ts` also mock `apache-arrow`, at breadth-risk level only, not separately confirmed.)
- `lucide-react` — 6 files (the sixth is `src/components/ui/__tests__/DateTimePicker.test.tsx`), each enumerating a fixed icon list; several carry confirmed value-position exposure, not just breadth risk:
  - `NotebookRenderer.test.tsx:23-44` omits `Check`; `NotebookRenderer.tsx:35` imports `./NotebookSourceView`, which imports `Check` (`:2`) and renders it at `:85` — but only when `showSource && copied` (`NotebookRenderer.tsx:714-719`, `NotebookSourceView.tsx:85`). Does not fire: the suite never renders `NotebookSourceView` at all — the `showSource` gate at `NotebookRenderer.tsx:714` is never driven, and there are zero `showSource` references in the test file.
  - The same `NotebookRenderer.test.tsx:23-44` factory also omits `AlertTriangle`, needed at value position by `src/components/ErrorBanner.tsx:1` (used `:29`), in-graph via `screen-renderers/shared.tsx:3`. Does not fire, for two reasons: `ErrorBanner.tsx:29` is `const Icon = isWarning ? AlertTriangle : AlertCircle`, and the sole call site, `RendererLayout` (`shared.tsx:55-79`) rendering `ErrorBanner` at `:68`, never passes `variant`, so `AlertTriangle` is never evaluated there; and no test file renders any of `RendererLayout`'s four consumers (`MetricsRenderer.tsx:212`, `ProcessListRenderer.tsx:407`, `LogRenderer.tsx:610`, `TableRenderer.tsx:412`) at all.
  - `src/components/__tests__/CellContainer.test.tsx:9-21` omits `Plus`, imported by `CellContainer.tsx:3`. `CellContainer.tsx:257`/`:266` gate its uses on `{onInsertAbove && …}` / `{onInsertBelow && …}`, and the test's `defaultProps` (`:26-31`) is `name`/`type`/`status`/`children` with zero `onInsert` occurrences anywhere in the file. Does not fire.
  - `cells/__tests__/HorizontalGroupCell.test.tsx:5-19` omits `Database`, `AlertCircle` and `AlertTriangle`. `Database`/`AlertCircle` are needed by `DataSourceSelector.tsx:2` (`:93`, `:110` — this file imports only those two, not `AlertTriangle`) reached via `HorizontalGroupCell.tsx:38` → rendered at `:348`; `AlertTriangle` is needed separately by `ErrorBanner.tsx:1` — same reasoning as the bullet above (`ErrorBanner.tsx:29` only evaluates `AlertTriangle` when `variant === 'warning'`, which no `RendererLayout` call site passes, and no test renders a `RendererLayout` consumer), so that half does not fire either. For `Database`/`AlertCircle`: `DataSourceField` early-returns `null` at `DataSourceSelector.tsx:30` when `sources.length <= 1 && !hasVariables && !showNotebookOption`, and the test's `getDataSourceList` mock (`:61-63`) never settles while `datasourceVariables`/`showNotebookOption` are never passed. Does not fire.

  (`CustomRange.test.tsx:15` returns only `Calendar`, but `CustomRange.test.tsx:4-13` carries its own `jest.mock('@/components/ui/DateTimePicker', …)`, so the real `DateTimePicker.tsx` — and its `Clock` import at `:4` — is never loaded. This is the same exemption described above for type-erased imports, applied here to a second mock in the same file: an intermediate module that the importing test file itself mocks is never loaded, so this is not a throw.)
- `@dnd-kit/core` / `@dnd-kit/sortable` / `@dnd-kit/utilities` — three separate factories in `NotebookRenderer.test.tsx` (`:49-57`, `:59-79`, `:81-87`); the `@dnd-kit/sortable` factory omits `horizontalListSortingStrategy`, used in a value position at `cells/HorizontalGroupCell.tsx:172`, inside `SortableContext`'s `strategy` prop, which sits past the empty-children early return at `:157` — so this fires only once the rendered notebook has an `hg` cell with children, a state `NotebookRenderer.test.tsx` never creates. `HorizontalGroupCell.test.tsx:49` does supply `horizontalListSortingStrategy: jest.fn()` in its own `@dnd-kit/sortable` mock, so it carries no exposure here.
- `@/components/layout`, `@/lib/auth` — `MapsPage.test.tsx`, `PerformanceAnalysisPage.test.tsx`
- `@/lib/data-sources-api` — 4 call sites, factories return 1 of 8 real exports
- `@/lib/api` — 3 call sites
- `@/lib/arrow-utils` — 1 of 15 real exports returned; confirmed value-position exposure at `log-utils.test.ts:10`, which returns only `timestampToDate` — `log-utils.tsx:8` imports `./table-utils`, which imports and calls `isTimeType`, `isNumericType`, `isBinaryType`, `isDurationType`, `durationToMs` (`table-utils.tsx:14-21`, used at `:730`, `:757`, `:762-763`, `:767`, `:782`). Five of the six calls are in `formatCell` (`:754-811`); `:730` is in `TableBody` (`:661-745`), also never rendered by that file — `log-utils.test.ts` imports only `LEVEL_NAMES`, `formatLocalTime`, `getLevelColor`, `formatLevelValue`, `classifyLogColumns` — so this does not fire in practice for that file.
- `../cell-registry` — 5 helper-based sites (Design §6) plus one independent inline factory at `notebook-cell-view.test.ts:14`, which is safe (it returns only `getCellTypeMetadata`, the only name `notebook-cell-view.ts:3` imports)
- `@/lib/perfetto-trace` — `cells/__tests__/PerfettoExportCell.test.tsx:8-10` returns only `fetchPerfettoTrace`, omitting `triggerTraceDownload`; `triggerTraceDownload` is referenced only in `downloadCachedBuffer` (`PerfettoExportCell.tsx:93-97`, wired to the "Download Instead" button at `:234`) and `handleDownloadTrace` (`:151-193`, the SplitButton secondary action at `:252`). `PerfettoExportCell.test.tsx` clicks only `/Open in Perfetto/` and `/Dismiss/`; its one "Download Instead" test (`:295-312`) merely asserts the button is present. Does not fire.

(`@/lib/config` exports exactly `getConfig` and `appLink`; all three factories mocking it return both, so despite its earlier billing here it carries zero strict-mock exposure.)

Fix per occurrence, in preference order: (a) add the missing export to the factory when the point of the mock is a narrow stub; (b) spread the real module when the mock only means to override one or two names — `vi.mock('m', async (importOriginal) => ({ ...(await importOriginal<typeof import('m')>()), foo: vi.fn() }))`, the same `importOriginal` idiom as Design §7. This is per-error work, not a sweep — budget the bulk of the migration time here, on the strength of the 63-factory breadth rather than the traced examples above, none of which are expected to throw.

### 9. `src/test-setup.ts` specifics

- `import '@testing-library/jest-dom'` → `import '@testing-library/jest-dom/vitest'`.
- The two global `jest.mock` calls (`@/lib/config` at line 42, `react-router-dom` at line 50) become `vi.mock`, with `vi.hoisted()` for `mockNavigate` and an async factory spreading `importOriginal()` for `react-router-dom` (Design §7). `vi.mock` in a setup file applying to every test file is documented and explicitly sanctioned, not merely an implementation fact: the automocking section of `docs/api/vi.md` says "To replicate Jest's automocking behaviour, you can call `vi.mock` for each required module inside `setupFiles`" (and its `vi.unmock` entry refers to modules "defined in `setupFiles`"), and `docs/config/experimental.md` says "Vitest only reads test files and setup files while looking for `vi.mock` or `vi.hoisted`." The one caveat worth naming: hoisting goes through Vite's module runner by default (`experimental.viteModuleRunner` defaults to `true`), but the Node-loader path (`experimental.nodeLoader`) also supports `vi.mock` in test and setup files — precisely this plan's usage — so the setup-file mock does not depend on that default. Given that, the check in Phase 2 step 7 is ordinary verification, not a load-bearing correctness gate. The real, documented constraint is different — **a module already imported by the setup file is cached before the mock is registered and cannot then be mocked.** `src/test-setup.ts` today imports only `@testing-library/jest-dom`, `util`, and `stream/web`, so neither `@/lib/config` nor `react-router-dom` is pre-imported and both mocks are safe; keep it that way as new setup-file imports are added. The 3 test files which re-mock `react-router-dom` themselves keep winning either way: their `vi.mock` is hoisted within a file that runs after setup, so the last registration wins — same precedence as Jest today. (A fourth site, `test-setup.ts:50` itself, is the global mock being overridden, not a re-mock.)
- `process.env.NODE_ENV = 'development'` (line 39) can be dropped: nothing in `src/` reads `NODE_ENV`, Vitest runs in mode `test` so `import.meta.env.DEV` is `true`, and React resolves to its development build because `process.env.NODE_ENV !== 'production'` at runtime. One-line revert if anything regresses. `src/lib/config.ts:32` does read `import.meta.env.DEV`, but it's inert under test because `@/lib/config` is globally mocked at `test-setup.ts:42-45` and no test imports the real module. Separately, `vite.config.ts:7`'s `loadEnv(mode, process.cwd(), '')` uses an empty prefix, so it passes all of `process.env` through to the test transform's `define` (`vite.config.ts:41`) — meaning a developer with `MICROMEGAS_BASE_PATH` exported (as `start_analytics_web.py` documents) now has it reach test config, which Jest never did. Harmless for the same mocking reason, and no `.env*` file exists in the repo.
- The `TextEncoder` / `TextDecoder` / web-streams polyfills (lines 2-3 and 5-16) are almost certainly redundant under Node 20/22 + Vitest's jsdom environment, but they are idempotent — **leave them alone in this change** to keep the diff to one concern. Switch the specifiers to `node:util` / `node:stream/web` while touching the file. The `@ts-expect-error` directives above them may become "unused directive" errors, but `tsconfig.json` excludes `src/test-setup.ts`, so `tsc --noEmit` will not see it.
- The `DOMRect` polyfill (lines 18-35, including its comment and `if` guard) stays.
- Update the trailing comment (lines 57-58) — it references `jest testEnvironmentOptions.url` and `jest.spyOn`.

### 10. Stale comments referencing Jest config

`src/components/__tests__/CellContainer.test.tsx:23` and `src/lib/screen-renderers/__tests__/NotebookRenderer.test.tsx:46` both say "mocked via moduleNameMapper in jest.config.js" → point them at `test.alias` in `vite.config.ts`.

`src/lib/screen-renderers/__tests__/table-utils.test.tsx:18-19` also survive the mechanical rename as plain comments — "…identical output. `jest.spyOn`" and "can't be used here — this repo's Jest runs in ESM mode and module-namespace…" — and would otherwise match Phase 5 step 17's cleanup grep. The technical claim still holds under Vitest; reword "this repo's Jest" to "this repo's test runner" (or similar) without changing the underlying reasoning.

## Implementation Steps

### Phase 1 — Toolchain swap
1. `package.json`: apply the dependency and script changes from Design §2; run `yarn install` and confirm a clean, warning-free install.
2. `vite.config.ts`: add the triple-slash reference and the `test` block from Design §1, including the `micromegas-datafusion-wasm` entry in `test.alias` — this is expected work, not a contingency: the dynamic import it stubs out is reached from `NotebookRenderer.test.tsx` (26 renders), not merely a theoretical risk. Add the corresponding stub at `src/__mocks__/micromegas-datafusion-wasm.ts`, whose `default` is an async function that rejects — reproducing today's swallowed `loadWasmEngine()` failure rather than resolving it (Design §1's decision).
3. No ambient globals file to add (Design §3) — skip.
4. Delete `jest.config.js` and `src/__mocks__/styleMock.js`; in `.eslintrc.json`'s `ignorePatterns`, remove `jest.config.js` and add `coverage` — `@vitest/coverage-v8`'s default reporters include `html`, and `istanbul-reports`' html reporter copies asset files (`block-navigation.js`, `sorter.js`, `vendor/prettify.js`) into the report directory; ESLint 8 in eslintrc mode does not read `.gitignore` (why `dist` is listed explicitly today), so without this entry `yarn lint` lints those assets too. Add a `coverage/` entry to `.gitignore` — Vitest's `coverage.reportsDirectory` defaults to `./coverage`, and no entry exists today (`git check-ignore analytics-web-app/coverage` exits 1), so Phase 5 step 18 and Testing Strategy item 4 would otherwise leave an untracked `coverage/` tree.
5. Split `src/lib/__tests__/arrow-ipc-fixtures.ts` per Design §1a: move its self-test block (lines 211-276) into a new `src/lib/__tests__/arrow-ipc-fixtures.test.ts` importing `createDictionaryFramedIpc`, `createPlainFramedIpc` and `combineChunks` from `./arrow-ipc-fixtures`. The moved block uses only `describe`/`it`/`expect` — zero `jest` references — so it needs no rename.
6. Confirm discovery before touching test bodies: `yarn vitest list --filesOnly` must report 56 files, including the new `src/lib/__tests__/arrow-ipc-fixtures.test.ts` and *excluding* `arrow-ipc-fixtures.ts`. (`--filesOnly` is required at this point in the sequence — plain `vitest list` imports each test file and `setupFiles`, and at Phase 1 they still contain `jest.*` calls that throw `ReferenceError: jest is not defined`.)

### Phase 2 — Mechanical pass
7. Rewrite `src/test-setup.ts` per Design §9 and get **one** small suite green end-to-end first (`src/lib/__tests__/units.test.ts` — no mocks, zero `jest` references, genuinely clean at this point) to validate config, then one that depends on the global setup mock without overriding it locally (`src/routes/__tests__/MapsPage.test.tsx` — no local `react-router-dom` mock, so it validates the setup-file `react-router-dom` mock; see Testing Strategy item 7). Unlike `units.test.ts`, `MapsPage.test.tsx` still carries 9 `jest` references at this point in the sequence (four `jest.mock` factories at `:12`/`:21`/`:28`/`:34`, `jest.restoreAllMocks()` at `:48`, `jest.fn()` at `:52`/`:65`, and the line-broken `jest\n  .fn()` at `:81`/`:116`) — since the full mechanical sweep is step 8, apply that rename to this one file first, then run it as the checkpoint. Note `MapsPage.test.tsx:21-24` does carry its own local `jest.mock('@/lib/config', …)` (post-rename, `vi.mock`) returning `basePath: '/mmlocal'`, so this check exercises that local mock, not the setup-file `@/lib/config` one — no substitute file is needed for that, though, since `@/lib/config`'s setup-file mock has zero strict-mock exposure anyway (Design §8). `src/hooks/__tests__/useScreenConfig.test.tsx` is not a valid check here — its own `jest.mock('react-router-dom', …)` fully overrides the setup-file mock, and it is converted later in Phase 3 (its `vi.hoisted`/`importActual` needs are covered by steps 11-12).
8. Apply the type renames (Design §3) — `Mock`, `MockedFunction` imported from `vitest`.
9. Apply the `jest.*` → `vi.*` rename across the 56 test files (Design §4); the 37 type-position sites converted in step 8 are already `Mock`/`MockedFunction` at this point, so the bare-`jest` sweep does not touch them (see §4's "must not touch" list).

### Phase 3 — Non-mechanical pass
10. Convert `src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts` to ESM imports and update its doc comment; convert the 5 `require()`-in-factory call sites (Design §6). This file is type-checked (not excluded by `tsconfig.json`), so the switch to typed static imports puts it under `strict: true` for the first time — budget time for `yarn type-check` errors here, not just at Phase 4 step 15.
11. Convert the 12 `vi.hoisted()` sites (Design §5).
12. Convert the 6 `importActual` / `importMock` sites (Design §7) — `importOriginal` inside factories, a direct `await import(...)` at `arrow-utils.test.ts:114`.

### Phase 4 — Converge
13. `yarn test` and work the failure list, which is expected to be dominated by strict partial mocks (Design §8). Fix per Design §8's preference order.
14. Update the stale comments (Design §10).
15. `yarn lint`, `yarn type-check`, `yarn build`, then `python3 build/analytics_web_ci.py` for the full CI path.
16. Update documentation (see below).

### Phase 5 — Cleanup verification
17. `grep -rn 'jest' analytics-web-app/src analytics-web-app/tests analytics-web-app/package.json` returns only `@testing-library/jest-dom` hits.
18. `yarn test:coverage` produces a report over `src/**/*.{ts,tsx}` — confirming newly-working behavior, not parity with a working baseline (see Current State → Coverage baseline).
19. **Before opening the PR, re-evaluate [#1347](https://github.com/madesroches/micromegas/issues/1347)** — see "Re-evaluating #1347" below.

## Re-evaluating #1347 before the PR

#1347 bumps `react-router-dom@^7.18.0` → `react-router@^8.3.0` for `GHSA-qwww-vcr4-c8h2` (high severity, Dependabot alert #388). Its scope is three parts, and this migration changes the value of only one of them:

1. The dependency bump and the `react-router-dom` → `react-router` rename across 29 files, plus the module name in the `vi.mock('react-router-dom')` sites — **still needed either way.**
2. Adding `react-router` to `transformIgnorePatterns` and `babel-plugin-transform-import-meta` to the `babel-jest` chain — **dead once this migration lands.** Both exist only because Jest's CJS pipeline cannot `require` a pure-ESM package and then trips on the two `import.meta.hot` guards in `lib/dom/ssr/routeModules.js`. Vitest loads it natively; `jest.config.js` and the whole Babel chain are deleted by Design §2.
3. The "land first, don't let a test-runner migration gate a security fix" sequencing argument — worth re-checking against actual state rather than assumed state.

Decide between these, in this order:

- **If #1347 has already landed:** nothing to decide about its scope. Instead, confirm this PR *removes* what it added — `babel-plugin-transform-import-meta` from `devDependencies` and its `transformIgnorePatterns` entry (both vanish with `jest.config.js`) — and that the plan's `react-router` renaming (Overview sequencing note) was applied throughout.
- **If #1347 has not landed and this migration is ready to merge first:** propose narrowing #1347 to part 1 only. Adding part 2 would mean landing four lines of Jest config plus a devDependency that this PR deletes in the same week. Post that reasoning on #1347 rather than silently rescoping it, and do not make this PR do the bump — the two changes stay separable so a revert of either is clean.
- **If #1347 has not landed and the security clock is the binding constraint:** leave #1347 exactly as scoped and let it land first, accepting the throwaway config. A high-severity alert open for longer is worse than four lines of config churn.

**#1347 is not a candidate for closing** in any branch of this decision: the alert stays open until `react-router@>=8.3.0` is in `analytics-web-app/yarn.lock`, and repo policy is to never dismiss a Dependabot alert. The question is its scope and ordering, not whether the vulnerability gets fixed.

## Files to Modify

**Delete**
- `analytics-web-app/jest.config.js`
- `analytics-web-app/src/__mocks__/styleMock.js`

**Create**
- `analytics-web-app/src/lib/__tests__/arrow-ipc-fixtures.test.ts` — the self-tests moved out of `arrow-ipc-fixtures.ts` (Design §1a)
- `analytics-web-app/src/__mocks__/micromegas-datafusion-wasm.ts` — stub aliased in `test.alias` (Design §1); `default()` rejects, reproducing today's swallowed `loadWasmEngine()` failure rather than resolving it (Design §1's decision); the dynamic import it replaces is reached from `NotebookRenderer.test.tsx`, not a contingency
- No ambient globals file (see Design §3 — none is needed for this migration).

**Config / metadata**
- `analytics-web-app/package.json` (deps, resolutions, scripts)
- `analytics-web-app/vite.config.ts` (`test` block + type reference)
- `analytics-web-app/.eslintrc.json` (`ignorePatterns` — remove `jest.config.js`, add `coverage`)
- `analytics-web-app/.gitignore` — needs a `coverage/` entry; `git check-ignore analytics-web-app/coverage` exits 1 today, yet the plan gates on `yarn test:coverage` twice (Testing Strategy item 4, Phase 5 step 18)
- `analytics-web-app/yarn.lock`

**Test infrastructure**
- `analytics-web-app/src/lib/__tests__/arrow-ipc-fixtures.ts` (self-tests removed; fixture exports unchanged)
- `analytics-web-app/src/test-setup.ts`
- `analytics-web-app/src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts`

**Test files** — all 56 matched by `testMatch` (55 under `src/**/__tests__/`, 1 at `tests/lib/screens-api.test.ts`). Of these, ~22 contain no `jest` reference at all and need no edits under `globals: true`, so "all 56" overstates the actual edit surface — the rest need at least the mechanical rename (Design §4).

**Docs**
- `analytics-web-app/README.md:7,94`
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

**Splitting `arrow-ipc-fixtures.ts` vs. an explicit `include`.** Mirroring Jest's `testMatch` would avoid touching any test file, but it makes a Jest convention permanent config and leaves the inverse trap (a future `__tests__` helper silently becoming a suite). Splitting the file is a ~20-line move in one file, keeps counts identical, and lets the config carry no `include` at all. Chosen: split (Design §1a). Cost: one extra file in the diff, and the fixtures module and its new test file must stay in sync if helpers are renamed.

**Second test runner in the monorepo.** `grafana/` gets Jest from the Grafana plugin toolchain and cannot easily move. Two runners for two unrelated toolchains is the accepted cost; the alternative (migrating `grafana/` too) is a much larger change against upstream-managed config.

## Documentation

- `analytics-web-app/README.md:94` — "Run Jest tests" → "Run Vitest tests"; add `test:watch` / `test:coverage` rows while there.
- `analytics-web-app/README.md:7` — "Node.js 18+" → "Node.js 20+": `jest@30`'s engines (`^18.14.0 || ^20 || ^22 || >=24`) support Node 18 today; `vitest@4.1.10`'s engines (`^20.0.0 || ^22.0.0 || >=24.0.0`) don't. `CONTRIBUTING.md:358` and `mkdocs/docs/contributing.md:238` already say "Node.js 20+" — this app README is the outlier.
- `CLAUDE.md` / `AI_GUIDELINES.md` — the analytics-web-app sections list `yarn test` without naming the runner, so no edit is required. Worth a scan for a stale "Jest" mention at implementation time.
- `mkdocs/docs/contributing.md:116` — `yarn test               # Jest unit tests` → `# Vitest unit tests`.
- `mkdocs/docs/contributing.md:205` — `cd analytics-web-app && yarn test        # Jest unit tests` → `# Vitest unit tests`.
- `mkdocs/docs/development/build.md:120` — `yarn test           # Jest unit tests` → `# Vitest unit tests`.
- `mkdocs/site/` is gitignored, not checked in; it regenerates from these sources, so no direct edit needed there.

## Testing Strategy

The test suite *is* the artifact under test, so verification is parity against the recorded baseline:

1. **Discovery parity** — `yarn vitest list --filesOnly` reports 56 files. Compare against the `testMatch` file list before and after: the only difference must be `arrow-ipc-fixtures.ts` → `arrow-ipc-fixtures.test.ts` (Design §1a).
2. **Count parity** — `yarn test` reports **56 passed / 1167 passed**. Any lower number means silently skipped tests, not a fix. Zero skipped, zero todo.
3. **No accidental relaxations** — during Phase 4, resist making a failing assertion pass by loosening it. A strict-partial-mock error is a missing export in a factory, not a wrong expectation.
4. **Coverage newly works** — `yarn test:coverage` completes and reports over `src/**/*.{ts,tsx}`. This verifies newly-working behavior, not parity: `yarn test:coverage` fails today on this branch with a `minimatch is not a function` error (see Current State → Coverage baseline), so there is no working baseline to match.
5. **Build unaffected** — `yarn build` succeeds and `dist/` output is unchanged in shape (the `test` block must not leak into the bundle; spot-check that no `vitest` chunk appears).
6. **Full CI path** — `python3 build/analytics_web_ci.py` green; separately, confirm interactive `yarn test` at a terminal runs once and exits rather than entering watch mode.
7. **Global-setup mocks** — ordinary verification (Design §9): confirm with a test file that has no local `react-router-dom` mock (e.g. `src/routes/__tests__/MapsPage.test.tsx`, which does carry its own local `@/lib/config` mock — that one validates the setup-file `react-router-dom` mock only) and one that does (`ScreenPage.urlState.test.tsx`) that the setup-file `react-router-dom` mock applies and that a per-file re-mock still takes precedence, matching Jest.

Wall-clock is recorded for information only (baseline ~5-7 s, machine-dependent); it is not a gate. If the run turns out dramatically slower, `pool: 'threads'` is the first knob.

## Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Strict partial mocks throw on missing exports | Medium — every traced chain in §8 resolves to "does not fire" once the importer's actual code path is checked, but that's exposure analysis on a handful of factories, not a survey of all 63; the main expected source of failures rests on that breadth, not on the traced examples | Design §8; the error message names the module and the export |
| Setup-file mock is defeated by an import-order cache hit (a mocked module gets imported by `test-setup.ts` itself before the `vi.mock` registers) | Low — `test-setup.ts` currently imports only `@testing-library/jest-dom`, `util`, `stream/web`; neither mocked module is among them | Keep setup-file imports minimal; re-check this invariant if a new import is added to `test-setup.ts`; confirmed in Phase 2 step 7 |
| `arrow-ipc-fixtures.ts`'s 3 tests are lost if the split (Design §1a) is skipped or botched, since no custom `include` backstops them | Medium | The Phase 1 step 6 discovery check (56 files, `arrow-ipc-fixtures.test.ts` present) runs before any test body is touched, and Testing Strategy item 2's count parity gate catches a partial move |
| `test.alias` object form prefix-matches an unintended subpath import | Low | Switch to array-of-regex form |
| `magicast` (via `@vitest/coverage-v8`) pulls in unpinned `@babel/parser`/`@babel/types` | Low | No CVE currently forces a pin; add one to `resolutions` targeting these two packages if one surfaces — the old `@babel/core` pin does not cover them |
| `micromegas-datafusion-wasm` resolution under jsdom | Medium — `useWasmEngine`'s effect is unconditional and the import **is** reached from `NotebookRenderer.test.tsx` (26 renders); under Jest the failure is silently swallowed, but without a stub Vitest's `resolve.alias` resolves the specifier for real and runs wasm-bindgen init, which needs `fetch`/`URL` support under jsdom | Addressed as expected work, not contingency: the `test.alias` stub's `default()` rejects, reproducing today's swallowed failure deterministically instead of depending on jsdom's `fetch` support (Design §1's decision); in Design §1's config block and applied in Phase 1 step 2, not deferred until "if it surfaces" |
| `jsdom@^26.1.0` is untested against `vitest@4.1.10` (Vitest's own `devDependencies` pin `jsdom: ^27.4.0`, a major ahead) | Medium — no upstream compatibility signal either way | In-scope fallback, not a strict follow-up: bump the `jsdom` devDep to a Vitest-tested `^27.x` if the jsdom environment fails to initialize or behaves inconsistently |

## Open Questions

None outstanding. The three questions from the previous draft are resolved: sequencing vs. #1347 is covered by the Overview's sequencing note (#1347 is unlanded as of this writing; read `react-router` for `react-router-dom` throughout if it lands first), test-file type-checking status quo is asserted in Design §3, and `@babel/preset-react` removal is folded into Design §2.
