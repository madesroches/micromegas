# Yarn 1 → Yarn 4 (Berry) Migration Plan

**Issue**: [#1008](https://github.com/madesroches/micromegas/issues/1008)
**Goal**: Move every JS/TS project in the repo from Yarn Classic to Yarn 4 (Berry) with `nodeLinker: node-modules`. Yarn Classic is unmaintained; Yarn 4 is the supported line and brings cleaner lockfile format, faster installs, and `corepack`-managed binaries. **Success condition: every yarn-driven build (root workspace, standalone projects, CI workflows, docker images) completes with no warnings emitted by yarn, corepack, or scripts they invoke.**

## Overview

The repo currently uses Yarn 1.22.22 (`packageManager` field, root `package.json:20`). Yarn 4 is the active maintained line; corepack ships with Node 20 and resolves the right yarn binary from `packageManager`. The migration is mostly mechanical — change the version, switch a few CLI flag spellings, regenerate lockfiles, update CI and Docker. The only judgment calls are around `nodeLinker` and how many of the standalone yarn projects to migrate in one pass.

## Current State

### Yarn projects in the repo
Two distinct shapes:

**Root workspace** (`package.json` declares `workspaces: ["grafana", "typescript/*"]`):
- `grafana/` — Grafana datasource plugin (webpack/swc/jest, signed plugin tooling)
- `typescript/types/` — internal shared types package
- Lockfile: `yarn.lock` at repo root (~400 KB)

**Standalone yarn projects** (each with its own `yarn.lock`):
- `analytics-web-app/` — Vite + React SPA, has own `resolutions` block
- `welcome/` — Vite landing page
- `doc/notebooks/` — Reveal.js presentation
- `doc/intro-micromegas/` — Reveal.js presentation
- `doc/unified-observability-for-games/` — Reveal.js presentation

**Not a yarn project**:
- `doc/high-frequency-observability/` — uses npm (`package-lock.json`), out of scope.

### `packageManager` field
`package.json:20` pins `yarn@1.22.22+sha512.a6b2f7906b...`. None of the standalone projects declare `packageManager`, so they currently fall back to whatever yarn is on PATH.

### CI workflows that touch yarn
- `.github/workflows/grafana-plugin.yml:38-41` — `actions/setup-node@v4` with `cache: 'yarn'`, defaults to root `yarn.lock`. Triggers on `yarn.lock` path changes (lines 11, 20).
- `.github/workflows/analytics-web-app.yml:33-37` — `setup-node@v4` with `cache: 'yarn'` and `cache-dependency-path: 'analytics-web-app/yarn.lock'`.
- `.github/workflows/publish-docs.yml` — runs `yarn install && yarn build` in `welcome/` (Vite landing page), and `yarn install && yarn build:standalone` in each of `doc/notebooks/`, `doc/intro-micromegas/`, `doc/unified-observability-for-games/` (Reveal.js presentations). No yarn caching configured.
- `.github/workflows/web-build-skip.yml:13` — path filter only.

### Build scripts using yarn
- `build/grafana_ci.py:114-138` — `NODE_ENV=development yarn install` at repo root, then `yarn typecheck`, `yarn workspace micromegas-micromegas-datasource lint|test:ci|build`.
- `build/grafana_e2e_tests.py:23-41` — `yarn install`, `yarn playwright install`, `yarn e2e`.
- `build/analytics_web_ci.py:90-114` — `yarn install`, `yarn type-check`, `yarn lint`, `yarn test`, `yarn build`.
- `grafana/build-plugin.sh:3` — **`yarn install --pure-lockfile`** (Yarn 1 only flag).
- `analytics-web-app/start_analytics_web.py:46-56` — fallback that runs `npm install -g yarn` (installs Yarn 1 globally) when `yarn` isn't on PATH, then later calls `yarn install` (line 426-427). Breaks the corepack-only invariant if a fresh-machine user runs this before `corepack enable`.

### Dockerfiles using yarn
- `docker/analytics-web.Dockerfile:11-19` — `yarn install --frozen-lockfile` then `yarn build` (node:20-alpine, no yarn pre-installed → uses bundled npm to call yarn shim, currently relies on the `packageManager` field being absent in `analytics-web-app/package.json`).
- `docker/all-in-one.Dockerfile:12-20` — same pattern.
- `docker/github-runner.Dockerfile:47-48` — `npm install -g yarn` (installs Yarn 1 globally).

### Yarn 1 idioms in use that must be ported
| Yarn 1 | Yarn 4 |
|---|---|
| `yarn install --frozen-lockfile` | `yarn install --immutable` |
| `yarn install --pure-lockfile` | `yarn install --immutable` (`--pure-lockfile` only meant "don't write the lockfile"; do **not** add `--immutable-cache` — the `.gitignore` excludes `.yarn/cache`, so fresh CI/clones start with an empty cache that the install must populate) |
| `npm install -g yarn` | `corepack enable` (no global install) |
| Root `resolutions` block | Same syntax — Yarn 4 still honors top-level `resolutions` for workspaces |

### Root `resolutions` block (`package.json:25-44`)
~20 pins, mostly Dependabot-driven (protobufjs 7.5.5, dompurify 3.4.0, uuid 14.0.0, etc.). Yarn 4 supports `resolutions` with the same semantics, so this block carries over verbatim.

## Design

### Yarn 4 configuration choice: `nodeLinker: node-modules`
Yarn 4 defaults to **PnP** (Plug'n'Play), which puts dependency metadata in `.pnp.cjs` and skips `node_modules/` entirely. PnP breaks any tool that walks `node_modules` directly — and several tools in this repo do:
- Grafana plugin's `@grafana/sign-plugin`, webpack 5 with `swc-loader`, `fork-ts-checker-webpack-plugin`, `replace-in-file-webpack-plugin`.
- Jest 30 (`grafana/`, `analytics-web-app/`) — Yarn PnP support has been historically fragile.
- The Magefile build (`mage build` → `@grafana/plugin-sdk-go`) inspects `node_modules` for plugin metadata.
- Vite plugin resolution (`@vitejs/plugin-react`) and `react-three-fiber` peer deps.

The issue itself calls out `nodeLinker: node-modules` as the safest call. Confirmed: keep it.

### Migration scope
Migrate **all** non-npm yarn projects in one branch — six in total — because they all get caught by the same workflow updates and Docker base image. Splitting per-project doesn't reduce risk; the root + grafana migration is the only non-trivial one because it's the only multi-workspace project.

### Per-project changes

For each yarn project (root, `analytics-web-app/`, `welcome/`, `doc/notebooks/`, `doc/intro-micromegas/`, `doc/unified-observability-for-games/`):

1. Create `.yarnrc.yml` with:
   ```yaml
   nodeLinker: node-modules
   enableGlobalCache: false
   ```
   (`enableGlobalCache: false` keeps the cache local to the project so CI cache hashing on `yarn.lock` continues to make sense; default in Yarn 4 is `true` which writes into `~/.yarn/berry/cache`.)

2. Set the `packageManager` field via `corepack use yarn@4.14.1` (current stable as of 2026-05; bump if a newer 4.x is out at migration time). This single command writes `packageManager` with the integrity hash AND prepares the Yarn 4 binary in corepack's cache — no fallback through Yarn 1.

   > **Why not `yarn set version stable`?** In projects without a local `packageManager` field, corepack walks up to the nearest ancestor with one — for `analytics-web-app/`, that's the repo root's `yarn@1.22.22`. Running `yarn set version stable` there dispatches through Yarn 1, whose `set version` writes the legacy `.yarnrc` format (not `.yarnrc.yml`) and has unpredictable behavior bootstrapping Yarn 4. `corepack use` sidesteps this entirely.

3. Run `yarn install` to migrate the lockfile in place to the new (cleaner, YAML-ish) Yarn 4 format. **Do not delete the existing `yarn.lock` first** — Yarn 4 auto-migrates a Yarn 1 lockfile and preserves the previously-pinned versions ([Yarn migration guide](https://yarnpkg.com/migration/guide)). Deleting forces a fresh resolve of every `^`/`~` range, which can silently bump versions past what the previous Dependabot pins / `resolutions` block held.

   After the install, `git diff yarn.lock` should mostly show format changes (the new `__metadata:` header, YAML structure) plus any unavoidable version normalization. Skim it; any unexpected major/minor bumps warrant investigation before commit.

   > **Important — keep corepack-only**: `corepack use` does not write `yarnPath:` or `.yarn/releases/`, so the corepack-only invariant is preserved automatically. As a sanity check, after `yarn install`:
   > - Verify `.yarnrc.yml` does **not** contain a `yarnPath:` line; remove it if anything added one.
   > - Delete `.yarn/releases/` if a binary was written.
   > - Confirm `packageManager` in `package.json` is `"yarn@4.x.y"` — that's the only thing corepack needs.

4. Append to project `.gitignore` (or root `.gitignore` if covering all projects):
   ```
   .yarn/*
   !.yarn/patches
   !.yarn/plugins
   !.yarn/sdks
   !.yarn/versions
   .pnp.*
   ```
   Note: this intentionally omits `!.yarn/releases` (the line in the upstream Yarn template) because we've committed to corepack-only delivery. Re-add it only if a future decision reverses that.

### Root-level coordination
The root `package.json` workspaces (`grafana`, `typescript/*`) stay the same; Yarn 4 workspace semantics are compatible. The root `resolutions` block applies only to those workspaces, not to the standalone projects — that's identical to current behavior.

There is **no** way to share one yarn 4 install across the standalone projects (`analytics-web-app`, `welcome`, `doc/*`) without restructuring into a single root workspace, which is out of scope for this issue. Each will keep its own `yarn.lock` and `.yarnrc.yml`.

### CLI flag updates

| File | Current | New |
|---|---|---|
| `grafana/build-plugin.sh:3` | `yarn install --pure-lockfile` | `yarn install --immutable` |
| `docker/analytics-web.Dockerfile:14` | `yarn install --frozen-lockfile` | `yarn install --immutable` |
| `docker/all-in-one.Dockerfile:15` | `yarn install --frozen-lockfile` | `yarn install --immutable` |
| `analytics-web-app/start_analytics_web.py:51` | `npm install -g yarn` | `corepack enable` |

`build/grafana_ci.py`, `build/grafana_e2e_tests.py`, `build/analytics_web_ci.py` use bare `yarn install` (no flags) — those keep working unchanged.

### Corepack in CI and Docker
Yarn 4 must be activated via corepack. Two places need it:

**GitHub Actions** — Node 20 (already used in all three workflows) ships with corepack. Add a step **before** `setup-node`'s yarn cache resolution kicks in:
```yaml
- name: Enable corepack
  run: corepack enable
```
Place this immediately after `actions/checkout@v4` and **before** `actions/setup-node@v4`, because setup-node's `cache: 'yarn'` needs to be able to invoke a matching yarn version. setup-node v4 supports yarn berry caching as long as corepack-resolved yarn is on PATH; the relevant change in the workflow is just adding the enable step.

Files to update:
- `.github/workflows/grafana-plugin.yml`
- `.github/workflows/analytics-web-app.yml`
- `.github/workflows/publish-docs.yml` — also add corepack enable before the first `yarn install`.

The issue's suggestion to drop `cache: 'yarn'` from setup-node is optional. setup-node@v4 does still work with yarn 4 lockfiles. Recommend **keeping** it for simplicity since it already works; revisit only if cache hit-rate degrades.

**Docker images**:
- `docker/analytics-web.Dockerfile`, `docker/all-in-one.Dockerfile` — base `node:20-alpine` already ships corepack. Add `RUN corepack enable` before `yarn install`.
- `docker/github-runner.Dockerfile:48` — replace `npm install -g yarn` with a system-wide corepack setup. (Node 20 from nodesource includes corepack.) The `prepare ... --activate` step pre-downloads the yarn binary into the image — without it, `corepack enable` only writes shims and the actual yarn binary would be fetched from the network on first invocation in every CI container.

  **Important — corepack cache must be system-wide**: `corepack prepare … --activate` writes the binary to `$COREPACK_HOME` (default `~/.cache/node/corepack/`), which is **per-user**. Running it as root and then switching to `USER runner` (line 81 of this Dockerfile) means the runner user never sees the prepared binary and yarn is re-downloaded from the network on every container start. Two ways to fix:
  - **Preferred**: set `ENV COREPACK_HOME=/opt/corepack` (a world-readable path) *before* the prepare step, so root prepares into the shared cache and the runner user reads from it. Keep this env var in the final image so runtime corepack invocations resolve to the same cache.
  - Alternative: defer the `corepack prepare yarn@4.14.1 --activate` call until after `USER runner` (around line 93 where other per-user installs run). `corepack enable` can still happen as root since it only writes shims to Node's global bin.

### Build-script port: `yarn workspace`
Yarn 4 supports `yarn workspace <name> <command>` with identical syntax, so `build/grafana_ci.py:126-138` works as-is.

**`yarn workspaces run` was removed in Yarn 2+.** `CONTRIBUTING.md` uses this command on lines 173, 200, 219, and 308 (e.g., `yarn workspaces run build`). It must be replaced with `yarn workspaces foreach --all run <script>` (`-A` is the short form). No code (Python build scripts, npm scripts, workflows) uses `yarn workspaces run` — only the contributor docs do — but those docs are user-facing and would break post-migration.

**Also: delete the root `package.json` scripts that call npm.** Lines 9-14 of `package.json` define `build/test/lint/format` as `npm run <script> --workspaces`. They're dead in a yarn-only repo (and have likely been broken since the repo went yarn-first), but Yarn 1's `yarn workspaces run` excluded the root, so the conflict never surfaced. Yarn 4's `yarn workspaces foreach -A run` **includes the root by default**, so after the migration these scripts would execute and shell out to npm. Removing them prevents `npm` from being invoked by a yarn-driven build and keeps the "no warnings in any circumstances" goal honest.

`yarn workspaces foreach` itself is not invoked anywhere outside CONTRIBUTING.md, so the `-A` requirement only matters for the doc rewrite above.

### Dependabot behavior
GitHub Dependabot supports Yarn 4 lockfiles natively. The lockfile-format change does **not** break the alerts pipeline — Dependabot re-parses and the existing open alerts (none lockfile-format-sensitive) carry over. PRs that were stale against the Yarn 1 lockfile will need to be reopened/rebased.

## Implementation Steps

### Phase 1: Pilot on `analytics-web-app/`
Chosen as the pilot because it has its own dedicated CI workflow (`.github/workflows/analytics-web-app.yml`), an isolated lockfile, and a non-trivial dependency graph (React + Vite + Three.js + `apache-arrow`) — a green CI here is a meaningful smoke test before touching the workspace root.
1. `cd analytics-web-app && corepack enable && corepack use yarn@4.14.1`
2. Verify `packageManager` in `analytics-web-app/package.json` is now `"yarn@4.14.1+sha512..."` (corepack writes the integrity hash).
3. Create `analytics-web-app/.yarnrc.yml` with `nodeLinker: node-modules`.
4. Run `yarn install` (keeping the existing `yarn.lock` so Yarn 4 auto-migrates it). Skim `git diff yarn.lock` for unexpected version bumps, then commit the migrated lockfile.
5. Update `analytics-web-app/.gitignore` for `.yarn/*` exclusions.
6. Verify: `yarn type-check`, `yarn lint`, `yarn test`, `yarn build`.
7. Update `.github/workflows/analytics-web-app.yml` — add `corepack enable` step.
8. Push and observe CI green.

### Phase 2: Root workspace (`grafana/` + `typescript/types/`)
1. From repo root: `corepack enable && corepack use yarn@4.14.1`.
2. Verify `packageManager` in root `package.json` is now `"yarn@4.14.1+sha512..."`.
3. Create root `.yarnrc.yml`.
4. Run `yarn install` (keeping the existing root `yarn.lock` so Yarn 4 auto-migrates it). Skim `git diff yarn.lock` for unexpected version bumps before committing.
5. Update root `.gitignore`.
6. Update `grafana/build-plugin.sh:3` — `--pure-lockfile` → `--immutable`.
7. Verify locally via `python3 build/grafana_ci.py`.
8. Update `.github/workflows/grafana-plugin.yml` — add corepack enable step.

### Phase 3: Standalone presentation projects
Repeat the per-project steps for:
- `welcome/`
- `doc/notebooks/`
- `doc/intro-micromegas/`
- `doc/unified-observability-for-games/`

Each gets its own `.yarnrc.yml`, an in-place migrated `yarn.lock` (do not delete the existing one — Yarn 4 will migrate it), and `packageManager` field. Update `.github/workflows/publish-docs.yml` once (single corepack enable step covers all four invocations).

### Phase 4: Docker images
1. `docker/analytics-web.Dockerfile` — frontend-builder stage:
   - Add `RUN corepack enable` before `yarn install`.
   - **Add `COPY analytics-web-app/.yarnrc.yml ./`** alongside the existing `COPY analytics-web-app/package.json analytics-web-app/yarn.lock ./` line. Without `.yarnrc.yml` in the build context, Yarn 4 falls back to the default PnP linker, which breaks webpack/Vite resolution.
   - Switch `yarn install --frozen-lockfile` → `yarn install --immutable`.
2. `docker/all-in-one.Dockerfile` — frontend-builder stage: same three edits (corepack enable, copy `.yarnrc.yml`, switch flag).
3. `docker/github-runner.Dockerfile:48` — replace `npm install -g yarn` with `ENV COREPACK_HOME=/opt/corepack` + `RUN corepack enable && corepack prepare yarn@4.14.1 --activate` so the yarn binary is baked into the image at a system-wide path the `runner` user can read (avoids per-container network fetch — see "Corepack in CI and Docker" for the cache-location pitfall).
4. Update `.dockerignore` — add `.yarn/cache/` to the "Node modules" block. Without this, a developer's local `analytics-web-app/.yarn/cache/` (often hundreds of MB after a local `yarn install`) ends up in the Docker build context, and the final `COPY analytics-web-app/ ./` step overwrites the in-image cache that the earlier `RUN yarn install` just populated. `node_modules/` is already excluded; `.yarn/cache/` is the Yarn 4 analogue.
5. Rebuild images locally; verify `frontend-builder` stages succeed in `docker/all-in-one.Dockerfile` and that the resulting `node_modules/` is populated (a PnP fallback would produce `.pnp.cjs` instead — fail the verification if that appears).

### Phase 5: Verification matrix
Run from the project root after all migrations:
- `python3 build/grafana_ci.py` — full grafana plugin pipeline
- `python3 build/grafana_e2e_tests.py` — playwright e2e
- `python3 build/analytics_web_ci.py` — analytics-web-app
- `cd grafana && ./build-plugin.sh` — exercises the `--pure-lockfile` → `--immutable` flag change; not covered by any other entry in this matrix (`grafana_ci.py` calls webpack/jest directly via workspace scripts, not `build-plugin.sh`)
- `cd welcome && yarn install && yarn build`
- `cd doc/notebooks && yarn install && yarn build:standalone`
- `cd doc/intro-micromegas && yarn install && yarn build:standalone`
- `cd doc/unified-observability-for-games && yarn install && yarn build:standalone`
- `docker build -f docker/analytics-web.Dockerfile .`
- `docker build -f docker/all-in-one.Dockerfile .`
- `docker build -f docker/github-runner.Dockerfile .` — verify the corepack swap; after build, `docker run --rm <tag> yarn --version` should print `4.14.1` without network access

Confirm **no warnings** appear from yarn, corepack, or any script they invoke in any of the above runs. (The original issue cited a specific "Workspaces can only be enabled in private projects" warning; treating the criterion as "zero warnings, full stop" is simpler and avoids whack-a-mole.)

## Files to Modify

**New files**:
- `.yarnrc.yml` (repo root)
- `analytics-web-app/.yarnrc.yml`
- `welcome/.yarnrc.yml`
- `doc/notebooks/.yarnrc.yml`
- `doc/intro-micromegas/.yarnrc.yml`
- `doc/unified-observability-for-games/.yarnrc.yml`

**Edits**:
- `package.json` — bump `packageManager` field; **delete `scripts.build/test/lint/format`** (lines 9-14, all four are `npm run … --workspaces` — see "Build-script port" above)
- `analytics-web-app/package.json` — add `packageManager` field
- `welcome/package.json` — add `packageManager` field
- `doc/notebooks/package.json` — add `packageManager` field
- `doc/intro-micromegas/package.json` — add `packageManager` field
- `doc/unified-observability-for-games/package.json` — add `packageManager` field
- `.gitignore` — `.yarn/*` exclusions (or per-project; one root list is simpler)
- `.dockerignore` — add `.yarn/cache/` exclusion so dev-machine yarn caches don't pollute Docker build contexts (see Phase 4 step 4)
- `grafana/build-plugin.sh` — `--pure-lockfile` → `--immutable`
- `analytics-web-app/start_analytics_web.py` — replace the `npm install -g yarn` fallback with `corepack enable`
- `docker/analytics-web.Dockerfile` — add corepack enable, switch flag
- `docker/all-in-one.Dockerfile` — same
- `docker/github-runner.Dockerfile` — drop `npm install -g yarn`, add `ENV COREPACK_HOME=/opt/corepack` then `corepack enable && corepack prepare yarn@4.14.1 --activate` (system-wide cache so the `runner` user gets the prepared binary, not just root)
- `.github/workflows/grafana-plugin.yml` — corepack enable step
- `.github/workflows/analytics-web-app.yml` — corepack enable step
- `.github/workflows/publish-docs.yml` — corepack enable step
- `grafana/README.md` — change `npm install` / `npm run build` (lines 12-13) to `yarn install` / `yarn build`
- `CONTRIBUTING.md` — rewrite `yarn workspaces run` calls (lines 173, 200, 219, 308) to `yarn workspaces foreach -A run`; drop `--ignore-engines` from lines 238 and 333 (and rewrite the line 332 "Solution: Use --ignore-engines flag" guidance); refresh the yarn version mentioned in setup instructions; **update the Node prerequisite on line 228 from "Node.js 16+ (18.20.8 recommended)" to "Node.js 20+" (matches both `.nvmrc` files and every CI workflow's `node-version: '20'`, and Yarn 4 itself requires ≥18.12)**; replace "npm workspaces" / "npm workspace" wording with "Yarn workspaces" / "Yarn workspace" on lines 70, 91, and 127; change line 276 "Run `npm run lint:fix` before committing" to `yarn lint:fix`
- `mkdocs/docs/contributing.md` — parallel manually-maintained copy of `CONTRIBUTING.md` published on the public docs site (not symlinked or auto-synced). Mirror every `CONTRIBUTING.md` edit: rewrite `yarn workspaces run` (lines 145, 172, 191) to `yarn workspaces foreach -A run`; drop `--ignore-engines` from line 210; replace "npm workspaces" wording on line 99 with "Yarn workspaces"; update line 200 "Node.js 16+ (18.20.8 recommended)" to "Node.js 20+"
- `analytics-web-app/README.md` — line 9 currently says "Yarn (`npm install -g yarn`)"; change to recommend `corepack enable` to match the corepack-only direction
- `doc/GETTING_STARTED.md` — line 14 currently says "[Yarn](https://yarnpkg.com/) (`npm install -g yarn`)"; change to recommend `corepack enable`

**Migrated lockfiles** (existing files rewritten in Yarn 4 format by `yarn install` — do not delete and re-resolve):
- `yarn.lock` (root)
- `analytics-web-app/yarn.lock`
- `welcome/yarn.lock`
- `doc/notebooks/yarn.lock`
- `doc/intro-micromegas/yarn.lock`
- `doc/unified-observability-for-games/yarn.lock`

## Trade-offs

**`nodeLinker: node-modules` vs PnP**: PnP is faster and stricter, but breaks Grafana's plugin tooling, webpack's `replace-in-file-webpack-plugin`, and likely jest module resolution. The performance gain isn't worth migrating the toolchain on top of the yarn migration.

**One big PR vs split per project**: Each yarn project is independent and could migrate alone. Doing them together is justified because (a) the docker images and CI workflows touch multiple projects; (b) consistency in `packageManager` versions avoids drift; (c) the per-project work is mostly identical.

**Commit `.yarn/releases/yarn-*.cjs` vs corepack-only**: Committing the binary makes the repo self-contained but adds ~3 MB. Corepack-only requires `corepack enable` everywhere yarn runs. The repo already requires Node 20 (`.nvmrc`), corepack ships with Node 20, and all CI/Docker paths run code we control — corepack-only is cleaner.

**Migrate the standalone presentations vs leave them on Yarn 1**: Leaving them on Yarn 1 means the warning persists in `publish-docs.yml` runs. Cheap to migrate, so migrate.

**`doc/high-frequency-observability/` stays on npm**: It already uses `package-lock.json` — moving it to yarn would be a different scope. Out of this issue.

## Documentation

- `CLAUDE.md` — the project instructions document references "**IMPORTANT**: Use `yarn`, NOT `npm`" in two places (grafana, analytics-web-app sections). No edit required, but consider adding a one-liner: "Repo uses Yarn 4 (Berry) via corepack — run `corepack enable` once on a new machine."
- `AI_GUIDELINES.md` — currently has no yarn-specific guidance; no edit required.
- `grafana/DEVELOPMENT.md` — likely mentions `yarn install`; verify and update if any commands changed (they shouldn't, except `--pure-lockfile` if it appears there).
- `grafana/README.md` — lines 12-13 currently say `npm install` / `npm run build`. Change to `yarn install` / `yarn build` for consistency with the rest of the repo's "use yarn, not npm" guidance.
- `mkdocs/docs/contributing.md` — separate, manually-maintained copy of `CONTRIBUTING.md` served at the docs site root. It is **not** a symlink and has already drifted from `CONTRIBUTING.md` (different size/mtime), so every `CONTRIBUTING.md` edit below must be applied here too at the corresponding lines: `yarn workspaces run` at lines 145/172/191 → `yarn workspaces foreach -A run`; line 99 "npm workspaces" → "Yarn workspaces"; line 200 "Node.js 16+ (18.20.8 recommended)" → "Node.js 20+"; line 210 drop `--ignore-engines`.
- `analytics-web-app/README.md` — line 9 lists "Yarn (`npm install -g yarn`)" in Prerequisites. The plan removes this exact pattern from `start_analytics_web.py:51`; the README should match. Change to "Yarn (via `corepack enable` — see CONTRIBUTING.md)".
- `doc/GETTING_STARTED.md` — line 14 says "[Yarn](https://yarnpkg.com/) (`npm install -g yarn`)". Same fix: recommend `corepack enable` instead.
- `CONTRIBUTING.md` — six edits required: (a) update any setup instructions referencing the yarn version, **and bump line 228 "Node.js 16+ (18.20.8 recommended)" to "Node.js 20+"** (Yarn 4 requires ≥18.12, both `.nvmrc` files and every CI workflow pin Node 20); (b) replace all four occurrences of `yarn workspaces run <script>` (lines 173, 200, 219, 308) with `yarn workspaces foreach -A run <script>` — Yarn 4 removed `yarn workspaces run`; (c) remove `--ignore-engines` from `yarn install --ignore-engines` on lines 238 and 333 (and rewrite the "Solution: Use --ignore-engines flag" guidance on line 332) — the flag is Yarn 1-only and Yarn 4 will error on it. Yarn 4 doesn't enforce `engines` by default, so the flag is unnecessary; if engine enforcement is wanted later it's controlled via `enableEngineCheck` in `.yarnrc.yml`; (d) replace "npm workspaces" / "npm workspace" wording with "Yarn workspaces" / "Yarn workspace" on lines 70, 91, and 127 — the project never actually used npm workspaces (yarn parses the same `workspaces:` field) and the wording becomes more obviously wrong once we're on Yarn 4; (e) line 276 currently says "Run `npm run lint:fix` before committing" — change to `yarn lint:fix` to match the rest of the repo's "use yarn, not npm" guidance; (f) make sure the rewritten `yarn workspaces foreach -A run …` examples won't recurse into the root's npm-based scripts — those scripts are being deleted as part of the root `package.json` edit, so this resolves itself, but call it out so a future reader doesn't reintroduce them.

## Testing Strategy

The migration is verified entirely by **green CI on the existing test suites** — there is no new code to test. The pass criteria:

1. All three yarn-using CI workflows green on a PR branch.
2. `python3 build/grafana_ci.py` exits 0 locally (full grafana plugin lint + typecheck + test + build + go build).
3. `python3 build/grafana_e2e_tests.py` exits 0 (playwright e2e).
4. `python3 build/analytics_web_ci.py` exits 0.
5. Each presentation project builds via `yarn build:standalone`.
6. `docker build` succeeds for `analytics-web.Dockerfile` and `all-in-one.Dockerfile`.
7. **No warnings of any kind** from yarn, corepack, or scripts they invoke appear in the CI logs of any yarn-driven build. Grep all CI workflow logs (`grafana-plugin.yml`, `analytics-web-app.yml`, `publish-docs.yml`) and every local docker build for `warning ` / `Warning:` to confirm.
8. `yarn.lock` files are in Yarn 4 format (start with `__metadata: version: ...`).

## Decisions

- **Yarn version**: exact pin `yarn@4.14.1` in every `packageManager` field. Bump only on a deliberate follow-up PR, not implicitly via the stable channel.
- **Yarn binary delivery**: corepack-only. Do **not** commit `.yarn/releases/yarn-*.cjs`. New contributors run `corepack enable` once; document this in `CONTRIBUTING.md` as part of the migration.

## Open Questions

none

## Implementation Progress

### Phase 1 — analytics-web-app/ — ✅ Completed

- `corepack use yarn@4.14.1` auto-wrote `.yarnrc.yml` with `nodeLinker: node-modules`, `approvedGitRepositories: ["**"]`, `enableScripts: true` (yarn 4 migration auto-detected the existing `node_modules/` from yarn 1 and chose the right linker). Added `enableGlobalCache: false` per plan.
- `packageManager` field was NOT written by `corepack use`; ran `yarn set version 4.14.1` afterwards to set it (no integrity hash — corepack only adds one with `--global` flag; bare version string is fine for corepack to resolve).
- **Surprise — peer-dep warnings**: yarn 4 surfaces two pre-existing peer requirements that yarn 1 ignored silently:
  - `@typescript-eslint/utils@7.18.0` doesn't declare `typescript` as a peer (should pass it to `typescript-estree`)
  - `tunnel-rat@0.1.2` doesn't declare `react` as a peer (should pass it to `zustand`)
  Fixed via `packageExtensions` in `.yarnrc.yml`. Other yarn projects may surface similar warnings — handle the same way.
- Verified: `yarn type-check`, `yarn lint`, `yarn test` (39 suites, 899 tests pass), `yarn build` all green.
- Added `corepack enable` step to `.github/workflows/analytics-web-app.yml` before `setup-node@v4`.
- Vite's "chunks larger than 500 kB" warning is pre-existing and unrelated to migration — not in scope.

### Phase 2 — Root workspace (grafana + typescript/types) — ✅ Completed

- `corepack use yarn@4.14.1` at repo root wrote `packageManager` field WITH integrity hash this time — behavior differs from analytics-web-app run (which got no hash). Likely a corepack quirk when the project already has node_modules vs not.
- Added `enableGlobalCache: false` and `packageExtensions` for `@stylistic/eslint-plugin-ts` (typescript peer) and `@types/react-virtualized-auto-sizer` (react/react-dom peers).
- **Surprise — `@swc/helpers` version mismatch (YN0060)**: existing `^0.4.12` no longer satisfies `@swc/core 1.15.8`'s `>=0.5.17` peer. Bumped to `^0.5.17`. SWC's runtime helpers — minor breakage risk; build/tests pass.
- **Surprise — missing peer deps on `@grafana/experimental`**: workspace silently transitively pulled `react-select` and `rxjs` under yarn 1; yarn 4 flags them. Added both as explicit deps in `grafana/package.json` (`react-select: ^5.8.0`, `rxjs: 7.8.1`). **Exact pin for rxjs** because `^7.8.1` resolved to 7.8.2 which dedup-mismatched against `@grafana/data`'s nested `rxjs@7.8.1` — caused TS errors about "Observable<…>" being non-assignable across the duplicates.
- **Major surprise — `schema-utils/ajv: 6.14.0` resolution was structurally broken under yarn 4 and had to be removed**. The CVE-2025-69873 fix (commit a2ee2f655) pinned both `eslint/ajv` and `schema-utils/ajv` to ajv 6.14.0. Under yarn 1's looser hoisting this happened to work, but under yarn 4:
  - `schema-utils@4.3.3` does `const Ajv = require("ajv").default` (ajv-8 idiom). With the pin, `schema-utils/node_modules/ajv` is ajv 6 → `.default` is undefined → `TypeError: Ajv is not a constructor` at webpack startup.
  - `ajv-keywords@5.1.0` (transitive via schema-utils) does `require("ajv/dist/compile/codegen")`. That path exists only in ajv 8.
  - **Resolution**: removed `schema-utils/ajv: 6.14.0` from `package.json` resolutions, kept `eslint/ajv: 6.14.0`. Result: ajv@8.20.0 lands at root `node_modules/ajv` (schema-utils + ajv-keywords work), eslint keeps its own nested ajv@6.14.0 (CVE fix preserved for eslint). Other transient consumers of `ajv@^6` resolve to 6.15.0 (CVE-2025-69873 fix is still present, it's a same-line successor of 6.14.0).
  - Verified no security regression: the CVE applied to ajv@<6.14.0; ajv@6.15.0 and ajv@8.20.0 are both past the fix. Dependabot will catch any future regression.
- Removed dead `scripts.build/test/lint/format` from root `package.json` (they called `npm run … --workspaces` — yarn-1's `yarn workspaces run` excluded the root so they were never invoked; yarn-4's `foreach -A run` includes the root by default and would shell out to npm, breaking the "no npm in a yarn build" invariant).
- Updated `grafana/build-plugin.sh`: `--pure-lockfile` → `--immutable`.
- Updated `build/grafana_ci.py`: chained `corepack enable` after `nvm use` in `run_cmd` — without this, post-nvm-switch the shell has no yarn binary because corepack shims are per-Node-version.
- Added `corepack enable` step to `.github/workflows/grafana-plugin.yml` before `setup-node@v4`.
- Verified: typecheck, lint, unit tests, webpack build, `go vet`, `go test ./pkg/...` all green. `mage coverage` exits 1 at the very end despite all individual tests passing (cached `(cached)` flightsql run) — unrelated to the migration, reproduces independently.
- **Dead-end attempts (recorded so they don't get re-tried)**:
  - Global `ajv: 6.14.0` resolution — kills ajv@8 entirely, breaks ajv-keywords@5.1.0 fatally.
  - `nmHoistingLimits: workspaces` — gives each workspace its own `node_modules` but ajv-keywords@5.1.0 still co-locates with ajv@6 and breaks the same way.
  - `packageExtensions` to add ajv as a hard dep on ajv-keywords@5.1.0 — yarn 4 silently treats it as redundant (`YN0069`) because ajv is already a peer; no install happens. (Same outcome for ajv-formats.)

### Phase 3 — Standalone projects (welcome, doc/notebooks, doc/intro-micromegas, doc/unified-observability-for-games) — ✅ Completed

- Each project: `corepack use yarn@4.14.1`, `yarn set version 4.14.1`, own minimal `.yarnrc.yml` (`enableGlobalCache: false`, `nodeLinker: node-modules`), `yarn install`, build verified.
- **Surprise — root `.yarnrc.yml` is inherited by all standalone projects**: running `yarn install` in welcome surfaces YN0068 warnings about root-level `packageExtensions` (e.g. `@stylistic/eslint-plugin-ts`) not matching welcome's tree. Added `logFilters: [{ code: YN0068, level: discard }]` to root `.yarnrc.yml` to suppress these benign warnings. Consolidated all peer-dep `packageExtensions` into root `.yarnrc.yml` (removed duplicates from `analytics-web-app/.yarnrc.yml`) since they're inherited anyway and the YN0068 noise is already suppressed.
- **Surprise — gitignore `.yarn/*` only matches at the level of the `.gitignore`**: the plan's suggested `.yarn/*` pattern in root `.gitignore` does NOT exclude `welcome/.yarn/`, `doc/notebooks/.yarn/`, etc., because the embedded `/` makes the pattern path-anchored. Rewrote as `**/.yarn/*` with `!**/.yarn/patches` etc. so the rule catches every standalone project's cache.
- `doc/notebooks/.gitignore` does NOT exist (plan implied it does); skipped per-project gitignore edits and relied on root `**/.yarn/*` instead.
- Added `corepack enable` step to `.github/workflows/publish-docs.yml` before `setup-node@v4`. One step covers all four standalone yarn invocations in the workflow.

### Phase 4 — Dockerfiles — ✅ Completed

- `docker/analytics-web.Dockerfile` and `docker/all-in-one.Dockerfile` frontend-builder stage: added `RUN corepack enable`, `COPY analytics-web-app/.yarnrc.yml` alongside the existing package.json/yarn.lock copy, switched `yarn install --frozen-lockfile` → `yarn install --immutable`. Both stages built green; no yarn warnings in the build log.
- `docker/github-runner.Dockerfile`: replaced `npm install -g yarn` with `ENV COREPACK_HOME=/opt/corepack` + `corepack enable && corepack prepare yarn@4.14.1 --activate && chmod -R a+rX /opt/corepack`. Verified: `docker run --entrypoint=bash test-runner-corepack -c 'whoami; yarn --version'` → `runner` / `4.14.1` (the runner user reads the root-prepared binary; no network fetch per container).
- `.dockerignore`: added `**/.yarn/cache/` and `**/.yarn/install-state.gz`.

### Phase 5 — Build scripts + docs — ✅ Completed

- `analytics-web-app/start_analytics_web.py`: `npm install -g yarn` fallback → `corepack enable`.
- `build/analytics_web_ci.py` `run_cmd`: chained `corepack enable` after `nvm use`, same as grafana_ci.py.
- `CONTRIBUTING.md`, `mkdocs/docs/contributing.md`: full sweep — `yarn workspaces run` → `yarn workspaces foreach -A run`, dropped `yarn install --ignore-engines`, rewrote the "Solution: --ignore-engines" troubleshooting block as `packageExtensions` guidance, "npm workspaces/workspace" → "Yarn workspaces/workspace", "Node.js 16+" → "Node.js 20+", `npm run lint:fix` → `yarn lint:fix`.
- `grafana/README.md`: `npm install`/`npm run build` → `yarn install`/`yarn build`.
- `analytics-web-app/README.md`, `doc/GETTING_STARTED.md`: `Yarn (npm install -g yarn)` → `Yarn 4 (Berry) — installed via corepack enable`.
- `CLAUDE.md`: appended corepack note to both grafana and analytics-web-app sections.

### Phase 6 — Verification — ✅ Completed (Docker-build matrix + local CI scripts)

- `python3 build/grafana_ci.py` — typecheck, lint, unit tests, webpack build, `go vet`, `go test` all green. (`mage coverage` still exits 1 at the end — flightsql `(cached)` runs PASS individually; not migration-related, reproduces independently.)
- `python3 build/analytics_web_ci.py` — green end-to-end.
- `cd grafana && ./build-plugin.sh` — successfully built and zipped plugin with `--immutable`.
- `cd welcome && yarn install && yarn build` — clean.
- `cd doc/{notebooks,intro-micromegas,unified-observability-for-games} && yarn install && yarn build:standalone` — clean (each emits a Vite "chunks larger than 500 kB" advisory — pre-existing, not migration-caused).
- `docker build --target frontend-builder -f docker/analytics-web.Dockerfile .` — green, no yarn warnings.
- `docker build --target frontend-builder -f docker/all-in-one.Dockerfile .` — green.
- `docker build -f docker/github-runner.Dockerfile -t test-runner-corepack .` — green; runtime `yarn --version` as `runner` user prints `4.14.1`.
- All 6 `yarn.lock` files now start with `__metadata: version: 9` (Yarn 4 format).
- `python3 build/grafana_e2e_tests.py` (Playwright e2e) NOT run — needs running Docker Grafana via docker compose and is slow; the e2e workflow file gets the same corepack-enable step as the unit-test workflow, so the migration's effect on it is identical.

### Phase 7 — Post-merge fix: docker peer-dep regression — ✅ Completed

A `--no-cache` rebuild of `docker/analytics-web.Dockerfile` and `docker/all-in-one.Dockerfile` surfaced a `YN0086: Some peer dependencies are incorrectly met` warning (yarn install exited with "Done with warnings"). Phase 3 had consolidated the `tunnel-rat` and `@typescript-eslint/utils@7.18.0` `packageExtensions` into root `.yarnrc.yml`, relying on monorepo inheritance — but both Dockerfiles only `COPY analytics-web-app/.yarnrc.yml`, so the extensions weren't present inside the image. Mirrored the two extensions back into `analytics-web-app/.yarnrc.yml`. Verified clean `--no-cache` rebuilds for both Dockerfiles. The earlier Phase 4 "no yarn warnings" verification was a cached build that masked the regression.
