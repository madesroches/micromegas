# Bump Outdated JS Majors in `analytics-web-app` Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1255

## Overview

Issue #1255 lists seven bump items across `analytics-web-app/` and `grafana/` (4 in `analytics-web-app`,
3 in `grafana/`), including a migration off the deprecated `@grafana/experimental`. Three of the seven
items have since been resolved by unrelated work, one should be deliberately held, and this plan closes
the remaining three — `date-fns`
2 → 4, `eslint` 8 → 10 (which forces the flat-config migration), and `tailwindcss` 3 → 4 — all confined
to `analytics-web-app/`, and records the documented reason for holding the fourth.

The three bumps are independent of each other and land as three atomic implementation commits, each
independently green under `python3 build/analytics_web_ci.py`, plus a fourth, docs-only changelog commit
that the CI gate does not apply to.

## Current State

### Already resolved since the issue was filed (3 of 7; 1 held, 3 remaining below)

| Issue item | Resolution |
|---|---|
| `react-router-dom ^6.30.4` → 7 | Done, and further: migrated to `react-router ^8.3.0` in #1351 (`f4201fd0a`). `react-router-dom` is gone from `analytics-web-app/package.json`. |
| Migrate off deprecated `@grafana/experimental` | Done in #1354 (`82bee5cc6`) — now `@grafana/plugin-ui ^0.16.1`. Verified zero remaining references in `grafana/src`, `grafana/package.json`, and the root `yarn.lock` (`grafana/` has no `yarn.lock` of its own — it is a workspace of the repo-root yarn project). |
| Grafana SDK pinned to 11.6.7 | Done in #1354 — `@grafana/data`/`runtime`/`ui`/`e2e-selectors` are all `12.4.6`. |

`grafana/` is also already on `eslint ^9` + `@typescript-eslint ^8` (via `@grafana/eslint-config ^9`).
`welcome/` is a second ESLint 8 / eslintrc / Tailwind 3 holdout — its `package.json` declares
`eslint ^8.57.0`, `@typescript-eslint/{eslint-plugin,parser} ^7.0.0`, `eslint-plugin-react-hooks ^4.6.0`,
`eslint-plugin-react-refresh ^0.4.5`, and `tailwindcss ^3.3.0`, with a legacy `welcome/.eslintrc.json` and
no `eslint.config.*` — a near-clone of the exact toolchain this plan retires. It is explicitly out of
scope: issue #1255 scopes only `analytics-web-app` and `grafana`, so `welcome/` is left untouched here
(see Trade-offs).

### Remaining, in `analytics-web-app/package.json`

- **`eslint ^8.57.0`** (resolves `8.57.1`). ESLint 8 is end-of-life. Config is still legacy eslintrc:
  `analytics-web-app/.eslintrc.json`, extending `eslint:recommended`,
  `plugin:@typescript-eslint/recommended`, `plugin:react-hooks/recommended`, with two rule overrides
  (`@typescript-eslint/no-unused-vars` and `react-refresh/only-export-components`, both `warn`) and
  `ignorePatterns: ["dist", "coverage", "src/lib/datafusion-wasm"]`.
  Companion devDeps: `@typescript-eslint/{eslint-plugin,parser} ^7.0.0`, `eslint-plugin-react-hooks ^4.6.0`
  (resolves `4.6.2`), `eslint-plugin-react-refresh ^0.4.5`, and `@eslint/eslintrc ^3.3.1` (resolves
  `3.3.5`) — which a repo-wide grep shows is **referenced by nothing** (ESLint 8 bundles its own
  `@eslint/eslintrc@2.1.4` internally), i.e. a dead devDep.
- **`date-fns ^2.30.0`** (resolves `2.30.0`). Exactly one import site:
  `analytics-web-app/src/components/ui/DateTimePicker.tsx:3` —
  `import { format, setHours, setMinutes, startOfDay, endOfDay } from 'date-fns'`.
  **`date-fns@4.1.0` is already in `analytics-web-app/yarn.lock`** (lines 2696 and 2705 hold `^2.30.0`
  and `^4.1.0` respectively): `react-day-picker@9.14.0` depends on `date-fns@^4.1.0`. So the app already
  ships two major versions of this library; the bump is a deduplication, not a new-version risk.
- **`tailwindcss ^3.3.0`** (resolves `3.4.19`). Config in `analytics-web-app/tailwind.config.ts` (TS,
  ~110 lines, almost entirely a `theme.extend.colors` map of `hsl(var(--…))` / `var(--…)` indirections
  plus `borderRadius`, and `plugins: [typography]`). No `corePlugins`, `safelist`, or `separator` keys.
  Wired through `analytics-web-app/postcss.config.mjs` (`tailwindcss` + `autoprefixer`).
  Entry stylesheet `analytics-web-app/src/styles/globals.css` (116 lines) opens with the three
  `@tailwind base/components/utilities` directives, has two `@layer base` blocks, and uses `@apply`
  exactly twice — `* { @apply border-border }` and `body { @apply bg-background text-foreground }`.
- **No `.github/dependabot.yml`** — the issue's closing note still holds; only GitHub's alert-based
  scanning is active. Out of scope here (see Trade-offs).
- The `resolutions` block the issue flagged as a smell is still hand-maintained (11 entries). Out of
  scope here (see Trade-offs).

### The one item to hold, with reason

