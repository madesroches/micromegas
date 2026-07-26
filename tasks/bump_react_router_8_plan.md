# Bump react-router to 8.x in analytics-web-app Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1347

## Overview

`analytics-web-app` depends on `react-router-dom@^7.18.0`, which is covered by Dependabot
alert #388 (`GHSA-qwww-vcr4-c8h2`, "RSC Mode CSRF Bypass", high severity, vulnerable range
`>= 7.12.0, < 8.3.0`). The fix is to move to `react-router@^8.3.0` (the package was renamed
from `react-router-dom` to `react-router` starting with v8) and update all imports and mocks
accordingly.

## Current State

- `analytics-web-app/package.json:41` depends on `"react-router-dom": "^7.18.0"`.
- The app already runs React 19 (`react@^19.2.0`), which satisfies react-router 8's peer
  requirement (`react >= 19.2.7`). The current `yarn.lock` already resolves `react`/`react-dom` to
  `19.2.8` — above the `19.2.7` floor — so no `package.json` version bump is expected to be
  needed; Implementation Step 2's conditional bump is a safety net in case a fresh resolution
  picks a lower `19.2.x` patch, not an anticipated action.
- `react-router@8.3.0` declares an `engines.node` floor of `>=22.22.0` (`npm view
  react-router@8.3.0 engines`). `analytics-web-app/.nvmrc` currently pins Node `20`, and both
  `.github/workflows/analytics-web-app.yml` (`node-version-file: 'analytics-web-app/.nvmrc'`) and
  `build/analytics_web_ci.py` (`setup_nvm_and_node`, which installs/uses the exact `.nvmrc`
  version before every check) build and test against that pinned Node 20 — two majors below the
  new floor. Four other places also pinned Node 20 for building `analytics-web-app` and needed the
  same bump: the frontend-builder stages in `docker/analytics-web.Dockerfile`,
  `docker/monolith.Dockerfile`, and `docker/all-in-one.Dockerfile` (each `FROM
  node:20-alpine`), and `docker/github-runner.Dockerfile`'s nodesource `setup_20.x` install (used
  for CI runner builds, not `.nvmrc`-driven). This change bumps `.nvmrc` to the floating major `22`
  (satisfying the `>=22.22.0` floor) and bumps all four of those Node 20 pins to Node 22, rather
  than trying to keep Node 20 working against a dependency that declares it unsupported.
- 29 files import from `react-router-dom` (components, hooks, routes, and their tests):
  `src/components/AppLink.tsx`, `src/components/AuthGuard.tsx`, `src/components/ErrorBoundary.tsx`,
  `src/components/layout/PivotButton.tsx`, `src/components/layout/Sidebar.tsx`,
  `src/components/map/__tests__/EventDetailPanel.test.tsx`,
  `src/components/map/__tests__/MapHoverTooltip.test.tsx`,
  `src/hooks/__tests__/useScreenConfig.test.tsx`, `src/hooks/useScreenConfig.ts`,
  `src/lib/screen-renderers/LogRenderer.tsx`, `src/lib/screen-renderers/MetricsRenderer.tsx`,
  `src/lib/screen-renderers/NotebookRenderer.tsx`, `src/lib/screen-renderers/ProcessListRenderer.tsx`,
  `src/lib/screen-renderers/TableRenderer.tsx`,
  `src/lib/screen-renderers/__tests__/useNotebookVariables.test.tsx`,
  `src/lib/screen-renderers/useNotebookVariables.ts`, `src/lib/url-cleanup-utils.ts`,
  `src/main.tsx`, `src/router.tsx`, `src/routes/LoginPage.tsx`,
  `src/routes/PerformanceAnalysisPage.tsx`, `src/routes/ProcessLogPage.tsx`,
  `src/routes/ProcessMetricsPage.tsx`, `src/routes/ScreenPage.tsx`, `src/routes/ScreensPage.tsx`,
  `src/routes/__tests__/MapsPage.test.tsx`, `src/routes/__tests__/PerformanceAnalysisPage.test.tsx`,
  `src/routes/__tests__/ScreenPage.urlState.test.tsx`, `src/test-setup.ts`.
