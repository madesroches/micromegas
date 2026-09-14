# Prerender the `welcome` Landing Page Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1574

## Overview

`https://micromegas.info/` is served as a client-rendered SPA: the HTML body is
`<div id="root"></div>` and nothing else, so every word of the landing-page copy — the pitch, the
four stages, the throughput and cost figures — exists only after the JS bundle runs. Crawlers that
do not execute JavaScript (CCBot and most non-Google fetchers) see a title and a meta description
and nothing more, while `/docs/` and `/rustdoc/` on the same host ship real HTML.

This plan adds a build-time prerender step to `welcome/`: after `vite build`, a Node script builds
an SSR bundle of the app, calls `renderToString`, and injects the markup into `dist/index.html`.
The client switches from `createRoot` to `hydrateRoot` so it attaches to that markup instead of
discarding it, and a `<noscript>` style override makes the fade-in wrappers visible when JS never
runs. The script fails the build if the injection does not happen, which is the regression guard —
a silent prerender failure would otherwise leave a green build and a blank page.

## Current State

### The app

`welcome/` is a single-page Vite + React 18 app with no router, no data fetching, and no state:

- `welcome/src/main.tsx` — `ReactDOM.createRoot(document.getElementById('root')!).render(<React.StrictMode><App /></React.StrictMode>)`.
- `welcome/src/App.tsx` — renders `Navbar`, `Hero`, then `HowItWorks`, `Differentiators`,
  `Notebooks`, `Integrations`, `Footer`, each of the last five wrapped in a local `FadeIn`.
- `welcome/src/components/*.tsx` — static JSX. A grep across all seven for `window`, `document`,
  `Math.random`, `Date.`, `localStorage`, `useState`, `matchMedia` returns nothing. There is no
  `useState` anywhere in the app, so there is no render-time nondeterminism to produce a hydration
  mismatch.

The only browser API in the tree is `IntersectionObserver`, inside `FadeIn`'s `useEffect`
(`App.tsx:13-29`), which does not run during server rendering.

### `FadeIn` hides its children until JS reveals them

`App.tsx:31-38`:

```tsx
<div
  ref={ref}
  className="opacity-0 translate-y-8 transition-all duration-700 ease-out"
>
```

The effect adds `opacity-100 translate-y-0` and removes `opacity-0 translate-y-8` once the element
intersects. Prerendering alone therefore produces markup that is *present* but, with JS disabled,
*invisible* — five of the seven sections, i.e. nearly all of the ~400 words of copy. Crawlers parse
markup rather than computed style, so this does not affect the issue's primary goal, but the no-JS
rendering would still be wrong for a human.

`Navbar` and `Hero` are not wrapped and are unaffected.

### The build

`welcome/package.json`: `"build": "tsc && vite build"`. `welcome/vite.config.ts` is
`{ plugins: [react()], base: '/' }` — no SSR configuration.

`.github/workflows/publish-docs.yml` has a *Build welcome page* step that runs
`cd welcome && yarn install && yarn build`, and a later step that copies `welcome/dist/*` into
`public_docs/` as the site root. The workflow runs on pushes to `main` and on pull requests
touching `welcome/**`, so anything wired into `yarn build` runs in CI on every PR that touches this
directory, with no workflow change.

`welcome/dist/index.html` as built today keeps `<div id="root"></div>` verbatim, exactly once —
Vite rewrites only the `<script>`/`<link>` tags:

```html
  <body>
    <div id="root"></div>
    <script data-goatcounter="..." async src="//gc.zgo.at/count.js"></script>
  </body>
```

### Toolchain facts that constrain the approach

Confirmed against `welcome/node_modules`:

- `vite@8.0.16`. Its node entry exports `build`, `createServer`, `createBuilder`,
  `createServerModuleRunner` — it does **not** export `ssrLoadModule`. The "spin up a dev server and
  `ssrLoadModule('/src/entry-server.tsx')`" pattern from Vite 4/5 guides is not available here; the
  supported route is a real SSR build.
- `react-dom@18.3.1` — `renderToString` (`react-dom/server`) and `hydrateRoot`
  (`react-dom/client`) both present. No new runtime dependency is needed.
- `nodeLinker: node-modules` in `welcome/.yarnrc.yml`, no `.pnp.cjs` — plain Node resolution works,
  so an SSR bundle that externalizes `react`/`react-dom`/`lucide-react` resolves them normally at
  prerender time.
- CI pins Node 20 (`actions/setup-node` with `node-version: '20'`); local is 22. Everything the
  prerender script needs (`node:fs`, top-level `await` in `.mjs`, dynamic `import()`, `rmSync`) is
  stable on 20.

