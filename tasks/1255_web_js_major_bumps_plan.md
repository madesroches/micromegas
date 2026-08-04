# Bump Outdated JS Majors in `analytics-web-app` Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1255

## Overview

Issue #1255 lists five outdated JS dependency majors across `analytics-web-app/` and `grafana/`, plus a
migration off the deprecated `@grafana/experimental`. Two of the six items have since been resolved by
unrelated work, and one should be deliberately held. This plan closes the remaining three — `date-fns`
2 → 4, `eslint` 8 → 10 (which forces the flat-config migration), and `tailwindcss` 3 → 4 — all confined
to `analytics-web-app/`, and records the documented reason for holding the fourth.

The three bumps are independent of each other and land as three atomic commits, each independently green
under `python3 build/analytics_web_ci.py`.

## Current State

### Already resolved since the issue was filed

| Issue item | Resolution |
|---|---|
| `react-router-dom ^6.30.4` → 7 | Done, and further: migrated to `react-router ^8.3.0` in #1351 (`f4201fd0a`). `react-router-dom` is gone from `analytics-web-app/package.json`. |
| Migrate off deprecated `@grafana/experimental` | Done in #1354 (`82bee5cc6`) — now `@grafana/plugin-ui ^0.16.1`. Verified zero remaining references in `grafana/src`, `grafana/package.json`, `grafana/yarn.lock`. |
| Grafana SDK pinned to 11.6.7 | Done in #1354 — `@grafana/data`/`runtime`/`ui`/`e2e-selectors` are all `12.4.6`. |

`grafana/` is also already on `eslint ^9` + `@typescript-eslint ^8` (via `@grafana/eslint-config ^9`), so the
ESLint 8 holdout is now `analytics-web-app/` alone.

### Remaining, in `analytics-web-app/package.json`

- **`eslint ^8.57.0`** (resolves `8.57.1`). ESLint 8 is end-of-life. Config is still legacy eslintrc:
  `analytics-web-app/.eslintrc.json`, extending `eslint:recommended`,
  `plugin:@typescript-eslint/recommended`, `plugin:react-hooks/recommended`, with two rule overrides
  (`@typescript-eslint/no-unused-vars` and `react-refresh/only-export-components`, both `warn`) and
  `ignorePatterns: ["dist", "coverage", "src/lib/datafusion-wasm"]`.
  Companion devDeps: `@typescript-eslint/{eslint-plugin,parser} ^7.0.0`, `eslint-plugin-react-hooks ^4.6.0`
  (resolves `4.6.2`), `eslint-plugin-react-refresh ^0.4.5`, and `@eslint/eslintrc ^3.3.1` — which a
  repo-wide grep shows is **referenced by nothing** (ESLint 8 bundles its own `@eslint/eslintrc@2.1.4`
  internally), i.e. a dead devDep.
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
would both violate the peer range and risk two-React-copies breakage at runtime. `yarn why react` in
`grafana/` resolves `react@npm:18.3.1 (via npm:^18.0.0)`. This is not a bump to defer-and-revisit; it is
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

### ESLint 10 breaking changes that apply here

From the ESLint 10 migration guide, screened against this codebase:

- **eslintrc removed** → `.eslintrc.json` must become `eslint.config.js`. This is the whole of the work.
- **`/* eslint-env */` comments now error** → grep over `analytics-web-app/src` found **zero**
  occurrences. No impact.
- **Three rules added to `eslint:recommended`**: `no-unassigned-vars`, `no-useless-assignment`,
  `preserve-caught-error`. These may produce new findings; fixing them is part of the commit.
- **Config lookup now walks up from each linted file** — irrelevant with a single root config.
- Removed deprecated `context`/`SourceCode` methods — no custom rules or plugins in this repo.

Existing inline disables reference `react-refresh/only-export-components` (28),
`react-hooks/exhaustive-deps` (22), `react-hooks/rules-of-hooks` (4), `no-constant-condition` (3),
`no-control-regex` (2), `require-yield` (1), `@typescript-eslint/no-unused-vars` (1). All of those rule
names survive in the target versions, so no disable-comment rewriting is expected.

### Flat config shape

