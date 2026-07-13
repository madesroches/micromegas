# Docker-based "Getting Started" Quickstart Plan

## Overview

Replace the current developer-oriented "Getting Started" page with a simple,
build-free Docker quickstart that stands up a working single-node Micromegas
instance in a few commands, then open the web app and run a sample query. Move
the existing build-from-source / dev-environment instructions into the
Development docs (`development/build.md` + `contributing.md`) where they belong.

Resolves [#1273](https://github.com/madesroches/micromegas/issues/1273).

## Current State

- **`mkdocs/docs/getting-started.md`** is really a *developer* setup: it requires
  cloning the repo, exporting DB/env vars, installing a Rust toolchain + C/C++
  build tools (mold/clang/VS Build Tools), Python, then building and launching
  services via `local_test_env/ai_scripts/start_services.py --monolith` (or split
  / manual `cargo run` startup). It ends with `pip install micromegas` + a sample
  query and troubleshooting.
- **`mkdocs/docs/development/build.md`** already documents building from source
  (Rust, JS/TS, Python, docs, ARM64 cross-compile, self-hosted CI runner). It
  does **not** cover the `start_services.py` dev-run flow or the local env vars.
- **`mkdocs/docs/contributing.md`** line 38: "Development Setup" just links back
  to `getting-started.md`. This is the reference that must be redirected.
- **Root `/CONTRIBUTING.md`** line 38 has the identical "Development Setup" →
  `https://micromegas.info/docs/getting-started/` link. It is a separate file
  from `mkdocs/docs/contributing.md` (not a symlink/copy), so it needs the same
  redirect.
- **`docker/docker-compose.monolith.yaml`** already does exactly what the
  quickstart needs: Postgres + `marcantoinedesroches/micromegas-monolith:latest`,
  file-backed object store (`file:///data`), `--disable-auth`, web app on `:3000`,
  ingestion on `:9000`, FlightSQL on `:50051`. **Caveat:** it bind-mounts
  `./init-databases.sh` (which creates the `micromegas_app` DB required by
  `MICROMEGAS_APP_SQL_CONNECTION_STRING`). That relative-path mount means the
  file is **not** clone-free as written — a `curl`-ed copy would break without
  the second file.
- **`docker/README.md`** documents the monolith compose + `docker run` forms and
  a full env-var reference (used by maintainers, not newcomers).
- **`mkdocs/docs/admin/monolith.md`** has a "Quick start with Docker Compose"
  section (`docker compose -f docker-compose.monolith.yaml up`) plus binary/direct
  run — this is the production/admin angle and stays.
- **Nav (`mkdocs/mkdocs.yml`)**: top-level `Getting Started: getting-started.md`;
  `development/build.md` and `contributing.md` already live under
  Operations → Development.
- **Entry-point links** to fix:
  - `README.md` line 74 (Getting Started section, plain markdown link — no
    button styling).
  - `mkdocs/docs/index.md` lines 12 (`→ Get Started` button) and 50 (Quick Start
    list) — both point to `getting-started.md` (fine to keep, content changes).
  - `mkdocs/docs/contributing.md` line 38 → should point to `development/build.md`.
  - Root `/CONTRIBUTING.md` line 38 → same fix, same target URL.
  - `mkdocs/docs/development/build.md` "Next Steps" links back to
    `getting-started.md` as "Set up a development instance" — reword.

## Design

### 1. Keep the `getting-started.md` URL as the newcomer entry point

The published URL `https://micromegas.info/docs/getting-started/` is already
advertised from the README and the site. Rather than mint a new `/quickstart/`
URL and add redirects, **rewrite `getting-started.md` in place** as the Docker
quickstart. This keeps every existing inbound link valid and makes the newcomer
path the default with zero nav churn.

New `getting-started.md` outline:

```
# Getting Started
(one-line: try Micromegas locally with Docker — no build, no dev setup)

## Prerequisites
- Docker (with Compose v2)

## 1. Start Micromegas
   - Option A (clone-free): curl the self-contained compose file, `docker compose up`
   - Option B (repo cloned): `docker compose -f docker/docker-compose.monolith.yaml up`
   - Inline the compose YAML in a collapsible block so users can paste it

## 2. Open the web app
   - http://localhost:3000  (+ what they'll see)

## 3. (Optional) Run a query from Python
   - pip install micromegas ; the existing sample query script
     (`micromegas.connect()` needs no configuration locally — it defaults to
     `grpc://localhost:50051`, which matches the compose FlightSQL port;
     optionally mention `MICROMEGAS_ANALYTICS_URI` as the CLI-only
     query-endpoint override)
   - The sample query returns Micromegas's own self-telemetry — the compose
     file's monolith ingests its own traces/logs by default — so newcomers
     should expect to see real rows, not an empty table; it's empty until the
     first sink flush (~5s) is ingested AND the maintenance role materializes
     it (another ~1-2s) — after that expect real rows