`welcome/` has no test runner and no `scripts/` directory today.

## Design

### Pipeline

```
yarn build
  ├─ tsc                       type-check src/ (now including src/entry-server.tsx)
  ├─ vite build                → dist/            client bundle + index.html
  └─ node scripts/prerender.mjs
        ├─ vite build (ssr)    → dist-ssr/entry-server.mjs
        ├─ import render()     → markup string
        ├─ assert + inject     → dist/index.html   <div id="root">MARKUP</div>
        └─ rm -rf dist-ssr/
```

The SSR build is driven from inside the script via Vite's JS `build()` API rather than as a third
`&&`-chained CLI command, so all prerender concerns — build, render, assert, inject, clean up —
live in one file and `package.json` gains one short step.

### `src/entry-server.tsx` (new)

```tsx
import React from 'react'
import { renderToString } from 'react-dom/server'
import App from './App'

export function render(): string {
  return renderToString(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  )
}
```

`renderToString`, not `renderToStaticMarkup` — the latter is explicitly not intended for markup that
will be hydrated. `StrictMode` emits no DOM, so wrapping here is about staying symmetric with
`main.tsx`, not about the output.

The file deliberately does **not** import `styles/globals.css`; only `main.tsx` does, keeping the
SSR module graph free of CSS.

### `scripts/prerender.mjs` (new)

Plain ESM (`welcome/package.json` is `"type": "module"`), outside `src/` so `tsc` ignores it.

1. Resolve the project root from `import.meta.url`, so the script works from any cwd.
2. `await build({ root, logLevel: 'warn', build: { ssr: 'src/entry-server.tsx', outDir: 'dist-ssr',
   emptyOutDir: true, rollupOptions: { output: { entryFileNames: 'entry-server.mjs' } } } })`.
   `entryFileNames` is pinned so step 3 does not have to guess the emitted filename.
3. `const { render } = await import(pathToFileURL(join(root, 'dist-ssr/entry-server.mjs')))`, then
   `const markup = render()`.
4. Read `dist/index.html`. If it does not contain exactly one `<div id="root"></div>`, throw —
   the injection point moved and silently producing an unprerendered page is the failure this whole
   change exists to prevent.
5. Replace that placeholder with `` `<div id="root">${markup}</div>` `` — exact string substitution,
   no added whitespace, so hydration sees no stray text nodes.
6. Assert the result: strip tags (`/<[^>]*>/g`) from the final HTML and require at least
   **100 words**. The page's real copy is ~400 words, so this is a "did anything render at all"
   floor, not a content contract — it will not trip on copy edits, and it does trip if `render()`
   ever returns an empty or near-empty string.
7. Write `dist/index.html`, then `rmSync(ssrOutDir, { recursive: true, force: true })`.

Any throw exits non-zero and fails `yarn build`, and therefore the *Build welcome page* CI step.

### `src/main.tsx` — hydrate when there is markup

```tsx
const container = document.getElementById('root')!
const app = (
  <React.StrictMode>
    <App />
  </React.StrictMode>
)

// `vite dev` serves the unprerendered shell, so the container is empty there.
if (container.firstChild) {
  hydrateRoot(container, app)
} else {
  createRoot(container).render(app)
}
```

Branching on whether the container actually has children — rather than on `import.meta.env.DEV` —
keeps `yarn dev` working (no hydration-mismatch error against an empty root) and also degrades
sanely if a prerender ever fails, without the mode flag having to stay in sync with what the build
really produced. The build-time assertion is what keeps a failed prerender from reaching production
unnoticed, so this branch is a fallback, not the guard.

### The no-JS fade-in fix

Give `FadeIn`'s wrapper a stable, non-Tailwind marker class and override it from a `<noscript>`
block in the static shell.

`App.tsx`:

```tsx
className="fade-in opacity-0 translate-y-8 transition-all duration-700 ease-out"
```

`index.html`, in `<head>` (`<noscript>` in head may contain `<style>`, which is valid HTML):

```html
<!-- App.tsx's FadeIn keeps its children at opacity-0 until IntersectionObserver fires. -->
<noscript><style>.fade-in{opacity:1!important;transform:none!important}</style></noscript>
```

`!important` is needed to beat Tailwind's `opacity-0`/`translate-y-8` utilities; `transform:none`
overrides Tailwind v3's `--tw-translate-y`-based transform. `.fade-in` is not a utility, so Tailwind
generates nothing for it and strips nothing. The rule is inert whenever JS is enabled, so there is
no hydration or visual effect on the normal path. The coupling between the class name in `App.tsx`
and the rule in `index.html` is the one non-obvious thing here, which is what the comment records.

