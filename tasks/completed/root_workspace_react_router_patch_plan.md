# Bump Root Workspace react-router Resolution to 8.x Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1350

## Overview

Dependabot alert #395 (`GHSA-qwww-vcr4-c8h2`, "React Router: RSC Mode CSRF Bypass", high
severity, vulnerable range `>= 7.12.0, < 8.3.0`) flags the root workspace's
`resolutions."react-router": "^7.18.0"` (`package.json:39`), which resolves to `7.18.1` — inside
the vulnerable range. A plain version bump to `^8.3.0` was already attempted and reverted earlier
today (commit `6d83fa29a`) because react-router 8.x dropped CommonJS support entirely, breaking
Jest's ability to load `@grafana/ui`'s legacy `react-router-dom-v5-compat` shim. This plan bumps
to `8.3.0` anyway, but pairs it with a small `yarn patch` that neutralizes the one line actually
responsible for the Jest breakage (dead code, never reachable in this app), plus a minimal Jest
config change for a second, genuinely new ESM-only transitive dependency. `react-router@8.3.0`
also declares an `engines.node >=22.22.0` floor, so this plan bumps `grafana/.nvmrc` and the
`grafana-plugin` CI workflow from Node 20 to 22, mirroring the same bump `analytics-web-app`
already made in #1351 for the identical requirement. All of this is narrowly scoped and verified
working end-to-end under Node 22 (`yarn test:ci`, `yarn build`, `yarn lint:fix`, `yarn typecheck`
all pass).

## Current State

- `package.json:39`: `"react-router": "^7.18.0"` under `resolutions`, resolving to `7.18.1` in
  `yarn.lock` — inside the alert's vulnerable range (`>= 7.12.0, < 8.3.0`).
- This resolution exists purely to force a patched version of `react-router` wherever it's pulled
  in transitively — nothing in the root workspace uses `react-router` directly. The sole consumer
  is `@grafana/ui@12.4.6`'s `react-router-dom-v5-compat@^6.26.1` (a legacy v5-routing-API compat
  shim used internally by `@grafana/ui`'s `Link`/`TextLink` components), which itself depends on
  `react-router@6.30.0` — a request the root `resolutions` entry hard-overrides to whatever the
  root key resolves to, in this case forcing the whole workspace onto `7.18.1` (or, after this
  plan, a patched `8.3.0`) regardless of what any nested package asks for. After this plan,
  that override forces `react-router-dom-v5-compat`'s pinned `react-router@6.30.0` (and
  `react-router-dom@5.3.4`'s own pinned `react-router` request) two majors forward to `8.3.0`,
  which drops several export names those packages statically import — see Trade-offs for why
  this is currently inert.