New `analytics-web-app/eslint.config.js`, replacing `.eslintrc.json` one-for-one in behavior:

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
depends on `eslint-plugin-react-hooks ^7.0.0`, and the installed
`grafana/node_modules/eslint-plugin-react-hooks` (7.0.1) exposes `configs.recommended`,
`configs['recommended-latest']`, and `configs.flat.{recommended,recommended-latest}`.
`configs.recommended` is eslintrc-shaped (`plugins: ['react-hooks']`) but carries a usable `.rules`
object, so the `...reactHooks.configs.recommended.rules` spread in the config above works as written.
That `.rules` object has **17** rules: `rules-of-hooks`, `exhaustive-deps`, plus 15 React-Compiler rules
(`static-components`, `use-memo`, `component-hook-factories`, `preserve-manual-memoization`,
`incompatible-library`, `immutability`, `globals`, `refs`, `set-state-in-effect`, `error-boundaries`,
`purity`, `set-state-in-render`, `unsupported-syntax`, `config`, `gating`), all set to `error` by default
— confirming the rule-set-growth concern below is real rather than hypothetical.

**`react-hooks` v7 rule-set growth.** v7's `recommended` pulls in that React Compiler rule family in
addition to `rules-of-hooks`/`exhaustive-deps`. On a codebase with 22 existing `exhaustive-deps`
disables this could surface a large batch of new findings — the one thing that stays open until
`yarn install` + `yarn lint` actually runs is *how many* of the 15 fire here. Mitigation, in order of
preference: (a) fix them if few and mechanical; (b) demote the noisy new rules to `warn` — `yarn lint`
has no `--max-warnings`, so warnings do not fail CI, matching how the two pre-existing overrides are
already set to `warn`; (c) if v7 is disproportionate, stay on `^7.1.1` (v6's peer range caps at
`eslint@^9.0.0` and does not satisfy `eslint@^10`, so downgrading the plugin is not an option) and
instead explicitly turn off the React-Compiler rule family in the flat config, keeping only
`rules-of-hooks`/`exhaustive-deps`. Any of the three keeps the ESLint-10 goal intact.

Because flat config lints only JS by default, `files: ['**/*.{ts,tsx}']` is required for the TS rules to
apply; `tseslint.configs.recommended` supplies the TS parser wiring. Verify coverage did not silently
shrink (see Testing Strategy — file-count check).

devDep churn for this commit: add `@eslint/js`, `typescript-eslint`, `globals`; bump `eslint` to `^10`,
`eslint-plugin-react-hooks` to `^7.1.1`, `eslint-plugin-react-refresh` to `^0.5`;
**remove** `@typescript-eslint/eslint-plugin`, `@typescript-eslint/parser` (superseded by the unified
`typescript-eslint` package) and `@eslint/eslintrc` (dead — see Current State). Delete `.eslintrc.json`.

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
lets `postcss.config.mjs` be deleted outright — v4 does its own `@import` inlining and vendor
prefixing, so `autoprefixer` is no longer needed. Vitest does not process CSS by default
(no `css: true` in the `test` block), so tests are unaffected by the pipeline swap.

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
  `border-color` on every element to `hsl(var(--border))`. So the 391 bare-`border` class occurrences in
  `src` are unaffected, and the compat shim from the upgrade guide is **not** needed. This is the single
  biggest de-risking finding for this bump.
- **Default ring width `3px` → `1px`**: bare `ring` occurrences in `src` = **0**. Every usage is an
  explicit width (`ring-2` ×12, `ring-1` ×4, `ring-inset` ×4) or a color (`ring-accent-link/50` ×7,
  `ring-accent-link` ×4, `ring-ring` ×2, `ring-brand-gold` ×2, `ring-destructive`, `ring-red-400`) or an
  offset (`ring-offset-*` ×5). No width regression.
- **Default ring color `blue-500` → `currentColor`**: a `ring-1`/`ring-2` with no companion color class
  will change hue. Enumerate those sites during implementation and add an explicit ring color where the
  rendered color mattered. Low count, visual-only.
- **Browser support floor rises to Safari 16.4+ / Chrome 111+ / Firefox 128+.** `package.json` has no
  `browserslist` field and there is no `.browserslistrc` (the `browserslist ^4.28.1` devDep is a
  transitive-pin artifact, not a target declaration), so nothing to edit — but this is a user-visible
  change and belongs in the changelog entry.