- 4 files call `vi.mock('react-router-dom', ...)`: `src/test-setup.ts:46`,
  `src/routes/__tests__/ScreenPage.urlState.test.tsx:20`,
  `src/hooks/__tests__/useScreenConfig.test.tsx:10`,
  `src/lib/screen-renderers/__tests__/useNotebookVariables.test.tsx:31`. Each also has a matching
  `importOriginal<typeof import('react-router-dom')>()` type reference to update.
- The issue's original obstacle — Jest's CJS pipeline choking on `react-router@8`'s pure-ESM
  `import.meta.hot` usage — no longer applies: `analytics-web-app` migrated from Jest to Vitest in
  #1349 (already merged to `main`). There is no `jest.config.js` in the project anymore, and
  Vitest handles ESM-only packages natively, so **no test-runner config changes are needed** for
  this bump (no `transformIgnorePatterns`, no `babel-plugin-transform-import-meta`). This
  significantly narrows the fix versus what the issue originally scoped.
- The root `yarn.lock` react-router alert (#395) is the *same* advisory as #388
  (`GHSA-qwww-vcr4-c8h2`, vulnerable range `>= 7.12.0, < 8.3.0`, confirmed via `gh api
  repos/madesroches/micromegas/dependabot/alerts/395`), not an unrelated one. It flags the root
  workspace's `resolutions.react-router: "^7.18.0"` (`package.json:39`, resolving to 7.18.1) — a
  pin added for `@grafana/ui`'s legacy v5 routing compat shim (`CHANGELOG.md:21`), unconnected to
  `analytics-web-app`. It's out of scope for issue #1347 (scoped to `analytics-web-app`), but needs
  its own follow-up fix — bumping that root resolution to `^8.3.0` and confirming `@grafana/ui`'s
  compat shim still resolves — tracked separately from this change.

## Design

This is a mechanical rename plus a version bump — no architectural change:

1. **`package.json`**: replace the `react-router-dom` dependency entry with
   `"react-router": "^8.3.0"`.
2. **Import rename**: change `from 'react-router-dom'` to `from 'react-router'` in all 29 files
   listed above. The named exports used (`BrowserRouter`, `Routes`, `Route`, `Navigate`,
   `MemoryRouter`, `useNavigate`, `useSearchParams`, `Link`, etc.) are unchanged between
   `react-router-dom@7` and `react-router@8`. Note: v8 does carve `RouterProvider` and
   `HydratedRouter` out into a separate `react-router/dom` subpath, but a repo-wide grep confirms
   this app uses neither (it's declarative `BrowserRouter`/`Routes`/`Route`, not a data router), so
   the blanket rename to `react-router` covers every import used here.
3. **Mock rename**: change the 4 `vi.mock('react-router-dom', ...)` calls to
   `vi.mock('react-router', ...)`, and their paired `importOriginal<typeof import('react-router-dom')>()`
   generic to `import('react-router')`.
4. **Lockfile**: run `yarn install` to regenerate `yarn.lock` with the new dependency resolved.

## Implementation Steps

1. In `analytics-web-app/package.json`, replace the `react-router-dom` line with
   `"react-router": "^8.3.0"`.
2. Run `yarn install` from `analytics-web-app/` to update `yarn.lock`. If yarn reports a peer
   dependency conflict on `react`/`react-dom` (react-router 8 wants `>=19.2.7`), bump
   `react`/`react-dom` to `^19.2.7` in the same commit — still within `analytics-web-app`'s
   existing major version.
3. Bump `analytics-web-app/.nvmrc` from `20` to the floating major `22`, satisfying
   `react-router@8.3.0`'s declared `engines.node >=22.22.0` floor. Both the GitHub Actions
   workflow (`node-version-file: 'analytics-web-app/.nvmrc'`) and local dev (`nvm install` via
   `build/analytics_web_ci.py`'s `setup_nvm_and_node`) read this file directly, so no other config
   needs to change for CI and local builds to pick up the new version.
4. Rename every `'react-router-dom'` import specifier to `'react-router'` across the 29 files in
   **Current State** above. A project-wide search-and-replace of the exact string
   `'react-router-dom'` → `'react-router'` (single-quoted, to avoid touching unrelated substrings)
   covers both plain imports and the `vi.mock`/`importOriginal` call sites.
5. Run `yarn build`, `yarn type-check`, `yarn lint`, and `yarn test` in `analytics-web-app/` and
   fix anything that surfaces (e.g. any behavioral differences between v7 and v8 the issue didn't
   anticipate).
6. Confirm Dependabot alert #388 closes once the bump is merged (Dependabot detects the
   `yarn.lock` change automatically; no manual action beyond merging).
7. Add a `CHANGELOG.md` entry under **Unreleased** / **Build** (the Unreleased section has no
   Security subsection; the prior `react-router-dom` 7.18.0 entry at `CHANGELOG.md:21` — the
   closer, same-file precedent — is itself filed under **Build**), noting the `react-router` bump
   to `^8.3.0` and the resolved Dependabot alert #388.

## Files to Modify

- `analytics-web-app/package.json`
- `analytics-web-app/yarn.lock`
- `analytics-web-app/.nvmrc` (Node 20 → 22 LTS, per Implementation Step 3).
- `docker/analytics-web.Dockerfile`, `docker/monolith.Dockerfile`, `docker/all-in-one.Dockerfile`
  (frontend-builder stage: `node:20-alpine` → `node:22-alpine`), and
  `docker/github-runner.Dockerfile` (`setup_20.x` → `setup_22.x`) — the other places pinning Node
  20 for `analytics-web-app` builds, per Current State.
- The 29 files listed in **Current State** (import rename), 4 of which also need the `vi.mock`
  string updated.
- `analytics-web-app/README.md` (Prerequisites section, per Documentation below).
- `doc/GETTING_STARTED.md` (Prerequisites section, per Documentation below).
- `CHANGELOG.md` (Build entry for the alert #388 fix, per prior react-router bump precedent — see
  Implementation Step 7).

## Trade-offs

- Considered keeping `react-router-dom` pinned below 8 and waiting for #1345/#1349 (Vitest
  migration) to land first, per the issue's own suggested sequencing — but that migration is
  already merged (#1349), so the sequencing concern is moot and this bump can proceed directly
  with no test-runner workaround needed.
- Considered a broader `sed`-style rename vs. per-file edits — a single global replace of the
  quoted string `'react-router-dom'` is safe here because grep confirms no other package or path
  contains that exact substring in this repo's `analytics-web-app` source tree.

## Documentation

No docs reference the `react-router-dom` package name itself, but two docs state Node version
requirements that go stale once `.nvmrc` is bumped to Node 22 (Implementation Step 3):
- `analytics-web-app/README.md`'s Prerequisites section (lines 7 and 9) states "Node.js 20+" and
  "Yarn 4 (Berry) — installed via `corepack enable` (Node 20 ships with corepack)"; update to
  "Node.js 22+" and adjust the corepack parenthetical to reference Node 22.
- `doc/GETTING_STARTED.md`'s Prerequisites section (lines 13 and 14) states "Node.js 18+" and
  "Yarn 4 (Berry) — installed via `corepack enable` (Node 20 ships with corepack)"; this guide
  covers setting up `analytics-web-app` for frontend development (see its "Full Local Stack" /
  "Hybrid Setup" steps, `cd analytics-web-app` at line 138), so update both lines the same way.

## Testing Strategy

- `yarn test` (Vitest) — all suites should pass unchanged; the mock rename is the only test-file
  edit required.
- `yarn build` — verify the production Vite build succeeds with the new package.
- `yarn type-check` — verify TypeScript resolves types from `react-router` correctly.
- `yarn lint` — verify no lint regressions from the import rename.
- Manual smoke navigation pass before merging, since `router.tsx`'s `<Routes>`/`<Route>`/`<Navigate>`
  usage with `AuthGuard`, nested `:name` params, and a catch-all is real routing surface that
  `test-setup.ts`'s mocked `useNavigate`/`useSearchParams` (etc.) don't exercise: verify the login
  redirect via `AuthGuard`, a param route (e.g. `/screen/:name`), and the catch-all/404 all behave
  correctly under `react-router@8`.

## Open Questions

- None — the issue fully scopes the change, and the Jest-specific complexity it anticipated is
  already obsolete following the Vitest migration.