## What you just ran  (evaluation-only callout)
   - --disable-auth, file object store, single process, ephemeral volumes
   - "Not for production" admonition → links to auth + monolith/scaling docs

## Stopping / cleaning up
   - docker compose down  (and `down -v` to wipe data)

## Next Steps
   - Query Guide, Architecture, Unreal, instrument your app
   - "Building from source / contributing?" → development/build.md

## Troubleshooting
   - port conflicts (3000/9000/5432), image pull, expect to see Micromegas's
     own self-telemetry in the sample query (empty until the first sink flush
     ~5s is ingested AND the maintenance role materializes it ~1-2s more)
```

### 2. Make the compose file genuinely clone-free (single source of truth)

Convert `docker/docker-compose.monolith.yaml` to use an **inline Compose
`configs`** block for the DB-init SQL instead of bind-mounting
`./init-databases.sh`. This removes the external-file dependency, so the exact
same file works whether it's cloned, `curl`-ed, or copy-pasted — one source of
truth for both in-repo use and the doc.

```yaml
services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_USER: micromegas
      POSTGRES_PASSWORD: micromegas
      POSTGRES_DB: micromegas
    configs:
      - source: pg_init
        target: /docker-entrypoint-initdb.d/init-databases.sql
    volumes:
      - pgdata:/var/lib/postgresql/data
    healthcheck: { ... unchanged ... }
  micromegas:
    # ... unchanged ...

configs:
  pg_init:
    content: |
      CREATE DATABASE micromegas_app;

volumes:
  pgdata:
  lake:
```

Notes:
- Postgres auto-runs `*.sql` files in `/docker-entrypoint-initdb.d/`, so the
  init logic is preserved without a shell script.
- Inline `configs.content` requires Docker Compose v2.23.1+ (released Nov 2023;
  bundled with current Docker Desktop / recent Docker Engine). Add a prerequisite
  note stating a recent Docker/Compose is required.
- The doc's clone-free `curl` command points at the raw URL of this file on
  `main`; the inline block in the doc is the same content.
- Once the script is no longer referenced by the compose file, `docker/init-databases.sh`
  becomes a dead, drifting second copy of the init SQL. **Resolved:** remove it —
  a repo grep confirmed the compose file was its only reference. The script has
  been removed; the init SQL now lives inline in the compose `configs.pg_init`
  block (see Resolved Decisions #2).

The compose file remains referenced by `docker/README.md` and
`admin/monolith.md`. The inline `configs.content` floor (Compose v2.23.1+)
applies to every invocation of the shared file, so both docs must gain a
one-line "requires Docker Compose v2.23.1+" prerequisite note in their
quick-start sections — they are not unaffected by this edit.

### 3. Move dev/build-from-source content into Development docs

- **`development/build.md`**: add a "Running a Development Instance" section
  covering what only `getting-started.md` documented today and `build.md` lacks:
  - the local env vars block (`MICROMEGAS_DB_USERNAME/PASSWD`,
    `MICROMEGAS_TELEMETRY_URL`, `MICROMEGAS_SQL_CONNECTION_STRING`,
    `MICROMEGAS_OBJECT_STORE_URI`) with the object-storage-path tip and the
    transport-tuning note;
  - `start_services.py --monolith` (Option A), split services (Option B), and
    manual per-service `cargo run` startup (Option C), plus `stop_services.py`;
  - the "Service Roles" info box.
  - Do **not** duplicate build-tool prerequisites — `build.md` already has them.
- **`contributing.md`** line 38: change the "Development Setup" link from
  `getting-started.md` to `development/build.md` (and mention the new dev-instance
  section there).
- **Root `/CONTRIBUTING.md`** line 38: same "Development Setup" link repoint, to
  `https://micromegas.info/docs/development/build/` (this file is separate from
  `mkdocs/docs/contributing.md`, not a copy/symlink of it).