### Config and lint housekeeping

- `welcome/.gitignore`: add `dist-ssr` (the script removes it on success; this covers a failed run).
- `welcome/.eslintrc.json`: add `dist-ssr` to `ignorePatterns`, and an `overrides` entry giving
  `scripts/**/*.mjs` `env: { node: true, browser: false }` — the root config is browser-only, so
  `process`/`console` would otherwise trip `no-undef`. If `yarn lint` reports
  `react-refresh/only-export-components` on `src/entry-server.tsx` (it exports a non-component
  function), disable that rule for that file in the same `overrides` block.
- `welcome/package.json`: `"build": "tsc && vite build && node scripts/prerender.mjs"`.

## Implementation Steps

1. **`welcome/src/entry-server.tsx`** — new file, as above.
2. **`welcome/scripts/prerender.mjs`** — new file implementing steps 1-7 of the Design section.
3. **`welcome/package.json`** — append `&& node scripts/prerender.mjs` to `"build"`.
4. **`welcome/src/main.tsx`** — import `hydrateRoot` and `createRoot` from `react-dom/client`;
   replace the unconditional `createRoot(...).render(...)` with the `container.firstChild` branch.
5. **`welcome/src/App.tsx`** — prepend `fade-in ` to `FadeIn`'s wrapper `className`.
6. **`welcome/index.html`** — add the `<noscript><style>` block to `<head>`, with its comment.
7. **`welcome/.gitignore`** — add `dist-ssr`.
8. **`welcome/.eslintrc.json`** — add `dist-ssr` to `ignorePatterns`; add the `scripts/**/*.mjs`
   node-env override (and the `entry-server.tsx` react-refresh override if `yarn lint` asks for it).
9. **Verify** — run `yarn build` in `welcome/`, confirm `dist-ssr/` is gone afterward and
   `dist/index.html` strips to several hundred words; run `yarn lint`; then work the
   **Manual Verification** list.
10. **`CHANGELOG.md`** — add the `## Unreleased` entry described under Documentation.

If step 2's SSR build emits an output that fails to import — the residual risk, since Vite 8 builds
through rolldown and dependency externalization is the one behaviour not verified from the installed
packages alone — the fallback is `ssr: { noExternal: true }` in the `build()` options, bundling
`react`, `react-dom`, and `lucide-react` into the SSR chunk. That is acceptable here precisely
because the SSR bundle is a throwaway artifact of a separate process: nothing ships it, and the
duplicated React never coexists with the client's.

## Files to Modify

| File | Change |
|---|---|
| `welcome/src/entry-server.tsx` | new — `render()` via `renderToString` |
| `welcome/scripts/prerender.mjs` | new — SSR build, render, assert, inject, clean up |
| `welcome/package.json` | `build` script gains the prerender step |
| `welcome/src/main.tsx` | `createRoot` → `hydrateRoot` when the root has markup |
| `welcome/src/App.tsx` | `fade-in` marker class on `FadeIn`'s wrapper |
| `welcome/index.html` | `<noscript>` style override in `<head>` |
| `welcome/.gitignore` | ignore `dist-ssr` |
| `welcome/.eslintrc.json` | ignore `dist-ssr`; node env for `scripts/**/*.mjs` |
| `CHANGELOG.md` | `## Unreleased` entry |

No change to `.github/workflows/publish-docs.yml` — it already calls `yarn build`.

## Trade-offs

- **Prerender script vs. an SSG framework.** `vite-react-ssg` or Vike would do this, but both bring
  a routing/data model and a build pipeline this one static, routerless page has no use for, plus a
  dependency to keep current. ~60 lines of Node against `react-dom/server`, which is already a
  dependency, is the smaller permanent cost.
- **Prerender vs. hand-writing the copy into `index.html`.** Duplicating ~400 words across the JSX
  and the shell would drift on the first copy edit, and nothing would catch it.
- **SSR build vs. `ssrLoadModule`.** Not a real choice on this toolchain: Vite 8 no longer exports
  `ssrLoadModule` (verified against the installed `vite@8.0.16` node entry). Its replacement,
  `createServerModuleRunner`, means running a dev server during a production build, for no benefit
  over a real SSR build.