- **Buttons lose default `cursor: pointer`.** Installed `tailwindcss@3.4.19`'s
  `src/css/preflight.css:343-346` sets `button, [role="button"] { cursor: pointer }`; that rule does not
  exist in `tailwindcss@4.3.3`'s preflight. The app has 117 `<button>` tags plus 6 `role="button"`
  elements against only 58 `cursor-pointer` occurrences repo-wide (and some of those are on non-buttons),
  so most buttons would silently lose their pointer cursor. Add the v4 upgrade guide's compat rule to
  `globals.css`'s `@layer base`: `button:not(:disabled), [role="button"]:not(:disabled) { cursor: pointer
  }`.
- **Input placeholder color changes.** Installed `tailwindcss@3.4.19`'s `src/css/preflight.css:333-337`
  sets `input::placeholder, textarea::placeholder { color: theme('colors.gray.400') }`; that rule is also
  absent from v4's preflight, so placeholders fall back to the browser default. 43 `placeholder=`
  attributes exist in `src` against only 7 explicit `placeholder-*` color classes. Add an explicit
  `input::placeholder, textarea::placeholder` color rule to `globals.css`'s `@layer base` to preserve the
  current look.

#### Utility renames required

Counts are `grep` occurrences over `analytics-web-app/src`:

| v3 | v4 | count |
|---|---|---|
| `outline-none` | `outline-hidden` | 83 |
| `rounded` (bare) | `rounded-sm` | 123 |
| `rounded-sm` | `rounded-xs` | 6 |
| `flex-shrink-*` | `shrink-*` | 22 |
| `blur` (bare) | `blur-sm` | 15 |
| `blur-sm` | `blur-xs` | 1 |
| `backdrop-blur-sm` | `backdrop-blur-xs` | 1 |
| `shadow` (bare) | `shadow-sm` | 1 |
| `shadow-sm` | `shadow-xs` | 1 |

**Ordering hazard**: the `rounded`/`blur`/`shadow` scales are *shifted*, not remapped — a naive
find-and-replace that rewrites `rounded` → `rounded-sm` before `rounded-sm` → `rounded-xs` will
double-shift the 6 original `rounded-sm` sites into `rounded-xs`-then-nothing. Run
`yarn dlx @tailwindcss/upgrade` (the official codemod, Node 20+) rather than hand-editing; it applies
the renames in the correct order and is the sanctioned migration path. Review its diff before
committing, and reject any part of it that tries to convert `tailwind.config.ts` into `@theme` (that is
the deliberate `@config` decision above).

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

#### Latent bug to verify (not to fix speculatively)

`analytics-web-app/src/components/ui/DateTimePicker.css` references `var(--color-accent-link)`,
`var(--color-app-card)`, `var(--color-theme-text-primary)`, `var(--color-theme-text-muted)`,
`var(--color-theme-border)`, `var(--color-theme-text-secondary)` — i.e. **v4-shaped `--color-*` theme
variable names**. Tailwind v3 with a JS config emits no `--color-*` variables, and `globals.css` defines
the raw names (`--accent-link`, `--card-bg`, `--text-primary`, `--text-muted`, `--border-color`,
`--text-secondary`), so these six references resolve to nothing today and those react-day-picker styles
are silently inert. Under v4 they may start resolving (v4 materializes theme colors as `--color-*`),
which would be a *behavior change* — a previously-dead rule coming to life. During implementation,
check in the browser whether the calendar picker's colors change; if they resolve to something wrong, or
do not resolve at all under `@config`, point them at the raw variables `globals.css` actually defines.
Either way this is a small, contained fix inside the Tailwind commit's blast radius.

## Implementation Steps

Three commits on one branch. Run `python3 build/analytics_web_ci.py` (type-check → lint → test → build,
exactly what `.github/workflows/analytics-web-app.yml` runs) to green **before each commit**, so the
branch bisects cleanly.

### Commit 1 — `date-fns` 2 → 4

1. `analytics-web-app/package.json`: `"date-fns": "^2.30.0"` → `"^4.4.0"`.
2. `yarn install` in `analytics-web-app/`. Confirm `yarn.lock` now holds a single `date-fns@npm:4.x`
   entry (was two: `^2.30.0` → `2.30.0` and `^4.1.0` → `4.1.0`) — `grep -n '^"date-fns' yarn.lock`.
3. `yarn type-check` — catches any stricter-typing fallout at
   `src/components/ui/DateTimePicker.tsx:3`. No edit expected.
4. Full `python3 build/analytics_web_ci.py`, then commit.

### Commit 2 — `eslint` 8 → 10 + flat config

0. **Before touching `package.json`**, with ESLint 8 still installed, capture the pre-migration baseline
   against the still-live `.eslintrc.json`: `yarn eslint . -f json | jq 'length'` (or the file list). On
   the current tree this is **253 files (131 `.tsx`, 122 `.ts`)** — record this number for the step 5
   comparison, since ESLint 10 removes eslintrc support entirely and no eslintrc-based run is possible
   once step 2 installs it.
1. `analytics-web-app/package.json` devDeps: bump `eslint` → `^10.8.0`,
   `eslint-plugin-react-refresh` → `^0.5.3`, `eslint-plugin-react-hooks` → `^7.1.1`; add
   `@eslint/js ^10.0.1`, `typescript-eslint ^8.66.0`, `globals ^17.9.0`; remove
   `@typescript-eslint/eslint-plugin`, `@typescript-eslint/parser`, `@eslint/eslintrc`.
2. `yarn install`.
3. Create `analytics-web-app/eslint.config.js` per the Design shape (the
   `...reactHooks.configs.recommended.rules` spread is confirmed against `grafana/node_modules`, not an
   install-time unknown — see Design); `git rm analytics-web-app/.eslintrc.json`.
4. `yarn lint`. Triage findings in two buckets: **(a)** the three new `eslint:recommended` rules
   (`no-unassigned-vars`, `no-useless-assignment`, `preserve-caught-error`) — fix these; **(b)** new
   `react-hooks` v7 rules — apply the Design's mitigation ladder (fix / demote to `warn` / turn off the
   React-Compiler rule family). Whichever is chosen, note it in the commit message.
5. Confirm lint coverage did not shrink (see Testing Strategy).
6. Full `python3 build/analytics_web_ci.py`, then commit.

### Commit 3 — `tailwindcss` 3 → 4 (+ `tailwind-merge` 3)

1. From `analytics-web-app/`, run `yarn dlx @tailwindcss/upgrade`. Do not accept the run blindly —
   `git diff` it in full.
2. Reconcile the codemod's output with the deliberate decisions here:
   - `src/styles/globals.css` must end up as `@import "tailwindcss";` +
     `@config "../../tailwind.config.ts";`, with both `@layer base` blocks and both `@apply` uses
     intact. **Revert** any attempt to inline `tailwind.config.ts` into `@theme`.
   - Confirm it applied all nine renames from the Design table, in the correct shifted order, and that
     `tailwind.config.ts` gained the `borderRadius.xs` entry (or the 6 `rounded-sm` sites were left
     alone) so `rounded-xs` stays value-preserving.
3. Switch the build pipeline to the Vite plugin: add `@tailwindcss/vite ^4.3.3` to devDeps, add
   `tailwindcss()` to the `plugins` array in `analytics-web-app/vite.config.ts`, delete
   `analytics-web-app/postcss.config.mjs`, and remove the `autoprefixer` devDep. Keep the `postcss`
   devDep and the `postcss` `resolutions` pin (both are security-pin machinery for transitive users,
   independent of Tailwind).
4. `analytics-web-app/package.json`: `"tailwindcss": "^3.3.0"` → `"^4.3.3"`,
   `"tailwind-merge": "^2.0.0"` → `"^3.6.0"`. Leave `@tailwindcss/typography ^0.5.19` alone — the
   existing caret range already admits the v4-compatible `0.5.20`.
5. `yarn install`, then `yarn build`, and diff the emitted CSS bundle size/shape for anything alarming.
6. Manual visual pass against a running app (`python3 local_test_env/ai_scripts/start_services.py
   --monolith`, then `yarn dev`): specifically the `ring-1`/`ring-2`-without-a-color sites, the
   `DateTimePicker` calendar for the `--color-*` variable question above, that buttons/`role="button"`
   elements still show a pointer cursor, and that input/textarea placeholders still render in the
   expected muted color.
7. Full `python3 build/analytics_web_ci.py`, then commit.

### Final — changelog

Add three bullets under `## Unreleased` → `**Web App:**` in `CHANGELOG.md`, matching the existing
dependency-bump entry style, referencing `(#1255)`. The Tailwind bullet must call out the raised browser
floor (Safari 16.4+ / Chrome 111+ / Firefox 128+) as user-visible, and the `tailwind-merge` v3 bump as
part of the same change. Also record the documented hold on `grafana/`'s React 18 with its reason
(`@grafana/ui@12.4.6` peers `react: ^18.0.0`).