- Cross-link: `build.md` "Next Steps" reference to `getting-started.md` is
  reworded (it's no longer a dev instance) or dropped.

### 4. Update entry-point links

- **`README.md`** "Getting Started" section (line 72–74): reframe as the Docker
  quickstart ("Run Micromegas locally with Docker in a couple of commands — see
  [Getting Started]"), and add a contributor pointer to the build/dev docs
  (`https://micromegas.info/docs/development/build/`). Keep the getting-started
  URL.
- **`mkdocs/docs/index.md`**: keep the `getting-started.md` targets (button +
  Quick Start list); optionally tweak the list label to "Run locally with Docker".
- **`mkdocs/mkdocs.yml`**: nav largely unchanged since the URL is reused.
  Optionally rename the nav label `Getting Started` (no path change). No new page
  to register.

## Implementation Steps

### Phase 1 — Make the compose file clone-free
1. Edit `docker/docker-compose.monolith.yaml`: replace the `init-databases.sh`
   bind-mount with an inline `configs:` block (as above). Verify locally:
   `docker compose -f docker/docker-compose.monolith.yaml up`, confirm
   `micromegas_app` DB is created and the web app serves on `:3000`.
2. Grep the repo for other references to `init-databases.sh`
   (`git grep init-databases`) and remove `docker/init-databases.sh` — resolved
   decision, no longer referenced once the compose file's init SQL is inline
   (done, see Progress).

### Phase 2 — Rewrite the newcomer page
3. Rewrite `mkdocs/docs/getting-started.md` as the Docker quickstart (outline
   above): clone-free `curl` + `docker compose up`, in-repo option, inline
   collapsible compose YAML, open web app, optional `pip install micromegas`
   sample query, evaluation-only callout with links to
   `admin/authentication.md` and `admin/monolith.md`, stop/cleanup,
   troubleshooting, and next steps (including a "building from source?" link to
   `development/build.md`).

### Phase 3 — Relocate dev instructions
4. Add "Running a Development Instance" to `mkdocs/docs/development/build.md`
   (env vars, `start_services.py` monolith/split/manual, `stop_services.py`,
   service roles) — content lifted from the old `getting-started.md`.
5. Update `mkdocs/docs/contributing.md` line 38 to link `development/build.md`.
   Also update root `/CONTRIBUTING.md` line 38 (same "Development Setup" link,
   same fix).
6. Reword/drop the `build.md` "Next Steps" link to `getting-started.md`.

### Phase 4 — Fix entry-point links + nav
7. Update `README.md` Getting Started section (newcomer quickstart + contributor
   build-docs pointer).
8. Update `mkdocs/docs/index.md` labels if desired (targets unchanged).
9. Adjust `mkdocs/mkdocs.yml` nav label if renaming (no path change).

### Phase 5 — Verify
10. `cd mkdocs && mkdocs build` (or `python build-docs.py`) — confirm no broken
    internal links and the page renders.
11. Manually walk the clone-free quickstart end to end on a clean machine/dir.

## Files to Modify

- `docker/docker-compose.monolith.yaml` — inline `configs` for DB init (clone-free)
- `docker/init-databases.sh` — removed (init SQL now inline in compose `configs.pg_init`)
- `mkdocs/docs/getting-started.md` — rewrite as Docker quickstart
- `mkdocs/docs/development/build.md` — add "Running a Development Instance"; fix Next Steps link
- `mkdocs/docs/contributing.md` — repoint "Development Setup" to build.md
- `CONTRIBUTING.md` (root) — repoint "Development Setup" to build.md (separate
  file, same fix)
- `mkdocs/docs/index.md` — optional label tweaks
- `mkdocs/mkdocs.yml` — optional nav label rename
- `README.md` — reframe Getting Started; add contributor build-docs link
- `docker/README.md` — add Docker Compose v2.23.1+ prerequisite note
- `mkdocs/docs/admin/monolith.md` — add Docker Compose v2.23.1+ prerequisite note

## Trade-offs

- **Reuse `getting-started.md` URL vs. new `/quickstart/` page.** Reusing keeps
  every inbound link valid, requires no redirects, and makes the simple path the
  default with minimal nav churn. A separate `quickstart.md` would preserve the
  old dev page's URL but leave a developer page sitting at the newcomer URL — the
  opposite of the issue's intent. Chosen: rewrite in place.
- **Inline `configs` vs. keeping the bind-mounted init script.** Inline `configs`
  makes the compose file self-contained (true clone-free `curl`), at the cost of
  requiring a reasonably recent Compose (v2.23.1+, ~2 years old) and one extra DB
  in the same file. Keeping the bind-mount would force the clone-free flow to
  fetch two files or clone — defeating the goal. Chosen: inline, with a prereq
  note.
- **Inline compose YAML in the doc vs. curl-only.** The issue asks for "both" —
  a `curl` command (least friction, always current) plus an inlined copy in a
  collapsible block (copy-paste, air-gapped, reviewable). Provide both.
- **Duplicating dev instructions vs. moving them.** Move, not copy: a single dev
  source (`build.md`) avoids drift. `getting-started.md` and `contributing.md`
  link to it. **Out of scope:** `doc/GETTING_STARTED.md` (~144 lines, a separate
  dev-setup guide outside the mkdocs site) is left as-is — it's not linked from
  the published docs or README, only from completed task plans under
  `tasks/completed/`, so it poses no broken-link risk here; a future cleanup
  could fold or remove it.

## Documentation

This task *is* documentation. Pages touched:
- `getting-started.md` (rewritten — primary deliverable)
- `development/build.md` (gains dev-instance run section)
- `contributing.md` (link fix)
- `index.md` (optional label tweak)
- `mkdocs.yml` (optional nav label)
- Repo `README.md` (entry links)
- `admin/monolith.md` (gains a Compose v2.23.1+ prerequisite note)
- `docker/README.md` (gains a Compose v2.23.1+ prerequisite note)

`admin/monolith.md` and `docker/README.md` stay as the production/admin and
maintainer references — they keep referencing the same compose file — but each
picks up a one-line "requires Docker Compose v2.23.1+" prerequisite note in its
quick-start section, since the inline `configs` edit raises the version floor
for every invocation of the shared file.

## Testing Strategy

- **Compose smoke test:** from a clean checkout, `docker compose -f
  docker/docker-compose.monolith.yaml up`; verify Postgres creates both
  `micromegas` and `micromegas_app`, the web app loads at `http://localhost:3000`,
  and `pip install micromegas` + the sample query connects and returns the
  monolith's own self-telemetry (empty only if run before both the first sink
  flush ~5s and the maintenance role's next materialization pass ~1-2s
  complete).
- **Clone-free test:** in an empty temp dir, `curl` the compose file from the
  branch raw URL and run it; verify identical behavior with no other files
  present.
- **Docs build:** `mkdocs build` passes with no broken-link/nav warnings; visually
  check the rewritten page and the new build.md section render and links resolve.
- **Link audit:** grep confirms no remaining doc/README link points at the old
  dev-oriented getting-started content for a "development setup".

## Resolved Decisions

1. **Image tag:** advertise `marcantoinedesroches/micromegas-monolith:latest` in
   the quickstart. Note the versioned-tag scheme (`…:<version>`) for reproducible
   pinning, but the newcomer path uses `:latest`.
2. **`docker/init-databases.sh`:** removed. Repo grep confirmed the compose file
   was its only reference; the init SQL now lives inline in the compose `configs`
   block.
3. **Compose version floor:** requiring Docker Compose v2.23.1+ (for inline
   `configs.content`) is accepted. Documented as a prerequisite in the quickstart.

## Progress

- **Phase 1 (clone-free compose) — done.** `docker/docker-compose.monolith.yaml`
  now uses an inline `configs:` block instead of the `./init-databases.sh`
  bind-mount; `docker/init-databases.sh` removed; `docker compose config -q`
  validates. Remaining: end-to-end `up` smoke test (Phase 5).
- Phases 2–5 (doc rewrite, dev-content relocation, entry-point links,
  verification) — pending.