- **Why a plain bump to `^8.3.0` doesn't work**: confirmed by reproducing today's already-reverted
  attempt (commit `114bdd172` → revert `6d83fa29a`). `react-router@8.3.0`'s package.json declares
  `"type": "module"` with no `"require"` export condition — it is pure ESM, no CJS build at all.
  Jest's CJS-based module loader (`jest-runtime`) evaluates whatever file the package's `exports`
  map resolves to as a raw script; when that file contains `import`/`export`/`import.meta` syntax
  and isn't run through a transform first, Jest throws `SyntaxError: Cannot use 'import.meta'
  outside a module` (or `Unexpected token 'export'`). Verified directly: `yarn test:ci` in
  `grafana/` fails on 4 of 5 suites with this exact error, all originating from
  `node_modules/react-router/dist/production/lib/dom/ssr/routeModules.js:25`, reached via
  `react-router-dom-v5-compat` → `@grafana/ui`'s `Link.cjs` → `TimeRangePicker`/`Icon` →
  `BuilderView.tsx`/`utils.ts`/`ConfigEditor.tsx` (three of our own component test files import
  `@grafana/ui` pieces that transitively pull in `Link`).
- **Why downgrading below the vulnerable range doesn't work either**: versions `7.0.0`–`7.11.0`
  fall outside alert #395's vulnerable range, but the *previous* bump from `6.x` to `7.18.0`
  (commit `f0512e455`) was itself required to fix three other advisories with no fix in the 6.x
  line: `GHSA-wrjc-x8rr-h8h6` and `GHSA-337j-9hxr-rhxg` (patched only at `7.18.0`) and
  `GHSA-jjmj-jmhj-qwj2` (vulnerable `7.9.6`–`7.12.0`, patched at `7.13.0`). Since the fix version
  for those (`7.18.0`/`7.13.0`) is itself *inside* alert #395's vulnerable range
  (`>= 7.12.0, < 8.3.0`), there is no 7.x version that is simultaneously patched against all four
  advisories — confirmed empirically by pinning `7.11.0` and running the grafana test suite, which
  passes, but reopens the three other CVEs. A downgrade is not a valid fix.
- **The advisory's actual trigger doesn't apply here, but the alert can't be dismissed anyway**:
  per the GHSA-qwww-vcr4-c8h2 advisory text, "this only affects your application if you are using
  the unstable RSC APIs" — this workspace uses `react-router` only via the v5-compat `Link` shim,
  never RSC/framework-mode routing. That makes the alert a false positive in practice, but repo
  policy (`CLAUDE.md`) is to never dismiss a Dependabot alert except via an actual code/dependency
  fix, so this plan proceeds with a real fix rather than a suppression.
- **The just-landed Grafana SDK upgrade (12.4.6, #1354, commit `82bee5cc6`) does not change any of
  this**: confirmed by re-running the same 8.3.0 bump against the current `@grafana/ui@12.4.6`.
  `@grafana/ui@12.4.6` still depends on `react-router-dom-v5-compat@^6.26.1` (unchanged from
  11.6.7), so the same Jest failure reproduces identically.
- `grafana/jest.config.js` (not the scaffolded `.config/jest.config.js`) currently sets:
  ```js
  transformIgnorePatterns: ["node_modules/?!(d3-interpolate)"],
  ```
  This pattern is malformed — `?!` here is just an optional `/` followed by a literal `!`
  character, not a negative-lookahead group, so the pattern never matches any real path (verified
  with `node -e`). The practical effect: this override causes **every** `.js`/`.ts`/`.jsx`/`.tsx`
  file under `node_modules` to be treated as transform-eligible by `@swc/jest` (nothing is
  excluded), which is what has been silently letting other ESM-only packages under `@grafana/ui`
  (e.g. `marked`, `d3-color`, `react-calendar`) parse successfully today, even though the pattern
  looks broken by inspection. **Do not "fix" this regex to a properly-scoped allowlist** — doing so
  changes what gets excluded for every other package too, and reopens parse failures for
  `marked`/`d3-color`/`react-calendar`/`get-user-locale` (confirmed by testing a corrected version
  of this pattern: it cascades through four unrelated ESM packages before converging, none of which
  are related to this issue). Leave this line untouched.
- The one real gap `transformIgnorePatterns` doesn't paper over: `@swc/jest`'s `transform` key is
  keyed only on `'^.+\\.(t|j)sx?$'` (`.ts`/`.tsx`/`.js`/`.jsx`), which does not match `.mjs`.
  `react-router@8.3.0` pulls in a new dependency, `cookie-es` (replacing 7.x's `cookie` +
  `set-cookie-parser`), which ships only `dist/index.mjs`. Since no transform is configured for
  `.mjs`, Jest loads it raw and fails on `Unexpected token 'export'` — this is a genuinely new gap
  introduced by the 8.x bump, unrelated to the pre-existing `transformIgnorePatterns` quirk.
- **Node version floor**: `react-router@8.3.0`'s `package.json` declares
  `"engines": {"node": ">=22.22.0"}`. `grafana/.nvmrc` (and root `.nvmrc`) currently pin Node
  `20`, and `.github/workflows/grafana-plugin.yml:43` hardcodes `node-version: '20'` for its
  `ubuntu-latest` fallback path; `build/grafana_ci.py`'s `setup_nvm_and_node` reads
  `grafana/.nvmrc` for the self-hosted-runner path. This is the same floor `analytics-web-app`
  hit in #1351, resolved there by bumping its own `.nvmrc` to the floating major `22` — this plan
  does the same for `grafana/`. Yarn Berry does not hard-enforce `engines` at install time (a
  plain `yarn install` under Node 20 completes with no engine error), so staying on Node 20
  wouldn't necessarily break the install — but running under an unsupported Node major is an
  unforced risk for no benefit, since nothing else in `grafana/` requires staying on 20. Separately,
  this bump does add a new contributor to `yarn install`'s peer-dependency warning output:
  `react-router@7.18.1` peer-required `react: ">=18"` (satisfied by this workspace's
  `react@18.3.1`), but `react-router@8.3.0` requires `react: ">=19.2.7"` — unsatisfiable here since
  `@grafana/ui`/`@grafana/data`/`@grafana/runtime`@12.4.6 all peer-require `^18.0.0` and the
  workspace pins `react@^18`. This is not a pre-existing, unrelated warning — react-router
  genuinely joins the non-overlapping peer-dependency warning set as a new contributor (see
  Trade-offs for the related, currently-inert `react-router-dom-v5-compat` export skew this same
  override introduces). The shared CI runner
  image (`docker/github-runner.Dockerfile`) already pre-installs both Node 20 and 22 via `nvm`
  (added for `analytics-web-app`'s #1351 bump), so no Docker image changes are needed here — only
  its explanatory comment needs a small update since grafana no longer pins 20. Verified: the full
  patch + jest-config fix, plus this `.nvmrc`/workflow bump, produces a clean `yarn test:ci`
  (47/47), `yarn build`, `yarn lint:fix`, and `yarn typecheck` when run under Node `22.22.2`
  (not just whatever Node the plan happened to be authored under).
- The Docker frontend-builder stages in `docker/analytics-web.Dockerfile`, `docker/monolith.Dockerfile`,
  and `docker/all-in-one.Dockerfile` (`node:22-alpine`) build only `analytics-web-app`, not the
  grafana plugin — confirmed by reading each stage's `COPY`/`RUN` steps, which reference
  `analytics-web-app/` exclusively. No changes needed there.

## Design

Three independent, narrowly-scoped changes, all verified together to produce a fully green
`yarn test:ci` (47/47 tests) and a clean `yarn build` under Node 22:

1. **`yarn patch` on `react-router@8.3.0`**: neutralize the single dead-code line responsible for
   the `import.meta` parse failure. The failing code is inside `loadRouteModule` in
   `dist/{production,development}/lib/dom/ssr/routeModules.js`:
   ```js
   async function loadRouteModule(route, routeModulesCache) {
     ...
     } catch (error) {
       console.error(`Error loading route module \`${route.module}\`, reloading page...`);
       console.error(error);
       if (window.__reactRouterContext && window.__reactRouterContext.isSpaMode && import.meta.hot) throw error;
       window.location.reload();
       return new Promise(() => {});
     }
   }
   ```
   `loadRouteModule` is part of react-router's framework-mode lazy-route-module loading (Remix-style
   data routers) — never invoked by `react-router-dom-v5-compat`'s simple `<Link>`-only usage, and
   this app does no client-side routing at all. `import.meta.hot` specifically guards Vite's HMR
   dev mode, which this webpack-built plugin never runs under. The condition is unreachable dead
   code in every context this app runs in (Jest, webpack dev, webpack prod). The patch replaces
   `import.meta.hot` with `false`, preserving identical runtime behavior (the check was already
   effectively `false` in a non-Vite environment — `import.meta.hot` is `undefined` there) while
   removing the only ESM-only syntax construct in this file's actual require chain.

   Patch both `dist/production/lib/dom/ssr/routeModules.js` and
   `dist/development/lib/dom/ssr/routeModules.js` (identical line in both; Jest may resolve either
   depending on export conditions).

2. **Extend `grafana/jest.config.js`'s `transform` key to also handle `.mjs`**, reusing the exact
   same `@swc/jest` transformer already configured for `.ts`/`.tsx`/`.js`/`.jsx`, so `cookie-es`
   parses correctly. `transformIgnorePatterns` is left completely untouched (see Current State
   above for why).
3. **Bump `grafana/.nvmrc` and `.github/workflows/grafana-plugin.yml`'s `node-version` from `20`
   to `22`**, satisfying react-router 8.3.0's declared `engines.node` floor, mirroring
   `analytics-web-app`'s #1351 precedent exactly.

No changes to `transformIgnorePatterns`, no new packages added to any allowlist, no changes to
`analytics-web-app` (already fixed separately by #1351), the Docker frontend-builder stages (see
Current State), or any other workspace.

## Implementation Steps

1. Bump `package.json:39`'s `resolutions."react-router"` from `"^7.18.0"` to `"^8.3.0"`.
2. Run `yarn patch react-router@^8.3.0` (or `yarn patch react-router` if that resolves
   unambiguously) to extract a working copy, edit both
   `dist/production/lib/dom/ssr/routeModules.js` and
   `dist/development/lib/dom/ssr/routeModules.js`, replacing
   `&& import.meta.hot) throw error;` with `&& false) throw error;` in each, then run
   `yarn patch-commit -s <path>`.
3. **Important**: `yarn patch-commit` auto-generates resolution override entries keyed by whatever
   descriptor(s) it happens to pick from the dependency graph (in testing, this produced
   `"react-router@npm:6.30.0"` and `"react-router@npm:5.3.4"` — the versions
   `react-router-dom-v5-compat`/`react-router-dom` request before the root override applies), which
   do **not** take effect because the existing bare `"react-router"` key in `resolutions` matches
   first and wins for every request regardless of range. After `yarn patch-commit` runs, manually
   edit `package.json` so the single existing `"react-router": "^8.3.0"` key's *value* itself
   becomes the patch descriptor it generated (e.g.
   `"react-router": "patch:react-router@npm%3A8.3.0#~/.yarn/patches/react-router-npm-8.3.0-<hash>.patch"`),
   and delete the extra per-version entries `yarn patch-commit` added. Run `yarn install` and
   confirm `node_modules/react-router/dist/production/lib/dom/ssr/routeModules.js` contains
   `&& false) throw error;` (not `import.meta.hot`) before proceeding.
4. In `grafana/jest.config.js`, extend the `transform` override to also map `.mjs` files to the
   same `@swc/jest` config already used for `.ts`/`.tsx`/`.js`/`.jsx`:
   ```js
   module.exports = {
     // Jest configuration provided by Grafana scaffolding
     ...require('./.config/jest.config'),
       // Inform jest to only transform specific node_module packages.
       transformIgnorePatterns: ["node_modules/?!(d3-interpolate)"],
     // cookie-es (pulled in transitively via react-router) ships only a .mjs build; the scaffolded
     // transform regex only matches .ts/.tsx/.js/.jsx, so .mjs files are never transformed otherwise.
     transform: {
       ...require('./.config/jest.config').transform,
       '^.+\\.mjs$': require('./.config/jest.config').transform['^.+\\.(t|j)sx?$'],
     },
   };
   ```
   Do not touch `transformIgnorePatterns` — leave the existing line exactly as-is (see Current
   State for why).
5. Bump `grafana/.nvmrc` from `20` to `22`, and `.github/workflows/grafana-plugin.yml:43`'s
   `node-version: '20'` to `node-version: '22'`. Also bump `grafana/package.json:76`'s own
   `"engines": {"node": ">=20"}` to `">=22"` (precedent: `tasks/completed/grafana_sdk_v12_upgrade_plan.md`
   bumped this same field from `>=16` to `>=20` to track `.nvmrc`/tooling requirements — it must
   move in lockstep here too, otherwise the package falsely declares support for a Node major that
   no longer satisfies its own transitive dependency's `engines` floor). Update
   `CONTRIBUTING.md:358` and its manually-maintained mirror `mkdocs/docs/contributing.md:238`
   (both currently read "Node.js 20+ (matches `.nvmrc` and all CI workflows; Yarn 4 requires
   ≥18.12)" under "Grafana Plugin Development" → "Prerequisites") from "Node.js 20+" to "Node.js
   22+". Update the stale comment in `docker/github-runner.Dockerfile` above the `nvm install
   20`/`nvm install 22` block, which currently says grafana pins `20` — no functional change
   needed there (both versions are already pre-installed for `analytics-web-app`'s sake, and other
   workspaces may still rely on the root `.nvmrc`'s `20`), just correct the comment.
6. Fix `build/grafana_ci.py`'s `run_cmd` to `nvm use` the explicit resolved Node version
   (`setup_nvm_and_node`'s return value) instead of a bare `nvm use`: the bare form resolves the
   nearest `.nvmrc` by walking up from `cwd`, which is wrong for steps invoked with
   `cwd=repo_root` (they'd pick up the root `.nvmrc`, not `grafana/.nvmrc`) — a bug that stayed
   latent while both `.nvmrc`s pinned `20`, but went live the moment this branch bumped
   `grafana/.nvmrc` to `22`.
7. Run `yarn install` from the repo root to regenerate `yarn.lock`.
8. Verify from `grafana/`, **under Node 22** (e.g. `nvm use 22` first — matching the new
   `.nvmrc`/CI pin, not whatever Node happens to be active locally): `yarn test:ci` (expect 5/5
   suites, 47/47 tests passing), `yarn build` (expect a clean compile — the ~60 pre-existing
   `immutable`/`@react-awesome-query-builder` warnings are unrelated and unchanged), `yarn
   lint:fix`, `yarn typecheck`.
9. Add a `CHANGELOG.md` entry under **Unreleased** / **Build**, following the precedent of the
   adjacent react-router entries in that section: note the bump to `^8.3.0` resolving Dependabot
   alert #395 (GHSA-qwww-vcr4-c8h2), the `yarn patch` neutralizing the dead
   `import.meta.hot` HMR guard in react-router's framework-mode `loadRouteModule`, the
   `.mjs` transform addition for `cookie-es`, and the `grafana/.nvmrc`/CI Node 20→22 bump required
   by react-router 8.3.0's `engines.node` floor. In the same edit, amend the existing
   `CHANGELOG.md:27` bullet (the `brace-expansion` entry, still under this same `Unreleased`/Build
   section), which currently ends "...Dependabot alert 395 (`react-router` CSRF bypass,
   GHSA-qwww-vcr4-c8h2, root workspace) stays open — react-router 8.x dropped CommonJS support
   entirely...so it can't be bumped past 7.x without a separate `@grafana/ui` major upgrade" —
   that claim is exactly what this plan disproves, so remove the now-stale "stays open" clause
   from that bullet (keeping the unrelated `brace-expansion` portion intact) rather than leaving
   two contradictory statements about the same alert in the same section.

## Files to Modify

- `package.json` (resolutions entry → patch descriptor)
- `yarn.lock` (regenerated)
- `.yarn/patches/react-router-npm-8.3.0-<hash>.patch` (new file, created by `yarn patch-commit`)
- `.yarnrc.yml` (yarn may add/update an `unplugged`/patch reference here automatically — verify
  after `yarn install`, only commit if it actually changes)
- `grafana/jest.config.js` (`.mjs` transform mapping)
- `grafana/.nvmrc` (`20` → `22`)
- `grafana/package.json` (`engines.node`: `>=20` → `>=22`)
- `.github/workflows/grafana-plugin.yml` (`node-version: '20'` → `'22'`)
- `docker/github-runner.Dockerfile` (comment only — no functional change)
- `build/grafana_ci.py` (`run_cmd`: `nvm use` the explicit resolved Node version instead of a bare
  `nvm use`, which was resolving the wrong `.nvmrc` when invoked with `cwd=repo_root`)
- `CONTRIBUTING.md` and `mkdocs/docs/contributing.md` ("Node.js 20+" → "Node.js 22+" in the
  Grafana Plugin Development prerequisites)
- `CHANGELOG.md` (Build entry)

## Trade-offs

- **Patch vs. downgrade vs. suppress**: downgrading reopens three previously-patched CVEs (see
  Current State); suppressing/dismissing the alert violates repo policy and the advisory's own
  language doesn't provide a config-level opt-out. A `yarn patch` targeting one dead-code line is
  the smallest change that satisfies "real fix, not a version bump that breaks the build."
- **Patch vs. fixing `transformIgnorePatterns` "properly"**: a correctly-scoped allowlist regex
  looks like the more "correct" fix, but it changes behavior for every other node_modules package
  too, not just react-router — confirmed this reopens parse failures for `marked`, `d3-color`,
  `react-calendar`, and `react-calendar`'s own dependency `get-user-locale`, none of which are
  related to this issue, and each of which would need its own further investigation. That's
  unrelated pre-existing technical debt (masked by the existing malformed pattern) and out of
  scope for a security-only fix. The `.mjs`-only `transform` extension in this plan is the minimal
  change that doesn't disturb that existing (accidentally-working) behavior.
- **Patch maintenance cost**: the `yarn patch` is pinned to `react-router@8.3.0` exactly; any
  future bump to a newer 8.x patch release will need the patch re-applied (or re-verified that the
  upstream code no longer needs it — react-router may eventually fix this at the source, e.g. by
  gating the check behind a proper environment detection instead of bare `import.meta.hot`).
  Acceptable one-line maintenance cost versus the alternatives above.
- **Peer-dependency skew is real, not cosmetic**: forcing the resolution's target from `7.18.1`
  to `8.3.0` doesn't change transparently for its dependents — it forces
  `react-router-dom-v5-compat`'s pinned `react-router@6.30.0` and `react-router-dom@5.3.4`'s own
  pinned `react-router` request two majors forward to `8.3.0`. `react-router@8.3.0` no longer
  exports 7 names `react-router-dom-v5-compat` statically imports/re-exports
  (`AbortedDeferredError`, `UNSAFE_logV6DeprecationWarnings`, `UNSAFE_mapRouteProperties`,
  `UNSAFE_useRouteId`, `UNSAFE_useRoutesImpl`, `defer`, `json`), and internally imports React 19's
  `useOptimistic`, on top of the unsatisfiable `react@>=19.2.7` peer requirement noted in Current
  State. This is verified inert today: `react-router`/`react-router-dom`/`@grafana/ui` are all
  configured as webpack externals in `grafana/.config/webpack/webpack.config.ts` (never bundled
  into the shipped plugin), and Jest's swc→CJS interop degrades the missing export names to
  `undefined` on code paths no test suite actually renders/exercises (`yarn test:ci` 47/47 and
  `yarn typecheck` both pass under Node 22.22.2). It's a real tripwire, though — it would break the
  moment grafana ever stops treating `@grafana/ui`/react-router as webpack externals, or migrates
  its test runner off Jest's CJS interop (e.g. to Vitest/native ESM).
- **Node 22 bump vs. staying on Node 20**: Yarn doesn't hard-enforce the `engines` floor, so
  staying on Node 20 would likely still install and run. Bumping to 22 was chosen instead of
  papering over the mismatch, since there's no reason to run an explicitly-unsupported Node major
  for a dependency already required by this fix, and it keeps `grafana/` consistent with
  `analytics-web-app`'s identical #1351 precedent rather than leaving the two workspaces on
  different (and, for grafana, unsupported) Node versions.

## Documentation

No project documentation references the root `react-router` resolution or this Jest quirk. Two
docs do state a Node version requirement tied to the grafana plugin specifically:
`CONTRIBUTING.md:358` and its manually-maintained mirror `mkdocs/docs/contributing.md:238`, both
under "Grafana Plugin Development" → "Prerequisites", currently reading "Node.js 20+ (matches
`.nvmrc` and all CI workflows; Yarn 4 requires ≥18.12)" — update both to "Node.js 22+" (per
Implementation Step 5). `CHANGELOG.md` also requires an update (per Implementation Step 8).

## Testing Strategy

- Run all of the below under Node 22 (`nvm use 22` / matching the new `grafana/.nvmrc`), not
  whichever Node happens to be active locally — this is the version CI and the actual grafana
  build will use once this plan lands.
- `yarn test:ci` in `grafana/` — all 5 test suites / 47 tests must pass (this is the primary
  regression check; it directly exercises the code path that broke on the first bump attempt).
- `yarn build` in `grafana/` — production webpack build must complete with no new
  warnings/errors beyond the ~60 pre-existing, unrelated `immutable` default-export warnings.
- `yarn lint:fix` and `yarn typecheck` in `grafana/` — must pass cleanly (project convention,
  required before commit per `grafana/CLAUDE.md`).
- Confirm Dependabot alert #395 closes once merged (Dependabot detects the `yarn.lock` change
  automatically).
- Manual smoke check not required beyond the automated suite: `react-router` has no direct
  application-level usage in this workspace (only reached transitively through `@grafana/ui`'s own
  `Link` component, already exercised by the Jest suites above).

## Open Questions

None — this plan bumps to a version outside the vulnerable range, keeps the app on a supported,
non-EOL react-router line, and has been verified end-to-end (tests, build, lint, typecheck) rather
than left as a hypothesis.