## Files to Modify

**Commit 1**
- `analytics-web-app/package.json` — `date-fns` range
- `analytics-web-app/yarn.lock` — dedupe to one `date-fns` entry

**Commit 2**
- `analytics-web-app/package.json` — eslint devDep set
- `analytics-web-app/yarn.lock`
- `analytics-web-app/eslint.config.js` — **new**
- `analytics-web-app/.eslintrc.json` — **deleted**
- `analytics-web-app/src/**` — only if new-rule findings need fixes

**Commit 3**
- `analytics-web-app/package.json` — `tailwindcss`, `tailwind-merge`, `@tailwindcss/vite`; drop `autoprefixer`
- `analytics-web-app/yarn.lock`
- `analytics-web-app/src/styles/globals.css` — `@import` + `@config`
- `analytics-web-app/vite.config.ts` — add `tailwindcss()` plugin
- `analytics-web-app/postcss.config.mjs` — **deleted**
- `analytics-web-app/src/**/*.tsx` — ~253 utility-class renames
- `analytics-web-app/src/components/ui/DateTimePicker.css` — only if the `--color-*` check says so
- `analytics-web-app/tailwind.config.ts` — add `borderRadius.xs` entry so `rounded-sm` → `rounded-xs`
  stays value-preserving (kept, still loaded via `@config`)

