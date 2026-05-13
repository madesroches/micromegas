# Issue #1034 — React 19 upgrade for `analytics-web-app`

## Overview

Bump the web app from React 18.3 → React 19.2 in a single coordinated step, along with the React-coupled libraries that block the upgrade. The forcing function is `@react-three/fiber@8` instantiating the deprecated `THREE.Clock` (Three.js 0.183 removed it in favor of `THREE.Timer`); R3F dropped `Clock` in v9, and v9 only supports React 19. Secondary motivation: most of the React ecosystem now targets 19 first, so 18.3 is the long tail.

Scope is intentionally narrow — React + RTL + R3F/drei + the `react-reconciler` pin. Radix component bumps (1.x → 2.x) and React Router 6 → 7 are explicitly **out of scope** here; they are listed in [Follow-ups](#follow-ups) and tracked separately if needed.

## Target versions

Verified against npm `latest` as of 2026-05-12:

| Package | Current | Target |
|---|---|---|
| `react`, `react-dom` | `^18.3.0` | `^19.2.0` (latest: 19.2.6) |
| `@types/react`, `@types/react-dom` | `^18.3.0` | `^19.2.0` |
| `@react-three/fiber` | `~8.18.0` | `^9.6.0` (latest: 9.6.1) |
| `@react-three/drei` | `~9.122.0` | `^10.7.7` (latest: 10.7.7) |
| `@testing-library/react` | `^14.0.0` | `^16.3.2` |
| `@testing-library/dom` | (transitive) | add `^10.4.0` as explicit dev dep (RTL 16 peer) |
| `lucide-react` | `^0.292.0` | `^1.14.0` (latest: 1.14.0) |
| `react-reconciler` resolution | `0.29.2` | **drop** |

Peer-dep reality check (already validated via `npm view` and `yarn.lock` inspection):

- R3F 9.6.1 requires `react: ">=19 <19.3"`, `three>=0.156` — we have `three@^0.183.0` ✓. Note the `<19.3` upper bound: 19.2.x is in range, but a future bump to 19.3 (currently canary only) will need a fresh R3F.
- drei 10 requires `react@^19`, `three>=0.159`, `@react-three/fiber@^9.0.0` ✓
- RTL 16 requires `react@^18 || ^19`, `@testing-library/dom@^10`
- `lucide-react@0.292.0` peers `react: ^16.5.1 || ^17.0.0 || ^18.0.0` — **does not include 19**, must bump. React 19 entered lucide-react's peer range at `0.400.0`, so any version from `0.400.0` onward would silence the yarn-peer warning; we go to `^1.14.0` (latest) for currency, accepting that this crosses the 0.x → 1.x major boundary. Risk is bounded because 1.14.0 retains backward-compat aliases for the icons currently imported (verified: `BarChart3`, `LineChart`, `AlertCircle`, etc. all still exported), and `yarn type-check` + `yarn build` will catch any rename that slipped the alias net. Yarn 4 would emit a peer warning on every install otherwise.
- Other React-coupled deps (react-day-picker, react-markdown, react-router-dom 6.30, @dnd-kit/core, @tanstack/react-query 5, @radix-ui/react-progress 1.1.8, @radix-ui/react-toast 1.2.15, @radix-ui/react-context-menu 2.2.16, @radix-ui/react-dropdown-menu 2.1.16) all list React 19 in their resolved peer ranges ✓ — no version bumps required to silence yarn peer warnings.

## Current state

### Entry point

`analytics-web-app/src/main.tsx:14-30` already uses `createRoot` and wraps in `<React.StrictMode>`. React Router 6 already has `v7_startTransition` and `v7_relativeSplatPath` future flags on, so the router side of a future v7 bump is pre-warmed (still not part of this plan).

### `forwardRef` usage (codemod target)

`React.forwardRef` is used in 16 places across 5 files:

- `src/components/ui/progress.tsx:5`
- `src/components/ui/button.tsx:42`
- `src/components/ui/toast.tsx:9,40,55,70,88,100` (6 instances)
- `src/components/ui/card.tsx:4,19,31,46,58,66` (6 instances)
- `src/components/CellContainer.tsx:1,64` (import + 1 usage)

All wrap host-DOM primitives. `forwardRef` is **not** deprecated in React 19 (it still compiles and runs), but React 19 supports `ref` as a regular prop, which is the preferred form going forward. The conversion is not part of `types-react-codemod preset-19` (types-only) or `codemod react/19/migration-recipe` (deprecated-API runtime fixes); it lives in a separate codemod, `react/19/remove-forward-ref`, which we run explicitly in Phase 1.

### Legacy-pattern audit (clean)

The codebase has **zero** hits on the patterns that typically cause React 19 pain:

- No `React.FC` / `FunctionComponent` / `VFC` annotations
- No `JSX.Element` / `JSX.IntrinsicElements` references (codemod for the JSX namespace move is a no-op)
- No `ReactDOM.render` / `hydrate` (already on `createRoot`)
- No `propTypes` / `defaultProps` (both removed in 19)
- No legacy string refs

This is unusually clean — the React 19 portion of the upgrade is largely a version bump with light forwardRef cleanup.

### R3F consumers (the actual risk)

R3F is used in exactly **one cell**, but it's a substantial one:

- `src/components/map/MapViewer.tsx` (812 lines) — the only file importing from `@react-three/fiber` and `@react-three/drei`. Wraps `<Canvas>`, uses `useThree`, `useFrame`, `ThreeEvent<MouseEvent>` / `ThreeEvent<PointerEvent>`, `useGLTF`, `Html`, `Grid`, `PerspectiveCamera`, plus heavy custom orbit/pan/raycast logic that reaches into `camera`, `gl.domElement`, and `THREE.Raycaster`.
- `src/lib/screen-renderers/cells/MapCell.tsx` (338 lines) — thin host that resolves the map URL and renders `<MapViewer>`. No R3F imports.

The recently-landed `1036_map_native_ue_coords_plan` rewrote a lot of `MapViewer`, so the code is in a known-good state and well-understood — easier to spot regressions.

### `react-reconciler@0.29.2` pin

The current `resolutions["react-reconciler"]: "0.29.2"` exists because R3F 8 has `react-reconciler@^0.27.0` as a direct dependency, which disagrees with React 18.3's hoisted version. R3F 9 removed `react-reconciler` from its dependency tree entirely (no replacement version — the custom renderer machinery moved internal), so once R3F 9 is in place nothing in the graph pulls `react-reconciler`. The resolution becomes a no-op pointing at a now-unused package and must be deleted to keep `package.json` honest.

### Tests

18 test files import from `@testing-library/react` (`render`, `renderHook`, `act`, `fireEvent`, `waitFor`). Jest setup is modern: `jest@30`, `jest-environment-jsdom@30`, `ts-jest@29` with ESM. No `@testing-library/dom` is currently in `devDependencies` — RTL 16 lists it as a peer that must be installed explicitly.

The map cell has **no** rendering test today (it renders a `<canvas>`; jsdom can't drive WebGL), so the R3F 9 hop has no automated coverage. Manual verification only — same as `1036`.

## Design

### Approach: single bundled PR

The packages are mutually constrained — R3F 9 only works on React 19, drei 10 only works on R3F 9, RTL 16 is the React-19 line. Splitting these would mean either holding an intermediate state that doesn't build, or upgrading one piece at a time and reverting the `react-reconciler` pin twice. A single PR is the correct shape.

Within the PR, work proceeds in ordered phases so each phase has a clear pass/fail signal (`yarn type-check`, `yarn lint`, `yarn test`, `yarn build`, manual map verification).

### Phase ordering rationale

1. **React first, then R3F.** Bumping React produces type errors at every R3F boundary (R3F 8's types reference React 18 internals). Running codemods on a still-failing tree is fine; the build doesn't need to be green between phases inside the PR.
2. **R3F + drei in lockstep.** drei imports R3F's internal types — bumping one without the other guarantees a TS break.
3. **RTL last.** Test code is decoupled from runtime code; doing it after the runtime is green narrows the diagnostic surface if something fails.

### R3F 8 → 9 specifics for `MapViewer.tsx`

The R3F changelog flags four migration areas; mapping each to this file:

| Change | Impact on `MapViewer` |
|---|---|
| `THREE.Clock` → `THREE.Timer` internally | Invisible — `useFrame(_, delta)` callback signature is unchanged. Confirms the issue's "deprecation warning gone" acceptance criterion. |
| Event types: `ThreeEvent<MouseEvent>` / `ThreeEvent<PointerEvent>` | Type names unchanged; payload shape unchanged. `e.stopPropagation()`, `e.instanceId` still work. Used at lines 181, 197. |
| `useThree` return shape | `camera`, `gl`, `scene` all still present. Called in two places: `UnrealCameraController` destructures `{ camera, gl }` (line 263), `SceneSetup` destructures `{ scene }` (line 662). |
| Camera helpers | We don't use drei's `<OrbitControls>` or built-in controllers — `UnrealCameraController` is hand-rolled against `THREE.Raycaster` / `THREE.Spherical` / `camera.position`. Insulated from R3F 9 changes. |

Drei 10 only touches `useGLTF`, `Html`, `Grid`, `PerspectiveCamera`. `useGLTF(url)` return shape (`{ scene, cameras, parser }`) is stable; `<Html center>`, `<Grid>`, `<PerspectiveCamera>` are unchanged in 10.x.

**Net assessment:** `MapViewer` is unlikely to need code changes beyond what the codemods touch. The risk is wrong — but the prediction is "nothing to change in this file."

### React 19 StrictMode

The issue notes "StrictMode double-invoke in 19 can surface latent effect-cleanup bugs that 18 hid." This is **not** a real concern for this codebase: StrictMode is already enabled in `main.tsx:15`, and React 18.3 already double-invokes effects under StrictMode in dev. If any effect had a latent cleanup bug, we'd already be seeing it. No action.

### JSX namespace move

React 19 moves `JSX.*` from the global namespace to `React.JSX.*`. The codebase has zero `JSX.Element` / `JSX.IntrinsicElements` references, so this is a no-op for us. The codemod is still safe to run (it's idempotent on clean code).

## Implementation steps

### Phase 1 — React 19 baseline

1. Update `analytics-web-app/package.json` dependencies:
   - `react` → `^19.2.0`
   - `react-dom` → `^19.2.0`
   - `@types/react` → `^19.2.0`
   - `@types/react-dom` → `^19.2.0`
   - `lucide-react` → `^1.14.0` (existing 0.292.0 peers omit React 19; bumping in lockstep avoids yarn peer warnings — see peer-dep note above for why we picked latest over a smaller 0.4xx bump)
2. `yarn install` from `analytics-web-app/`.
3. Run the React 19 codemod recipes:
   - `yarn dlx types-react-codemod@latest preset-19 ./src` — TypeScript types only (deprecated child-prop types, scoped JSX, refobject defaults, etc.)
   - `yarn dlx codemod@latest react/19/migration-recipe -t ./src` — runtime deprecations (string refs, `act` import, `ReactDOM.render`, etc.) (run from `analytics-web-app/`). The codemod CLI requires the target path via `-t` / `--target`; positional args are only honored by `jssg run`.
   - `yarn dlx codemod@latest react/19/remove-forward-ref -t ./src` — rewrites `forwardRef` call sites into ref-as-prop signatures. The first two codemods don't touch `forwardRef`; this one does.
4. Review the diff — expect ref-prop rewrites in the 5 files listed above. Manually check anything unexpected.
5. `yarn type-check` — expect failures from R3F 8 against React 19 types. Note them, **don't fix yet**.
6. `yarn lint` — should be clean or near-clean.

### Phase 2 — R3F 9 + drei 10

7. Update `analytics-web-app/package.json` dependencies:
   - `@react-three/fiber` → `^9.6.0`
   - `@react-three/drei` → `^10.7.7`
8. **Remove** the `resolutions["react-reconciler"]` entry. Do not replace it — R3F 9 has no `react-reconciler` runtime dependency at all; the only thing in the graph is `@types/react-reconciler`, pulled transitively as a types-only dep via `its-fine@^2.0.0`.
9. `yarn install`.
10. `yarn dedupe` — collapses now-orphaned entries (e.g., the stale `@types/react-reconciler@0.26.7` left behind by R3F 8) and keeps `yarn.lock` honest.
11. Read `src/components/map/MapViewer.tsx` end-to-end and confirm none of the affected R3F 9 surface areas (event signatures, `useThree` destructure, `useFrame` callback) need changes. If TS surfaces a real diff, fix in place; otherwise move on.
12. `yarn type-check` should now pass.
13. `yarn build`.

### Phase 3 — Testing Library 16

14. Update `analytics-web-app/package.json` devDependencies:
    - `@testing-library/react` → `^16.3.2`
    - Add `@testing-library/dom` → `^10.4.0` (RTL 16 peer)
15. `yarn install`.
16. `yarn test`. RTL 16 is largely API-compatible with 14; any failures are likely:
    - Stricter `act()` warnings around async updates (React 19 surfaces more of these). Wrap offending updates in `act()`.
    - `renderHook` return shape unchanged from RTL 14.
17. Address failures file by file. If a pattern repeats across files, fix once and propagate.

### Phase 4 — Manual verification

18. `yarn dev` and exercise the map cell against a real notebook:
    - GLB loads without console errors
    - Pan, orbit, zoom (wheel) and WASD fly-on-hover all behave as before (Q/E and the button-driven reset were removed by PR #1045)
    - Marker click selects an event; hover changes cursor
    - Z key (while hovering the cell) resets the view
    - Browser console shows **no** `THREE.Clock` deprecation warning (acceptance criterion)
    - No new StrictMode warnings beyond what was there before
19. Spot-check a non-map screen (notebook, log, table, metrics) — these are React-19-sensitive but R3F-free, so they should be unaffected.

### Phase 5 — Cleanup and PR

20. Run the AI-CI: `python3 ../build/rust_ci.py` is rust-only; the web-app gate is `yarn lint && yarn type-check && yarn test && yarn build`.
21. `git log --oneline main..HEAD` to draft the PR body.
22. PR title: `analytics-web-app: upgrade to React 19, R3F 9, drei 10, RTL 16`.
23. PR body should reference issue #1034 and call out the dropped `react-reconciler` pin.

## Files to modify

- `analytics-web-app/package.json` — version bumps (react, react-dom, types, lucide-react, R3F, drei, RTL, @testing-library/dom) + drop `react-reconciler` resolution
- `analytics-web-app/yarn.lock` — regenerated by yarn
- `analytics-web-app/src/components/ui/progress.tsx` — `forwardRef` → ref-as-prop (codemod)
- `analytics-web-app/src/components/ui/button.tsx` — same
- `analytics-web-app/src/components/ui/toast.tsx` — same (6 instances)
- `analytics-web-app/src/components/ui/card.tsx` — same (6 instances)
- `analytics-web-app/src/components/CellContainer.tsx` — same
- `analytics-web-app/src/components/map/MapViewer.tsx` — only if R3F 9 surfaces a real type/API delta; baseline expectation is no edits
- Any test file flagged by `yarn test` after Phase 3 (RTL 16 act-warning surface)

## Trade-offs

### One PR vs. staged PRs

Considered: land React 19 in one PR (broken map), then R3F 9 in a second PR. **Rejected** — would require either reverting the `react-reconciler` pin twice or leaving the map cell broken on `main` for hours/days. The R3F + React coupling makes this a single atomic change.

### Going to drei 10.7.7 vs. drei 10.4+

Issue says "10.7.7+". The drei changelog inside 10.x is small (mostly bug fixes), and we use only four imports (`useGLTF`, `Html`, `Grid`, `PerspectiveCamera`), all of which were stable since 10.0.0. Going to latest minimizes the chance of needing another bump later. **Selected: 10.7.7.**

### Holding `@testing-library/react` at 14

RTL 14 technically peer-depends on React 18, and yarn 4 enforces peers. Holding at 14 would require either bumping the peer-dep exception or living with install warnings on every CI run. **Rejected — bump to 16.**

### Bumping Radix and React Router in the same PR

Issue mentions these in "Verify". They're React-19-compatible at current pinned versions per their changelogs, so no action is required to ship 19. Mixing optional cleanup into a load-bearing upgrade adds review surface for no benefit. **Defer to follow-ups.**

## Risks

1. **R3F 9 type changes in `useThree`/event payloads.** Mitigation: read changelog + audit the four call sites listed in [R3F 8 → 9 specifics](#r3f-8--9-specifics-for-mapviewertsx) before running yarn install. Estimated probability of needing code changes: **low** (~20%).
2. **`react-reconciler` pin removal leaves a stale transitive somewhere.** R3F 9 no longer depends on `react-reconciler`, so the resolution should drop cleanly. If `yarn install` warns about an unmet peer on a stray `react-reconciler` consumer (e.g. a dev tool we haven't audited), audit `yarn why react-reconciler` and either add a fresh resolution or remove the consumer. Detect immediately after step 8.
3. **Codemod misses edge cases.** Codemods are reportedly ~70% — expect 1–2 hand-fixes. The clean baseline (no `JSX.Element`, no `React.FC`) lowers the surface area substantially.
4. **Map cell has no automated test.** Manual verification is the only signal. Same risk profile as `1036`; accept and document in the PR.

## Testing strategy

### Automated

- `yarn type-check` — gate on green
- `yarn lint` — gate on green
- `yarn test` — full suite must pass; expect to fix a handful of `act()`-warning test cases
- `yarn build` — production bundle must succeed

### Manual (acceptance criteria from issue #1034)

- Map cell renders a GLB
- Pan, orbit, GLB load all work without console errors
- `THREE.Clock` deprecation warning is gone in dev
- No new StrictMode warnings

### What is *not* tested

- The map's interaction surface (orbit/pan/zoom math) has no automated coverage. The R3F 9 upgrade is the riskiest piece of this work, and the only signal is human-driven. This is a pre-existing condition of the codebase — out of scope to fix here.

## Documentation

- `analytics-web-app/CLAUDE.md` — none currently exists for this app; the root `CLAUDE.md` lists yarn commands but doesn't pin React versions, so no update needed.
- `mkdocs/docs/` — no web-app architecture page references React/R3F versions; nothing to update.
- This plan file moves to `tasks/completed/` after merge.

## Follow-ups (out of scope)

Tracked here so they don't get lost when the PR lands:

- **Radix bumps:** `@radix-ui/react-progress@1.0` → `2.x`, `@radix-ui/react-toast@1.1` → `2.x`. These are independent of React 19 and should be a small separate PR.
- **React Router 6 → 7:** non-breaking, unlocks 19 features. Future flags are already on. Defer.
- **Web-app CI gate:** today there is no `python3 ../build/web_ci.py`-equivalent that bundles `yarn lint && type-check && test && build` — only the per-command yarn scripts. Worth adding so React-version-sensitive regressions catch in CI; tracked separately.

## Open questions

None blocking. The plan is concrete and the codebase is unusually clean for a React 19 hop.