**`grafana/`'s `react ^18.0.0`** must stay on 18. `@grafana/ui@12.4.6` declares
`peerDependencies: { react: "^18.0.0", react-dom: "^18.0.0" }`, and a Grafana panel/datasource plugin
shares the host Grafana app's React runtime rather than bundling its own — so React 19 in the plugin
would both violate the peer range and risk two-React-copies breakage at runtime. `yarn why react`, run at
the repo root (`grafana/` has no `yarn.lock`/`.yarnrc.yml` of its own — it is a workspace of the root
yarn project), resolves `react@npm:18.3.1 (via npm:^18.0.0)`. This is not a bump to defer-and-revisit; it is
gated on Grafana itself shipping a React 19 SDK, so the acceptance criterion should be recorded as
"documented reason to hold" (which #1255 explicitly allows) rather than left open.

### Verified compatibility of the target versions

Latest published, and their `eslint` peer ranges:

| Package | Current | Target | `eslint` peer |
|---|---|---|---|
| `eslint` | 8.57.1 | 10.8.0 | — (engines: `^20.19.0 \|\| ^22.13.0 \|\| >=24`) |
| `typescript-eslint` (unified) | — (split `^7`) | 8.66.0 | `^8.57.0 \|\| ^9.0.0 \|\| ^10.0.0` |
| `eslint-plugin-react-hooks` | 4.6.2 | 7.1.1 | `… \|\| ^9.0.0 \|\| ^10.0.0` |
| `eslint-plugin-react-refresh` | ^0.4.5 | 0.5.3 | `^9 \|\| ^10` |
| `@eslint/js` | — | 10.0.1 | `^10.0.0` |
| `date-fns` | 2.30.0 | 4.4.0 | — |
| `tailwindcss` | 3.4.19 | 4.3.3 | — |
| `tailwind-merge` | 2.6.1 | 3.6.0 | — |

Node: local `v22.17.0` and `analytics-web-app/.nvmrc` = `22` both satisfy ESLint 10's `^22.13.0`
(`.nvmrc` bare `22` resolves to the latest 22.x under both `nvm` and `actions/setup-node`). No `.nvmrc`
change needed.

`typescript-eslint@8.66.0` requires `typescript: >=4.8.4 <6.1.0`; the repo is on `typescript ^5.4.0`. OK.

## Design

### Why ESLint 10, not 9

The issue was written when 9 was current; 10 is now the current major. ESLint 10 **removes eslintrc
support entirely**, so the flat-config migration is mandatory at either target — going to 9 would mean
doing the same migration and immediately being one major behind. Every plugin in the chain already
declares `^10.0.0` in its peer range (table above). `grafana/` stays on 9 because its ESLint config is
`@grafana/eslint-config`-driven and therefore Grafana's to move, not ours; a version skew between two
independently-linted packages costs nothing.

### ESLint 8 → 10 breaking changes that apply here

The migration skips 9 entirely, so the screened delta is the full 8 → 10 span, not just 9 → 10:

- **eslintrc removed** → `.eslintrc.json` must become `eslint.config.js`. This is the whole of the work.
- **`/* eslint-env */` comments now error** → grep over `analytics-web-app/src` found **zero**
  occurrences. No impact.
- **`eslint:recommended` changed across both skipped majors.** Diffing the installed `@eslint/js@8.57.1`
  recommended set against `@eslint/js@10.0.1` gives **7 rules added**: `no-constant-binary-expression`,
  `no-empty-static-block`, `no-new-native-nonconstructor`, `no-unused-private-class-members` (all added
  in ESLint 9), plus `no-unassigned-vars`, `no-useless-assignment`, `preserve-caught-error` (added in
  ESLint 10) — and **4 rules removed**: `no-extra-semi`, `no-inner-declarations`,
  `no-mixed-spaces-and-tabs`, `no-new-symbol`. Of the 7 additions, a measured run against the real `src/`
  shows only **2** produce findings: `preserve-caught-error` (`src/lib/arrow-stream.ts:352`) and
  `no-useless-assignment` (`src/lib/time-range.ts:130`); the other 5 report zero. Fixing the two live ones
  is part of the commit.
- **ESLint 9 changed `no-constant-condition`'s `checkLoops` default** from `true` to
  `"allExceptWhileTrue"`. This makes the 3 existing `no-constant-condition` disables in
  `analytics-web-app/src/lib/arrow-stream.ts` stale under the target version (see the disable-directive
  inventory below).
- **Config lookup now walks up from each linted file** — irrelevant with a single root config.
- Removed deprecated `context`/`SourceCode` methods — no custom rules or plugins in this repo.

Existing inline disables reference `react-refresh/only-export-components` (28),
`react-hooks/exhaustive-deps` (22), `react-hooks/rules-of-hooks` (4), `no-constant-condition` (3),
`no-control-regex` (2), `require-yield` (1), `@typescript-eslint/no-unused-vars` (1). All of those rule
names survive in the target versions — but flat config defaults `linterOptions.reportUnusedDisableDirectives`
to `warn` (eslintrc left it off entirely), so a disable that no longer matches a live finding now surfaces
as a lint warning. A measured run of the plan's exact `eslint.config.js` against the real `src/` produced
exactly **12** "Unused eslint-disable directive" reports:
- 3x `no-constant-condition` in `lib/arrow-stream.ts` (stale from the `checkLoops` default change above)
- 4x `react-hooks/rules-of-hooks` in `__tests__/useNotebookVariables.test.tsx`
- 3x `react-hooks/exhaustive-deps` in `routes/ProcessMetricsPage.tsx`, `routes/ScreenPage.tsx`,
  `perf-analysis/PerformanceMetricsChart.tsx`
- 1x `react-refresh/only-export-components` in `cells/MapCell.tsx`
- 1x `@typescript-eslint/no-unused-vars` in `cells/VariableCell.tsx`

So disable-comment rewriting **is** needed, not skipped: Commit 2 removes these 12 stale directives.
(Alternative, not taken: set `linterOptions.reportUnusedDisableDirectives: 'off'` in `eslint.config.js`
to keep them — rejected since they are genuinely stale and removing them is cheap.)

### Flat config shape

New `analytics-web-app/eslint.config.js`, replacing `.eslintrc.json` one-for-one in *shape* (same three
extends collapsed into flat form, same two rule overrides) — not in exact findings: the bumped
`eslint:recommended` set and plugin majors add a measured 2 errors and 20 warnings where the baseline had
0 errors / 4 warnings (see Commit 2 step 5 triage); the two errors must be fixed (bucket (a)), the
warnings do not fail `yarn lint`.

```js
import js from '@eslint/js'
import tseslint from 'typescript-eslint'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import globals from 'globals'

export default tseslint.config(
  { ignores: ['dist', 'coverage', 'src/lib/datafusion-wasm'] },
  js.configs.recommended,
  tseslint.configs.recommended,
  {
    files: ['**/*.{ts,tsx}'],
    languageOptions: { ecmaVersion: 2020, globals: globals.browser },
    plugins: { 'react-hooks': reactHooks, 'react-refresh': reactRefresh },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // React Compiler rules that fire on this codebase (106 findings across 48 files, see below) —
      // turned off here; adoption is tracked in a dedicated follow-up issue (see Trade-offs).
      'react-hooks/refs': 'off',
      'react-hooks/set-state-in-effect': 'off',
      'react-hooks/static-components': 'off',
      'react-hooks/immutability': 'off',
      'react-hooks/purity': 'off',
      '@typescript-eslint/no-unused-vars': [
        'warn',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
      ],
      'react-refresh/only-export-components': ['warn', { allowConstantExport: true }],
    },
  }
)
```

**`eslint-plugin-react-hooks` v7's flat-config export is confirmed, not assumed** — `grafana/` already
depends on `eslint-plugin-react-hooks ^7.0.0`; the installed `grafana/node_modules/eslint-plugin-react-hooks`
was 7.0.1 at the time of that check, but the version targeted here is **7.1.1**. Re-verified directly
against `7.1.1`: it exposes the same `configs.recommended`, `configs['recommended-latest']`, and
`configs.flat.{recommended,recommended-latest}` shape, and `configs.recommended` is eslintrc-shaped
(`plugins: ['react-hooks']`) but carries a usable `.rules` object, so the
`...reactHooks.configs.recommended.rules` spread in the config above works as written. That `.rules`
object has **16** rules in `7.1.1`, not 17 — `component-hook-factories` (present in `7.0.1`) is gone:
`rules-of-hooks`, `exhaustive-deps`, plus 14 React-Compiler rules (`static-components`, `use-memo`,
`preserve-manual-memoization`, `incompatible-library`, `immutability`, `globals`, `refs`,
`set-state-in-effect`, `error-boundaries`, `purity`, `set-state-in-render`, `unsupported-syntax`,
`config`, `gating`). Not all default to `error`: `exhaustive-deps`, `incompatible-library`, and
`unsupported-syntax` default to `warn`; the other 13 default to `error`.

**`react-hooks` v7 rule-set growth — measured, not open-ended.** v7's `recommended` pulls in the React
Compiler rule family in addition to `rules-of-hooks`/`exhaustive-deps`. Running the plan's exact
`eslint.config.js` (ESLint 10.8.0 + `typescript-eslint@8.66.0` + `eslint-plugin-react-hooks@7.1.1`)
against the real `src/` produces **106 errors across 48 files** (50 files have at least one finding)
from 5 rules: `react-hooks/refs` 60, `react-hooks/set-state-in-effect` 33, `react-hooks/static-components`
8, `react-hooks/immutability` 4, `react-hooks/purity` 1. `rules-of-hooks` and `exhaustive-deps` report
**zero** new findings under v7 — the 22 existing `exhaustive-deps` disables are not where this flood
comes from; that rule is exactly as noisy as it is today, and stays `warn` in v7's recommended set
regardless. Baseline under ESLint 8 is 0 errors / 4 warnings, so a green `yarn lint` in Commit 2 is not
reachable by fixing the React-Compiler findings by hand — 106 errors across 48 files is out of
proportion to a dependency bump. Mitigation, decided in-plan: turn off the 5 rules that actually fire
(`refs`, `set-state-in-effect`, `static-components`, `immutability`, `purity`) in `eslint.config.js`,
keeping `rules-of-hooks`/`exhaustive-deps` at their v7 severities. This is narrower and more honest than
blanket-demoting the whole `react-hooks` plugin to `warn`, and it means the React Compiler rules are
adopted via a dedicated follow-up issue (see Trade-offs), not silently dropped. `^6` is not a fallback option regardless — its
peer range caps at `eslint@^9.0.0` and does not satisfy `eslint@^10`.

Because flat config lints only JS by default, `files: ['**/*.{ts,tsx}']` is required for the TS rules to
apply; `tseslint.configs.recommended` supplies the TS parser wiring. Verify coverage did not silently
shrink (see Testing Strategy — file-count check).

devDep churn for this commit: add `@eslint/js`, `typescript-eslint`, `globals`; bump `eslint` to `^10`,
`eslint-plugin-react-hooks` to `^7.1.1`, `eslint-plugin-react-refresh` to `^0.5`;
**remove** `@typescript-eslint/eslint-plugin`, `@typescript-eslint/parser` (superseded by the unified
`typescript-eslint` package) and `@eslint/eslintrc` (dead — see Current State). Delete `.eslintrc.json`.
Removing the split `@typescript-eslint/*` packages orphans the `@typescript-eslint/utils@7.18.0`
`packageExtensions` entry in `analytics-web-app/.yarnrc.yml` (it exists solely to patch that v7-only
package's peer range — `typescript-eslint@8.66.0`'s own `utils` declares its own `typescript` peer, which
is why the pin is version-exact). The root mirror in `/.yarnrc.yml` is **not** orphaned by this commit —
it is live for `welcome/` (a separate standalone yarn project, out of scope here — see Current State):
`welcome/package.json` is still on `@typescript-eslint/{eslint-plugin,parser} ^7.0.0`, which resolves
`@typescript-eslint/utils@7.18.0` in `welcome/yarn.lock`, and a reproduced `welcome/` install with the
root extensions removed fails to satisfy that package's peer requirements (`YN0086`; `yarn
explain peer-requirements` reports `@typescript-eslint/utils@npm:7.18.0 doesn't provide typescript to
@typescript-eslint/typescript-estree@npm:7.18.0`). So only `analytics-web-app/.yarnrc.yml`'s own copy of
the entry is orphaned by this commit and gets removed; the root file is left alone entirely.

The root `/.yarnrc.yml` is itself inherited by `analytics-web-app`: `yarn config get packageExtensions`
run there returns the root file's four entries (including this one), and `yarn config get logFilters`
run there returns the root's `[{code: YN0068, level: discard}]`. So the local mirror is already redundant
for repo-local `yarn install` runs — its only real consumer is the Docker frontend build, which copies
just `analytics-web-app/{package.json,yarn.lock,.yarnrc.yml}` (`docker/analytics-web.Dockerfile:50`,
`docker/monolith.Dockerfile:51`, `docker/all-in-one.Dockerfile:51`) and therefore has no root file to
inherit from. Removing the local entry is thus a cleanup with no functional effect on repo-local
installs; verification is (a) `yarn install` in `analytics-web-app` succeeds — already covered by the CI
gate — and optionally (b) a Docker frontend build. A check for an absent `YN0068` warning is **not** used
as a signal: because the root `logFilters` entry is inherited, `YN0068` is unconditionally discarded in
`analytics-web-app` regardless of whether the removed entry was needed, so its absence would pass
vacuously.

This commit also orphans the `js-yaml` `resolutions` pin (`"js-yaml": "^4.3.0"` in
`analytics-web-app/package.json`), the same way Commit 3 orphans `baseline-browser-mapping`: in
`analytics-web-app/yarn.lock`, `js-yaml` is reachable only through `eslint@^8.57.0` (which declares
`js-yaml: ^4.1.0`) and `@eslint/eslintrc@{^2.1.4,^3.3.1}` — all three leave the tree in this commit — and
`eslint@10.8.0`'s own dependency list has no `js-yaml`. The other pins were checked and stay live under
ESLint 10: `ajv ^6.14.0`, `minimatch ^10.2.x`, and `flatted` (via `file-entry-cache@8` →
`flat-cache@4`) all remain reachable, so only `js-yaml` needs pruning here.

### date-fns 2 → 4

All five imported functions (`format`, `setHours`, `setMinutes`, `startOfDay`, `endOfDay`) exist with
unchanged signatures in v4. The v3/v4 breaking changes are about the package shape and typing, not these
APIs: submodule/ESM path changes (this app imports only from the package root), removal of the
`sub-path`-style `date-fns/esm` entry (unused), and stricter `Date`-argument typing. v4 additionally adds
first-class time-zone support via the separate `@date-fns/tz` package, which is opt-in and not needed
here.

Net effect beyond the version string: `yarn.lock` collapses to a single `date-fns@npm:4.x` entry shared
with `react-day-picker`. `DateTimePicker.tsx` is expected to need no edit; if the stricter typing flags
anything, it will surface in `yarn type-check`.

### Tailwind 3 → 4

The two structural decisions:

**Keep `tailwind.config.ts` via the `@config` directive rather than porting the theme to CSS `@theme`.**
v4 still supports a JS/TS config file; it is simply no longer auto-detected and must be named from CSS.
The three unsupported v4 options (`corePlugins`, `safelist`, `separator`) are absent from this config, so
`@config` is a complete, lossless path. The alternative — hand-translating ~60 `hsl(var(--…))` /
`var(--…)` color entries into `@theme` declarations — is a large mechanical diff with real
transcription risk and no functional gain in this PR. Recorded as a follow-up in Trade-offs.

**Use `@tailwindcss/vite` rather than `@tailwindcss/postcss`.** The app is a Vite 8 project
(`analytics-web-app/vite.config.ts`), the Vite plugin is the path Tailwind recommends for Vite, and it
lets `postcss.config.mjs` be deleted outright. Note this is narrower than "v4 handles prefixing" —
`@tailwindcss/vite` only prefixes the stylesheet that imports Tailwind (`src/styles/globals.css`); it
does not run over component-imported CSS such as `src/components/ui/DateTimePicker.css` or
`react-day-picker/style.css`, both imported directly from `DateTimePicker.tsx`. Checked against the
installed `postcss` + `autoprefixer`: the only prefixes emitted anywhere in the app today are
`-moz-appearance: none` and `max-width: -moz-fit-content`, both in `react-day-picker/style.css`
(`DateTimePicker.css` and `globals.css` are unchanged by `autoprefixer`) — and both are moot given v4's
new Firefox 128+ floor (they matter only below Firefox 94 and 80 respectively), so dropping
`autoprefixer` costs nothing here even though it stops covering those component stylesheets too. Vitest
does not process CSS by default (no `css: true` in the `test` block), so tests are unaffected by the
pipeline swap.

`globals.css` becomes:

```css
@import "tailwindcss";
@config "../../tailwind.config.ts";
```

(`@config`'s path is relative to the CSS file: `src/styles/globals.css` → repo-relative
`analytics-web-app/tailwind.config.ts`.) The two `@layer base` blocks and both `@apply` uses stay as-is —
`@apply` still works in v4, and the "bundled-separately stylesheet needs `@reference`" caveat does not
apply because `globals.css` *is* the entry stylesheet that imports Tailwind.

#### Default-value changes, screened against this codebase

- **Default border color `gray-200` → `currentColor`**: **already neutralized.**
  `analytics-web-app/src/styles/globals.css:85-87` contains `* { @apply border-border }`, which sets
  `border-color` on every element to `hsl(var(--border))`. So the 201 bare-`border` class occurrences in
  `src` (`grep -rPo '(?<![-\w:])border(?![-\w])'`, which counts only the unprefixed, unsuffixed utility —
  it excludes both hyphenated forms like `border-2` and variant-prefixed forms like `hover:border`) are
  unaffected, and the compat shim from the upgrade guide is **not** needed. This is the single biggest
  de-risking finding for this bump.
- **Default sans-serif font stack changes.** Not in the v4 upgrade guide's "Preflight changes" list, but
  verified by compiling `tailwind.config.ts` under both versions: `tailwindcss@3.4.19` emits
  `html { font-family: ui-sans-serif, system-ui, sans-serif, "Apple Color Emoji", … }`;
  `tailwindcss@4.3.3` emits `--font-sans: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto,
  "Helvetica Neue", "Noto Sans", Arial, sans-serif, …` and `html, :host { font-family:
  var(--default-font-family, …) }`. `tailwind.config.ts` sets no `fontFamily`, so nothing pins the old
  stack — every unstyled text node in the app switches font stacks (most visibly on Linux, where
  `system-ui`/`ui-sans-serif` resolve to the desktop UI font but v4 falls through to Roboto/Noto Sans).
  Same class of silent, value-changing default as `rounded-sm` → `rounded-xs` below. Made
  value-preserving as part of Commit 3's reconciliation step (step 2): add
  `fontFamily.sans: ['ui-sans-serif', 'system-ui', 'sans-serif', '"Apple Color Emoji"', '"Segoe UI
  Emoji"', '"Segoe UI Symbol"', '"Noto Color Emoji"']` to `tailwind.config.ts` alongside the
  `borderRadius.xs` entry. Cover in Commit 3 step 6's visual pass; if the font-stack change is instead
  accepted rather than preserved, it belongs in the changelog's user-visible list alongside the
  `hover:` and browser-floor changes.
- **Default ring width `3px` → `1px`**: bare `ring` occurrences in `src` = **0**. Every usage is an
  explicit width (`ring-2` ×12, `ring-1` ×4, `ring-inset` ×4) or a color (`ring-accent-link/50` ×7,
  `ring-accent-link` ×4, `ring-ring` ×2, `ring-brand-gold` ×2, `ring-destructive`, `ring-red-400`) or an
  offset (`ring-offset-*` ×5). No width regression.
- **Default ring color `blue-500` → `currentColor`**: a `ring-1`/`ring-2` with no companion color class
  will change hue. Enumerate those sites during implementation and add an explicit ring color where the
  rendered color mattered. Low count, visual-only.
- **`hover:` now gated behind `@media (hover: hover)`.** v3 emits `.hover\:underline:hover { … }`
  unconditionally; v4 wraps it in `@media (hover: hover) { … }`, so hover-only styles stop applying on
  touch/pen-primary devices where `hover: hover` doesn't match. Verified by compiling `hover:underline`
  under `tailwindcss@4.3.3` via `@config`. `analytics-web-app/src` has 286 `hover:`
  (`grep -rPo '(?<!group-)hover:'`) and 9 `group-hover:` occurrences (295 total) — the largest count of
  any screened default change here. Accept v4's behavior:
  it is the intended modern default and this is a desktop-oriented analytics app; `@custom-variant hover
  (&:hover)` in `globals.css` is the one-line override if touch regressions show up. Cover these sites in
  Commit 3 step 6's visual pass and call this out as user-visible in the changelog bullet.
- **Browser support floor rises to Safari 16.4+ / Chrome 111+ / Firefox 128+.** `package.json` has no
  `browserslist` field and there is no `.browserslistrc` — the `browserslist ^4.28.1` and
  `baseline-browser-mapping` devDeps (plus the `baseline-browser-mapping` `resolutions` pin) exist solely
  as transitive dependencies of `autoprefixer` (confirmed via `yarn why`), not as a target declaration.
  Once Commit 3 drops `autoprefixer` (see below), both become orphaned and are removed alongside it — see
  Commit 3 step 3. Nothing else to edit here, but the browser-floor change is user-visible and belongs in
  the changelog entry.
- **Opacity modifiers on bare-`var(--…)` theme colors are dead in v3, live in v4.** v3 cannot apply an
  alpha channel to a bare `var(--x)` color reference and silently drops the utility entirely — only the
  `hsl(var(--…))`-based color families (e.g. `bg-background/50`) survive an opacity modifier today. v4
  compiles every `/opacity` modifier to `color-mix(in oklab, var(--x) N%, transparent)` regardless of
  which color form is behind it, so these currently-dead classes come to life. Verified by compiling a
  reduced copy of `tailwind.config.ts` under both `tailwindcss@3.4.19` and `tailwindcss@4.3.3` via
  `@config`. A grep over `src` for the bare-`var()` color families with an opacity modifier finds ~106
  sites, topped by `bg-theme-border/50` (19), `bg-accent-link/5` (9), `bg-accent-error/10` (9),
  `ring-accent-link/50` (7), `bg-accent-link/20` (6), `border-accent-error/30` (5), `bg-app-card/50` (4),
  `divide-theme-border/50` (3), and others. This is the same "previously-dead rule coming to life" pattern
  as the `DateTimePicker.css` case below, but two orders of magnitude larger and app-wide. Because v3
  drops these utilities entirely, today's shipped look at every one of these sites is no
  background/ring/border at all — so a value-preserving option exists alongside the two visual ones.
  Triage each site in Commit 3 step 6's visual pass into one of three outcomes: keep the opacity modifier
  (translucent is the intended look), drop the modifier (the opaque color is the intended look), or
  remove the class entirely (no background/ring/border was intended — this is the option that reproduces
  today's rendering exactly).
- **`space-*`/`divide-*` selector rewrite.** v3 emits `.space-y-2 > :not([hidden]) ~ :not([hidden]) {
  margin-top: … }`; v4 emits `:where(.space-y-2 > :not(:last-child)) { margin-block-end: … }` (verified by
  compiling `space-y-2 divide-y` under both versions). Three behavioral deltas: `[hidden]` children now
  contribute spacing, the margin moves from the following element's top to the preceding element's
  bottom, and the `:where()` wrapper drops specificity to 0 so any child-level margin utility now wins.
  A grep over `src` finds 60 `space-[xy]-*` and 6 `divide-*` occurrences (66 total). Cover these layouts
  in Commit 3 step 6's visual pass.
- **Buttons lose default `cursor: pointer`.** Installed `tailwindcss@3.4.19`'s
  `src/css/preflight.css:343-346` sets `button, [role="button"] { cursor: pointer }`; that rule does not
  exist in `tailwindcss@4.3.3`'s preflight. The app has 132 `<button` occurrences total, 118 of them
  outside `__tests__` (the ones that ship), plus 6 `role="button"` elements against only 58
  `cursor-pointer` occurrences repo-wide (and some of those are on non-buttons),
  so most buttons would silently lose their pointer cursor. Add the v4 upgrade guide's compat rule to
  `globals.css`'s `@layer base`: `button:not(:disabled), [role="button"]:not(:disabled) { cursor: pointer
  }`.
- **Input placeholder color changes.** Installed `tailwindcss@3.4.19`'s `src/css/preflight.css:333-337`
  sets `input::placeholder, textarea::placeholder { color: theme('colors.gray.400') }` (`#9ca3af`). v4's
  `preflight.css` (lines 287-301) does not fall back to the browser default — it sets `::placeholder {
  opacity: 1 }` and, under an `@supports` guard, `::placeholder { color: color-mix(in oklab, currentcolor
  50%, transparent) }`, i.e. a 50%-transparent current text color. So this is a different placeholder
  color, not an absent one. 43 `placeholder=` attributes exist in `src` against only 7 explicit
  `placeholder-*` color classes. Add an explicit `input::placeholder, textarea::placeholder` color rule to
  `globals.css`'s `@layer base` to preserve the current look.

#### Utility renames required

Counts are `grep` occurrences over `analytics-web-app/src`:

| v3 | v4 | count |
|---|---|---|
| `outline-none` | `outline-hidden` | 83 |
| `rounded` (bare) | `rounded-sm` | 126 |
| `rounded-sm` | `rounded-xs` | 6 |
| `flex-shrink-*` | `shrink-*` | 22 |
| `backdrop-blur-sm` | `backdrop-blur-xs` | 1 |
| `shadow-sm` | `shadow-xs` | 1 |

The 126 bare-`rounded` occurrences include three variant-prefixed forms — `prose-code:rounded` in
`src/lib/screen-renderers/table-utils.tsx:420`, `src/lib/screen-renderers/cells/MarkdownCell.tsx:26`,
and `src/components/map/EventDetailContent.tsx:82` — which also need the rename.

There is no bare-`blur` or standalone-`blur-sm` row: all matches of bare `blur` in `src` are JavaScript
DOM uses (`e.currentTarget.blur()`, `addEventListener('blur', …)`, `fireEvent.blur(input)`) or comment
prose, not Tailwind classes — a hand-edit must not touch them. The only `blur`-scale utility class
present is `backdrop-blur-sm`, covered by its own row above.

There is likewise no bare-`shadow` row: the only match of bare `shadow` in `src` is comment prose in
`src/components/map/MapCell.tsx:141` ("doesn't shadow the default"), not a Tailwind class — it must not
be rewritten. The only `shadow`-scale utility class present is `shadow-sm`, covered by its own row above.

**Ordering hazard**: the `rounded` scale is *shifted*, not remapped — a naive find-and-replace
that rewrites `rounded` → `rounded-sm` before `rounded-sm` → `rounded-xs` will double-shift the 6
original `rounded-sm` sites into `rounded-xs`-then-nothing. Run `yarn dlx @tailwindcss/upgrade` (the
official codemod, Node 20+) rather than hand-editing; it applies the renames in the correct order and is
the sanctioned migration path. Review its diff before committing, and reject any part of it that tries
to convert `tailwind.config.ts` into `@theme` (that is the deliberate `@config` decision above).

**`rounded-sm` → `rounded-xs` is not value-preserving here.** `tailwind.config.ts` overrides
`borderRadius.sm` to `calc(var(--radius) - 4px)` (with `--radius: 0.5rem` in `globals.css`), so v3
`rounded-sm` renders 4px today. The config only overrides `lg`/`md`/`sm`, so nothing defines
`--radius-xs` under `@config`, and v4's built-in default `--radius-xs` is `0.125rem` (2px) — the rename
would silently halve the radius at the 6 affected sites. Add a `borderRadius.xs:
'calc(var(--radius) - 4px)'` entry to `tailwind.config.ts` as part of Commit 3's reconciliation step so
the rename stays value-preserving (alternative: leave those 6 sites at `rounded-sm`). This makes
`tailwind.config.ts` a modified file in this commit, not an unchanged one (see Files to Modify).

No `bg-opacity-*` / `text-opacity-*` / `border-opacity-*` occurrences exist (count 0), so the
opacity-modifier removals do not apply.

#### `tailwind-merge` must move to v3 in the same commit

Not mentioned in the issue, but load-bearing: `analytics-web-app/src/lib/utils.ts:2` wraps every
`className` composition in `twMerge(clsx(inputs))`, and `tailwind-merge` v2 encodes Tailwind **v3**'s
class taxonomy. Left at v2 against Tailwind v4 it would mis-group the renamed utilities — e.g. failing to
recognize `shadow-xs`/`rounded-xs`/`outline-hidden` as members of the groups they now belong to — so
conflicting classes would stop cancelling and last-wins overrides would silently break. `tailwind-merge`
v3 is the Tailwind-v4-aware line (target `3.6.0`). This is why the Tailwind work cannot be split
finer than one commit.

#### Dead `--color-*` references repoint (verified dead, not a speculative check)

`analytics-web-app/src/components/ui/DateTimePicker.css` references `var(--color-accent-link)`,
`var(--color-app-card)`, `var(--color-theme-text-primary)`, `var(--color-theme-text-muted)`,
`var(--color-theme-border)`, `var(--color-theme-text-secondary)` — i.e. **v4-shaped `--color-*` theme
variable names**. Tailwind v3 with a JS config emits no `--color-*` variables, and `globals.css` defines
the raw names (`--accent-link`, `--card-bg`, `--text-primary`, `--text-muted`, `--border-color`,
`--text-secondary`), so these six references resolve to nothing today and those react-day-picker styles
are silently inert.

`DateTimePicker.css` has eight references across those six distinct names (`--color-theme-text-primary`
and `--color-theme-border` each appear twice, at lines 3, 4, 8, 12, 21, 25, 30, 35).

A repo-wide grep for `--color-` in `analytics-web-app/src` finds five more occurrences of the same dead
pattern, all `className="accent-[var(--color-accent-link)]"`: `src/routes/ExportScreensPage.tsx:182,209`,
`src/routes/ImportScreensPage.tsx:289,318`, and `src/routes/DataSourcesPage.tsx:213`. Same cause, same
fix — thirteen dead `--color-*` references total across four files.

They stay inert under v4 too — verified, not speculative: compiling the repo's actual
`tailwind.config.ts` + `globals.css` (`@import "tailwindcss"` + `@config "../../tailwind.config.ts"`)
under `tailwindcss@4.3.3` produces no definition for any of the six `--color-*` names above (v4 emits
`--color-*` only for the built-in palette entries actually in use elsewhere in the app — e.g.
`--color-red-500`, `--color-gray-400`, `--color-blue-500` — 25 such properties are emitted, just none of
these six). On the `@config` path v4 inlines legacy-JS-config colors directly instead (e.g.
`.bg-accent-link\/20 { background-color: var(--accent-link) }`). So this is an unconditional edit, not a
browser check: repoint all thirteen references at the raw names `globals.css` actually defines —
`--accent-link` for the five route-file sites and for `DateTimePicker.css`'s own `--color-accent-link`
reference, plus `--card-bg`, `--text-primary`, `--text-muted`, `--border-color`, `--text-secondary` for
`DateTimePicker.css`'s other five — as part of Commit 3. (The eventual `@theme` migration — the deferred
follow-up in Trade-offs — is what would make the `--color-*` names valid instead.)

## Implementation Steps

Three implementation commits plus a changelog commit, on one branch. Run
`python3 build/analytics_web_ci.py` (type-check → lint → test → build, exactly what
`.github/workflows/analytics-web-app.yml` runs) to green **before each of the three implementation
commits**, so the branch bisects cleanly. The changelog commit (see "Final" below) is docs-only and the
CI gate does not apply to it.

### Commit 1 — `date-fns` 2 → 4

1. `analytics-web-app/package.json`: `"date-fns": "^2.30.0"` → `"^4.4.0"`.
2. `yarn install` in `analytics-web-app/`. Confirm `yarn.lock` now holds a single `date-fns@npm:4.x`
   entry (was two: `^2.30.0` → `2.30.0` and `^4.1.0` → `4.1.0`) — `grep -n '^"date-fns@' yarn.lock`
   (the `@` after `date-fns` excludes the unrelated `date-fns-jalali` entry that a bare `^"date-fns` grep
   also matches).
3. `yarn type-check` — catches any stricter-typing fallout at
   `src/components/ui/DateTimePicker.tsx:3`. No edit expected.
4. Full `python3 build/analytics_web_ci.py`, then commit.

### Commit 2 — `eslint` 8 → 10 + flat config

0. **Before touching `package.json`**, with ESLint 8 still installed, capture the pre-migration baseline
   against the still-live `.eslintrc.json`: `yarn eslint . -f json | jq 'length'` (or the file list). On
   the current tree this is **253 files (131 `.tsx`, 122 `.ts`)**, and **zero** `.js`/`.mjs` files — record
   the `.ts`/`.tsx` split for the step 6 comparison, since ESLint 10 removes eslintrc support entirely and
   no eslintrc-based run is possible once step 3 installs it. Flat config lints `**/*.js`/`.cjs`/`.mjs` by
   default in addition to whatever `files` names, so the post-migration total is **not** expected to stay
   at 253 — it additionally picks up the new `eslint.config.js` and, until Commit 3 deletes it,
   `postcss.config.mjs` (255 files here; 254 once Commit 3 removes `postcss.config.mjs`). Comparing raw
   totals would flag a correct migration as a regression; see Testing Strategy.
1. `analytics-web-app/package.json` devDeps: bump `eslint` → `^10.8.0`,
   `eslint-plugin-react-refresh` → `^0.5.3`, `eslint-plugin-react-hooks` → `^7.1.1`; add
   `@eslint/js ^10.0.1`, `typescript-eslint ^8.66.0`, `globals ^17.9.0`; remove
   `@typescript-eslint/eslint-plugin`, `@typescript-eslint/parser`, `@eslint/eslintrc`. Also remove the
   now-orphaned `"js-yaml": "^4.3.0"` entry from `resolutions` (see Design) — mirroring how Commit 3 prunes
   `baseline-browser-mapping`; `ajv`, `minimatch`, and `flatted` stay live under ESLint 10 and keep their
   `resolutions` pins.
2. Remove the now-orphaned `@typescript-eslint/utils@7.18.0` `packageExtensions` entry from
   `analytics-web-app/.yarnrc.yml` only (see Design). Leave the root `/.yarnrc.yml` untouched — its copy
   of the entry is still live for `welcome/`.
3. `yarn install` in `analytics-web-app` — this is the verification. `packageExtensions` changes leave no
   trace in `yarn.lock`, and the removed entry is redundant for repo-local installs since it is already
   inherited from the root `.yarnrc.yml` (see Design), so a passing install is the right signal here, not
   a lockfile diff. Optionally also build the Docker frontend image, since that build path copies only
   `analytics-web-app/{package.json,yarn.lock,.yarnrc.yml}` and has no root file to inherit from. Do
   **not** check for an absent `YN0068` warning as a verification step: the root `.yarnrc.yml`'s
   `logFilters: [{code: YN0068, level: discard}]` is inherited by `analytics-web-app`, so `YN0068` is
   unconditionally suppressed there and its absence proves nothing.
4. Create `analytics-web-app/eslint.config.js` per the Design shape (the
   `...reactHooks.configs.recommended.rules` spread is confirmed against `grafana/node_modules`, not an
   install-time unknown — see Design); `git rm analytics-web-app/.eslintrc.json`.
5. `yarn lint`. Triage findings in four buckets: **(a)** of the seven newly-recommended
   `eslint:recommended` rules added across the skipped 9 and 10 majors, the two that actually fire —
   `preserve-caught-error` (`src/lib/arrow-stream.ts:352`) and `no-useless-assignment`
   (`src/lib/time-range.ts:130`); the other five (`no-constant-binary-expression`,
   `no-empty-static-block`, `no-new-native-nonconstructor`, `no-unused-private-class-members`,
   `no-unassigned-vars`) report zero findings here — fix the two live ones; **(b)** the 5
   React-Compiler `react-hooks` v7 rules (`refs`, `set-state-in-effect`, `static-components`,
   `immutability`, `purity`) that would otherwise fire — already turned off in step 4's
   `eslint.config.js` per the Design's decision, so this run reports none of the 106 findings they'd
   otherwise produce; noted here as the rationale for keeping them off rather than hand-fixing, adopting
   them as a separate follow-up instead; **(c)** the 12 stale
   `eslint-disable` directives flagged by flat config's `reportUnusedDisableDirectives: 'warn'` default
   (see Design) — remove them; **(d)** four new rule-warnings introduced by the bumped plugins: two
   `@typescript-eslint/no-unused-vars` (from `@typescript-eslint` 7 → 8) at
   `src/lib/__tests__/auth.test.tsx:422` and `:492` — fix by prefixing the unused identifiers with `_`,
   which the config's own `varsIgnorePattern: '^_'` (see the flat config shape in Design) already exempts;
   and two `react-refresh/only-export-components` (from `eslint-plugin-react-refresh` 0.4.26 → 0.5.3) at
   `src/components/ErrorBoundary.tsx:74` and `src/lib/screen-type-utils.tsx:14`, left as-is, consistent
   with the 4 pre-existing warnings of the same rule already accepted in the baseline (`yarn lint` has no
   `--max-warnings` gate). Note the bucket (b) decision in the commit message. Raw total measured
   immediately after the config swap (before fixes): **20 warnings** (4 pre-existing
   `react-refresh/only-export-components` in `src/components/FolderTree.tsx` + the 12 stale-disable-directive
   warnings from bucket (c) + these 4). Accepted steady-state warning count once buckets (a)-(d) are
   applied: **6** — the 4 pre-existing `react-refresh/only-export-components` warnings plus the 2 new ones
   left in bucket (d).
6. Confirm the 253 `.ts`/`.tsx` files are all still linted, plus the two newly-in-scope non-TS files (see
   Testing Strategy) — 255 files total at this point, not 253.
7. Full `python3 build/analytics_web_ci.py`, then commit.

### Commit 3 — `tailwindcss` 3 → 4 (+ `tailwind-merge` 3)

1. From `analytics-web-app/`, run `yarn dlx @tailwindcss/upgrade`. Do not accept the run blindly —
   `git diff` it in full. Note: the codemod's bundle also installs `@tailwindcss/postcss` as a
   devDependency and rewrites `postcss.config.mjs`'s `tailwindcss:` entry to `'@tailwindcss/postcss':`;
   both are superseded by step 3's switch to `@tailwindcss/vite` and must be removed there.
2. Reconcile the codemod's output with the deliberate decisions here:
   - `src/styles/globals.css` must end up as `@import "tailwindcss";` +
     `@config "../../tailwind.config.ts";`, with both `@layer base` blocks and both `@apply` uses
     intact. **Revert** any attempt to inline `tailwind.config.ts` into `@theme`.
   - Confirm it applied all six renames from the Design table, in the correct shifted order, and that
     `tailwind.config.ts` gained the `borderRadius.xs` entry (or the 6 `rounded-sm` sites were left
     alone) so `rounded-xs` stays value-preserving.
   - Add `fontFamily.sans: ['ui-sans-serif', 'system-ui', 'sans-serif', '"Apple Color Emoji"', '"Segoe UI
     Emoji"', '"Segoe UI Symbol"', '"Noto Color Emoji"']` to `tailwind.config.ts` so the base font stack
     stays value-preserving (see Design's font-stack default-value change).
   - Repoint the thirteen dead `--color-*` references at the raw names `globals.css` defines — eight in
     `src/components/ui/DateTimePicker.css`, plus the `accent-[var(--color-accent-link)]` sites at
     `src/routes/ExportScreensPage.tsx:182,209`, `src/routes/ImportScreensPage.tsx:289,318`, and
     `src/routes/DataSourcesPage.tsx:213` (see Design). The codemod does not touch these; this is a
     manual edit.
3. Switch the build pipeline to the Vite plugin: add `@tailwindcss/vite ^4.3.3` to devDeps, add
   `tailwindcss()` to the `plugins` array in `analytics-web-app/vite.config.ts`, delete
   `analytics-web-app/postcss.config.mjs`, and remove the `autoprefixer` devDep, plus the
   `@tailwindcss/postcss` devDep that step 1's codemod added (superseded by the Vite plugin).
   `autoprefixer` is the only dependent of `browserslist` and `baseline-browser-mapping` (`yarn why`
   confirms both resolve through it alone; `tasks/completed/vitest_migration_plan.md` records this same
   pair being kept specifically "for `autoprefixer`"), so remove the `browserslist` and
   `baseline-browser-mapping` devDeps and the `baseline-browser-mapping` entry in `resolutions` too. Keep
   the `postcss` devDep and the `postcss` `resolutions` pin (both are security-pin machinery for
   transitive users, independent of Tailwind).
4. `analytics-web-app/package.json`: `"tailwindcss": "^3.3.0"` → `"^4.3.3"`,
   `"tailwind-merge": "^2.0.0"` → `"^3.6.0"`. Leave `@tailwindcss/typography ^0.5.19` alone — `0.5.19`
   (already locked in `yarn.lock` and unchanged by a plain `yarn install`) declares a v4-compatible peer
   range (`>=3.0.0 || insiders || >=4.0.0-alpha.20 || >=4.0.0-beta.1`), which accepts `tailwindcss@4.3.3`,
   so it needs no change.
5. `yarn install`, then `yarn build`, and diff the emitted CSS bundle size/shape for anything alarming.
6. Manual visual pass against a running app, using the documented dev path
   (`analytics-web-app/README.md:36`): `python3 local_test_env/ai_scripts/start_services.py` (split
   mode, not `--monolith` — the monolith's web role binds `MICROMEGAS_PORT` (default 3000), the same
   port Vite's dev server defaults to, and `yarn dev`'s proxy target/base-path env vars are only set by
   the script below), then `python3 analytics-web-app/start_analytics_web.py`, which sets
   `MICROMEGAS_BASE_PATH` (default `/mmlocal`), starts `analytics-web-srv` on 8000, and runs Vite on
   3000. Check: whether the base sans-serif font changed app-wide (see the font-stack default-value
   change in Design — reconcile in step 2 above if so), the `ring-1`/`ring-2`-without-a-color sites, that
   buttons/`role="button"` elements still show a pointer cursor, that input/textarea placeholders still
   render in the expected muted color, the ~106 now-live `/opacity`-modified `var(--…)` color sites (per
   site: keep the modifier, drop it for an opaque look, or remove the class entirely to match today's
   no-op rendering — see Design), the 66 `space-*`/`divide-*` layouts affected by the selector rewrite,
   and the 295 `hover:`/`group-hover:` sites now gated behind `@media (hover: hover)`.
7. Full `python3 build/analytics_web_ci.py`, then commit.

### Commit 4 (Final) — changelog

A fourth commit, docs-only, on the same branch — the per-commit CI gate above does not apply to it.
Add four bullets under `## Unreleased` → `**Build:**` in `CHANGELOG.md` (create the subsection — it does
not yet exist under `## Unreleased`), matching the existing dependency-bump entry style used elsewhere in
the changelog (e.g. the `react-router-dom` → `react-router` major, the Jest→Vitest migration, the
`postcss`/`tar`/`brace-expansion` bumps — all filed under `**Build:**`, not `**Web App:**`, which is
reserved for user-facing features and fixes), referencing `(#1255)`: one each for the `date-fns` bump,
the `eslint` 8 → 10 + flat-config migration, and the `tailwindcss` 3 → 4 bump (including the
`tailwind-merge` v3 bump as part of the same change), plus a fourth bullet recording the documented hold
on `grafana/`'s React 18 with its reason (`@grafana/ui@12.4.6` peers `react: ^18.0.0`). The Tailwind
bullet must call out as user-visible: the raised browser floor (Safari 16.4+ / Chrome 111+ / Firefox
128+), the `hover:` variant now requiring true hover support (`@media (hover: hover)`), the ~106
previously-inert `/opacity`-modified bare-`var(--…)` color sites that now render (translucent or opaque,
per the Commit 3 step 6 triage) instead of not at all, and the `space-*`/`divide-*` selector rewrite
(margin moves from the following element's top to the preceding element's bottom; `[hidden]` children now
contribute spacing).

## Files to Modify

**Commit 1**
- `analytics-web-app/package.json` — `date-fns` range
- `analytics-web-app/yarn.lock` — dedupe to one `date-fns` entry

**Commit 2**
- `analytics-web-app/package.json` — eslint devDep set
- `analytics-web-app/yarn.lock`
- `analytics-web-app/.yarnrc.yml` — remove the orphaned `@typescript-eslint/utils@7.18.0`
  `packageExtensions` entry (root `/.yarnrc.yml` is untouched — its copy is still live for `welcome/`,
  see Design)
- `analytics-web-app/eslint.config.js` — **new**
- `analytics-web-app/.eslintrc.json` — **deleted**
- `analytics-web-app/src/lib/arrow-stream.ts` — 1 fix (`preserve-caught-error`) + 3 stale
  `no-constant-condition` disable-directive removals
- `analytics-web-app/src/lib/time-range.ts` — 1 fix (`no-useless-assignment`)
- `analytics-web-app/src/lib/__tests__/auth.test.tsx` — 2 fixes (`@typescript-eslint/no-unused-vars` at
  `:422`, `:492`)
- `analytics-web-app/src/lib/screen-renderers/__tests__/useNotebookVariables.test.tsx` — 4 stale
  `react-hooks/rules-of-hooks` disable-directive removals
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx` — 1 stale
  `react-refresh/only-export-components` disable-directive removal
- `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx` — 1 stale
  `@typescript-eslint/no-unused-vars` disable-directive removal
- `analytics-web-app/src/routes/ProcessMetricsPage.tsx` — 1 stale `react-hooks/exhaustive-deps`
  disable-directive removal
- `analytics-web-app/src/routes/ScreenPage.tsx` — 1 stale `react-hooks/exhaustive-deps`
  disable-directive removal
- `analytics-web-app/src/routes/perf-analysis/PerformanceMetricsChart.tsx` — 1 stale
  `react-hooks/exhaustive-deps` disable-directive removal

**Commit 3**
- `analytics-web-app/package.json` — `tailwindcss`, `tailwind-merge`, `@tailwindcss/vite`; drop
  `autoprefixer`, `@tailwindcss/postcss` (added by step 1's codemod, superseded by `@tailwindcss/vite`),
  `browserslist`, `baseline-browser-mapping`, and the `baseline-browser-mapping` `resolutions` entry
- `analytics-web-app/yarn.lock`
- `analytics-web-app/src/styles/globals.css` — `@import` + `@config`
- `analytics-web-app/vite.config.ts` — add `tailwindcss()` plugin
- `analytics-web-app/postcss.config.mjs` — **deleted**
- `analytics-web-app/src/**/*.tsx` — ~239 utility-class renames
- `analytics-web-app/src/components/ui/DateTimePicker.css` — repoint the eight `--color-*` references
  (six distinct names) to the raw variable names `globals.css` defines (verified dead under `@config`,
  not conditional)
- `analytics-web-app/src/routes/ExportScreensPage.tsx` — repoint the two
  `accent-[var(--color-accent-link)]` sites to `accent-[var(--accent-link)]`
- `analytics-web-app/src/routes/ImportScreensPage.tsx` — repoint the two
  `accent-[var(--color-accent-link)]` sites to `accent-[var(--accent-link)]`
- `analytics-web-app/src/routes/DataSourcesPage.tsx` — repoint the one
  `accent-[var(--color-accent-link)]` site to `accent-[var(--accent-link)]`
- `analytics-web-app/tailwind.config.ts` — add `borderRadius.xs` entry so `rounded-sm` → `rounded-xs`
  stays value-preserving, and a `fontFamily.sans` entry so the base font stack stays value-preserving
  (kept, still loaded via `@config`)

**Final**
- `CHANGELOG.md`

Not modified: anything under `grafana/` (see the documented hold).

## Trade-offs

- **One PR with three atomic commits, vs. the issue's "do as separate PRs" — settled: one PR.** The
  issue asked for separate PRs because each bump is independently breaking. Three commits on one branch,
  each independently CI-green, preserves the property that actually matters — bisectability and a clean
  revert per bump — while keeping one review thread for what is one coherent maintenance task in one
  package. This is a deliberate deviation from the issue text; it was raised with the repo owner, who
  confirmed one PR. Not open for re-litigation during review.
- **`@config` now, `@theme` later.** Keeping `tailwind.config.ts` is the low-risk path and is fully
  supported in v4. The cost is staying on a compatibility shim and keeping the theme split across a TS
  file and CSS variables. Porting ~60 color entries to `@theme` is a mechanical but transcription-risky
  diff worth its own change, once v4 is in and visually verified. Worth a follow-up issue.
- **`@tailwindcss/vite` over `@tailwindcss/postcss`.** The Vite plugin is faster and lets
  `postcss.config.mjs` and `autoprefixer` go away entirely. The cost is that the CSS pipeline is now
  declared in `vite.config.ts` rather than a standalone PostCSS config — slightly less portable if the
  app ever moves off Vite, which is not on the horizon.
- **ESLint 10 over 9.** Costs a version skew with `grafana/`'s 9. Buys not repeating the same mandatory
  flat-config migration one major later. The skew is free: the two packages lint independently, and
  `grafana/`'s config is `@grafana/eslint-config`-owned.
- **Unified `typescript-eslint` over the split `@typescript-eslint/*` pair.** One devDep instead of two,
  and `tseslint.config()` gives typed flat-config composition. Slight cost: diverges from `grafana/`,
  which still lists the split packages.
- **Dropping `@eslint/eslintrc` rather than adopting `FlatCompat`.** Nothing imports it, and a
  hand-written flat config for three extends + two rules is short and clearer than a compat shim.
- **Turning off the 5 React-Compiler `react-hooks` v7 rules is deferred, not dropped.** `refs` (60),
  `set-state-in-effect` (33), `static-components` (8), `immutability` (4), and `purity` (1) — 106 findings
  across 48 files — are turned off in `eslint.config.js` rather than hand-fixed inline (see Design). This
  is the plan's largest deferral, so it gets the same treatment as the other three: a follow-up issue will
  be filed to adopt the React-Compiler rule family, scoped to that 106-finding/5-rule breakdown, rather
  than leaving it as an untracked, indefinitely-off rule set.
- **`.github/dependabot.yml` and the rest of the `resolutions` block are left alone.** Both are noted in
  #1255 but neither is a version bump; automating dependency updates is a policy change with its own blast
  radius (PR volume, CI cost). Commit 2 prunes the `js-yaml` entry (orphaned by the ESLint bump, see
  Design) and Commit 3 prunes `baseline-browser-mapping` (orphaned by dropping `autoprefixer`), but the
  remaining entries (`ajv`, `minimatch`, `flatted`, `postcss`, `brace-expansion`, and others) stay live and
  are out of scope — with one exception already known and pre-existing: `rollup` (`^4.59.0`) is dead
  today (`grep -c rollup analytics-web-app/yarn.lock` is 0, since Vite 8 uses Rolldown instead), a fact
  already recorded as pre-existing and out of scope in `tasks/completed/vitest_migration_plan.md:188`, so
  it is left alone here as a known-inert legacy pin rather than implicitly claimed live. A full audit of
  the rest would require re-checking their advisories, which deserves its own issue rather than riding
  along here.
- **`welcome/` is left on ESLint 8 / eslintrc / Tailwind 3.** It is out of scope per #1255's own scope
  statement (`analytics-web-app` and `grafana` only), but it is a near-identical toolchain to the one
  retired here, so it will need the same migration eventually. Worth its own issue rather than riding
  along here.

## Documentation

No documentation changes required. `analytics-web-app/README.md:92` (`yarn lint` → "Run ESLint") and
`analytics-web-app/CLAUDE.md:8` (`yarn lint` REQUIRED before commit) both reference the script name,
which is unchanged; neither names the config file, the ESLint version, or Tailwind. A grep of
`CONTRIBUTING.md` and `mkdocs/docs/contributing.md` for `eslintrc`/`tailwind` found nothing.
`CHANGELOG.md` is the only prose artifact to update.

## Testing Strategy

- **Per-commit gate**: `python3 build/analytics_web_ci.py` from the repo root, which runs
  `yarn install` → `yarn type-check` → `yarn lint` → `yarn test` (vitest) → `yarn build`. This is
  byte-for-byte what `.github/workflows/analytics-web-app.yml` invokes, so a local green means a green
  PR check.
- **Commit 1 specific**: assert the lockfile dedupe (`grep -n '^"date-fns@' analytics-web-app/yarn.lock`
  should show one entry, not two; the `@` excludes the unrelated `date-fns-jalali` entry).
- **Commit 2 specific — lint coverage must not silently shrink.** Flat config lints `**/*.js`/`.cjs`/`.mjs`
  by default *plus* whatever extensions `files` adds, so a mis-scoped config can pass `yarn lint` by
  checking almost nothing — but it also means the raw file *total* is the wrong thing to diff, since flat
  config newly picks up `eslint.config.js` itself and (until Commit 3 deletes it) `postcss.config.mjs`.
  Compare the **`.ts`/`.tsx` subset** of the file list from `yarn eslint . -f json` against the
  pre-migration baseline captured in Commit 2 step 0: all 253 `.ts`/`.tsx` files (131 `.tsx`, 122 `.ts`)
  must still be present. The raw total is expected to be **255** in Commit 2 (253 + `eslint.config.js` +
  `postcss.config.mjs`) and **254** after Commit 3 removes `postcss.config.mjs` — not equal to 253.
- **Commit 3 specific**: `yarn build` must succeed and emit CSS; then the manual visual pass, run against
  the split-mode services + `start_analytics_web.py` dev path (see Implementation step 6 — not
  `--monolith` + `yarn dev`, which don't work together, per step 6's note), covering its full checklist
  (not restated here to avoid the two copies drifting): the base sans-serif font, the colorless
  `ring-1`/`ring-2` sites, the button/`role="button"` cursors, the input/textarea
  placeholders, the ~106 now-live `/opacity`-modified sites, the 66 `space-*`/`divide-*` layouts, the 295
  `hover:`/`group-hover:` sites, and the DateTimePicker calendar plus the checkbox accent colors on the
  Export/Import/DataSources screens (now that the thirteen `--color-*` references are repointed). Spot-check a few high-traffic screens for the renamed `rounded`/`outline-none` utilities
  rendering as before.
- **Grafana plugin**: untouched by this change's `analytics-web-app/` edits, but `grafana/`'s own CI
  (`.github/workflows/grafana-plugin.yml`) runs on a path filter (`grafana/**`, `typescript/**`,
  `package.json`, `yarn.lock`, the workflow file) that this branch does not trip. The root `/.yarnrc.yml`
  is not touched by this plan (see Design), so there is nothing there to verify. The issue's acceptance
  criterion "`yarn lint`/`type-check`/`build`/`test` pass for both packages" is satisfied for `grafana/`
  by its unchanged, already-passing state.

## Risks

- **`eslint-plugin-react-hooks` v7 rule-set expansion is measured, not open-ended.** Its `recommended`
  set fires 106 errors across 48 files from 5 React-Compiler rules (`refs`, `set-state-in-effect`,
  `static-components`, `immutability`, `purity` — see Design). The plan turns those 5 off in
  `eslint.config.js` and keeps `rules-of-hooks`/`exhaustive-deps` at their v7 severities, staying on
  `^7.1.1` (`^6` is not a fallback option, its peer range caps at `eslint@^9.0.0`), deferring the
  React-Compiler rules to a dedicated follow-up issue (see Trade-offs) rather than hand-fixing them here.
- **The Tailwind codemod's output needs real review**, not just a passing build — the renames are
  visual, and `yarn build` succeeding proves nothing about whether a `rounded` became the right
  `rounded-sm`. The manual pass in step 6 is load-bearing, not ceremonial.
- Everything else is bounded: `date-fns` is one import site against a version already in the tree, and
  the two scariest Tailwind defaults (border color, ring width) were both shown inapplicable to this
  codebase by direct inspection.

## Open Questions

None. Two decisions are deliberately made in-plan and recorded so review can override cheaply:
**ESLint 10 rather than 9** (Trade-offs), and **`@config` rather than porting the theme to `@theme`**
(Trade-offs). A third — **one PR rather than the issue's "separate PRs"** — was raised with the repo
owner and confirmed as one PR (Trade-offs); it is settled, not open. The `react-hooks` v7 noise level is
now measured (106 errors from 5 rules across 48 files, see Design), not unknown — the plan turns those 5
rules off rather than leaving it as an open question.