- **`entry-server.tsx` vs. rendering from the `.mjs` directly.** The script could SSR-build
  `src/App.tsx` and call `renderToString(React.createElement(...))` itself, avoiding a new `src/`
  file and the react-refresh lint wrinkle. Keeping the JSX in a TSX file that `tsc` type-checks, in
  the shape Vite's own SSR docs use, is worth one lint override.
- **`container.firstChild` vs. `import.meta.env.DEV`** for choosing hydrate-vs-render. The env flag
  states the intent more directly; the DOM check is a fact about what is actually there, so it
  cannot disagree with what the build produced. Chosen for that reason.
- **`<noscript>` override vs. a `.js`-class on `<html>`.** The classic progressive-enhancement
  alternative — an inline script stamping `.js` on `<html>`, with the hidden state defined as
  `.js .fade-in { ... }` — needs no `!important` and no `<noscript>`, but moves `FadeIn`'s styling
  out of Tailwind utilities and into hand-written CSS in `globals.css`. The `<noscript>` block keeps
  the component's styling where it is, at the cost of two `!important`s in a rule that only ever
  applies when JS is off.

## Decisions

- No test framework is added to `welcome/`. The build-time assertion in `prerender.mjs` is the
  automated regression guard and runs in CI already; see Testing Strategy.
- The word floor is 100 against ~400 words of real copy — a smoke floor, deliberately not a
  content contract.
- `sitemap.xml` and the `micromegas.info` / `madesroches.github.io` host split, raised at the end of
  the issue as "Related, separate", stay out of scope.

## Documentation

No user- or contributor-facing documentation covers the `welcome/` build today
(`mkdocs/docs/contributing.md` does not mention it), and the prerender step is visible in
`welcome/package.json`'s `build` script, so no docs page needs to change.

`CHANGELOG.md` gets one `## Unreleased` entry under a **Website:** heading: the landing page is now
prerendered at build time and hydrated on the client, so the served HTML carries the full copy
(~400 words, previously 0) for crawlers that do not execute JavaScript; `welcome`'s `build` script
gains a prerender step that fails the build if the markup is not injected; and the page now renders
readably with JavaScript disabled.

## Testing Strategy

**Automated:** `scripts/prerender.mjs`'s own assertions — the `<div id="root"></div>` placeholder
must be found exactly once, and the finished `dist/index.html` must strip to ≥ 100 words. These run
on every `yarn build`, which means on every push to `main` and every PR touching `welcome/**` via
the *Build welcome page* step in `publish-docs.yml`. A non-zero exit fails that step and blocks the
deploy.

This is the check that matters, because the failure mode is silent: if the SSR render ever returned
an empty string or the placeholder were renamed, `vite build` would still succeed, the deploy would
still go out, and the page would quietly revert to exactly the state this issue reports — visible
only to a crawler, weeks later.

**No vitest in `welcome/`.** The only genuinely unit-testable piece is the placeholder-replacement
and word-count logic, and it is already executed against the real `dist/index.html` on every build,
by the assertion above. Adding a test runner, a config, and CI wiring to `welcome/` to cover ~20
lines that CI already exercises end-to-end would cost more than it catches.

## Manual Verification

1. `cd welcome && yarn build`
   - Expected: exits 0; `welcome/dist-ssr/` does not exist afterward.
   - `sed 's/<[^>]*>//g' dist/index.html | wc -w` → several hundred, not 0. (This mirrors the
     issue's acceptance criterion; the build already asserts a floor, so this is confirmation.)
   - `grep -c 'id="root"><' dist/index.html` → `0` (the placeholder is no longer empty).
2. `yarn preview`, open the page, open devtools console.
   - Expected: the page looks and behaves exactly as before, sections fade in on scroll, and the
     console shows **no** hydration warning or `Text content did not match` error.
   - Not automated: catching a hydration mismatch needs a real browser running React's dev build,
     and any mismatch shows up the first time anyone opens the page.
3. In the same preview, disable JavaScript (devtools → Settings → Debugger → Disable JavaScript) and
   reload.
   - Expected: all sections are readable and fully opaque — in particular `HowItWorks`,
     `Differentiators`, `Notebooks`, `Integrations`, and `Footer`, the five wrapped in `FadeIn`.
   - Not automated: this is a computed-style outcome of a `<noscript>` block, which only a real
     browser with scripting off evaluates.
4. `yarn dev`, open the page.
   - Expected: renders normally with no console error — confirms the `container.firstChild` fallback
     takes the `createRoot` branch against the unprerendered dev shell.
   - Not automated: same reason as step 2, and breakage would be immediate for anyone running `dev`.
5. `yarn lint` → clean.