**Final**
- `CHANGELOG.md`

Not modified: anything under `grafana/` (see the documented hold).

## Trade-offs

- **One PR with three atomic commits, vs. the issue's "do as separate PRs".** The issue asked for
  separate PRs because each bump is independently breaking. Three commits on one branch, each
  independently CI-green, preserves the property that actually matters — bisectability and a clean
  revert per bump — while keeping one review thread for what is one coherent maintenance task in one
  package. If review prefers the original split, the branch can be cut at the commit boundaries with no
  rework. Flagging this because it is a deliberate deviation from the issue text.
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
- **`.github/dependabot.yml` and the `resolutions` block are left alone.** Both are noted in #1255 but
  neither is a version bump; automating dependency updates is a policy change with its own blast radius
  (PR volume, CI cost), and pruning `resolutions` requires re-checking 11 advisories. Each deserves its
  own issue rather than riding along here.

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
- **Commit 1 specific**: assert the lockfile dedupe (`grep -n '^"date-fns' analytics-web-app/yarn.lock`
  should show one entry, not two).
- **Commit 2 specific — lint coverage must not silently shrink.** Flat config lints only JS unless
  `files` says otherwise, so a mis-scoped config can pass `yarn lint` by checking almost nothing.
  Compare `yarn eslint . --debug 2>&1 | grep -c 'Linting'` (or the file list from
  `yarn eslint . -f json`) against the pre-migration baseline captured in Commit 2 step 0, before
  `eslint` was bumped: **253 files (131 `.tsx`, 122 `.ts`)**. The counts should match.
- **Commit 3 specific**: `yarn build` must succeed and emit CSS; then a manual pass in `yarn dev`
  covering the areas the default-value analysis flagged — the colorless `ring-1`/`ring-2` sites, and the
  `DateTimePicker` calendar (the `--color-*` question). Spot-check a few high-traffic screens for the
  renamed `rounded`/`outline-none` utilities rendering as before.
- **Grafana plugin**: untouched by this change, but `grafana/`'s own CI
  (`.github/workflows/grafana-plugin.yml`) runs on a path filter that this branch does not trip, so no
  action needed. The issue's acceptance criterion "`yarn lint`/`type-check`/`build`/`test` pass for both
  packages" is satisfied for `grafana/` by its unchanged, already-passing state.

## Risks

- **`eslint-plugin-react-hooks` v7 rule-set expansion is the one genuinely open-ended item.** If its
  `recommended` set floods the codebase with React Compiler findings, the mitigation ladder in Design
  (fix → demote to `warn` → turn off the React-Compiler rule family, staying on `^7.1.1` — `^6` is not a
  fallback option, its peer range caps at `eslint@^9.0.0`) keeps the commit shippable without an
  unbounded fix-up. The ESLint-10 objective does not depend on which rung is used.
- **The Tailwind codemod's output needs real review**, not just a passing build — the renames are
  visual, and `yarn build` succeeding proves nothing about whether a `rounded` became the right
  `rounded-sm`. The manual pass in step 6 is load-bearing, not ceremonial.
- Everything else is bounded: `date-fns` is one import site against a version already in the tree, and
  the two scariest Tailwind defaults (border color, ring width) were both shown inapplicable to this
  codebase by direct inspection.

## Open Questions

None blocking. Two decisions are deliberately made in-plan and recorded here so review can override
cheaply: **ESLint 10 rather than 9** (Trade-offs), and **`@config` rather than porting the theme to
`@theme`** (Trade-offs). The `react-hooks` v7 noise level is unknown until `yarn install` + `yarn lint`
runs, which is why it is written as a mitigation ladder rather than a question — implementation resolves
it from the actual output.
