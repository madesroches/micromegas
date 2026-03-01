# Blog under micromegas.info Plan

## Overview

Add a blog to the Micromegas documentation site, served under the custom domain `micromegas.info`, to improve organic discoverability. The blog is powered by the MkDocs Material blog plugin and seeded with posts based on published LinkedIn content.

**Important**: Blog posts should use the original published content without additions. Longer-form posts can be written separately later.

## Implementation Steps

### Phase 1: Custom domain setup ✅

1. ✅ Fixed `site_url` in `mkdocs/mkdocs.yml`: `https://micromegas.dev` → `https://micromegas.info`
2. ✅ Added CNAME to CI/CD in `.github/workflows/publish-docs.yml`
3. ✅ Configured GitHub Pages custom domain via `gh api`
4. ✅ Configured DNS in Route 53: 4 A records + www CNAME
5. ✅ HTTPS enforced — Let's Encrypt cert (expires 2026-05-30)
6. ✅ Fixed welcome page asset paths (`base: '/micromegas/'` → `base: '/'`)
7. ✅ Old URL redirects — `madesroches.github.io/micromegas/` → `https://micromegas.info/` (301)

### Phase 2: Blog infrastructure ✅

8. ✅ Enabled blog and tags plugins in `mkdocs/mkdocs.yml`
9. ✅ Added Blog to nav + welcome page navbar
10. ✅ Created directory structure: `mkdocs/docs/blog/posts/`
11. ✅ Created `.authors.yml` at `mkdocs/docs/blog/.authors.yml`
12. ✅ Verified build

### Phase 3: Published posts

Source of truth: LinkedIn data exports at `~/posts/linkedin_export/` (Shares.csv for posts, Articles/ for articles)

#### Completed

| Date | Title | Status |
|------|-------|--------|
| 2026-02-15 | DataFusion WASM in the Browser | ✅ |
| 2025-09-10 | MTBF with Unified Telemetry | ✅ |
| 2026-01-08 | Unified Observability for Unreal Engine | ✅ |
| 2025-10-24 | Corporate Open Source Strategy | ✅ |
| 2026-02-25 | Bevy game + Micromegas instrumentation tutorial | ✅ |
| 2024-04-27 | How to record millions of events for pennies | ✅ |
| 2025-07-19 | Splunk cost comparison (24x difference) | ✅ |
| 2025-12-10 | OSAcon 2025 presentation recap | ✅ |
| 2024-08-22 | Lakehouse architecture (v0.1.7) | ✅ |
| 2025-01-16 | Doubling down on FlightSQL | ✅ |
| 2025-02-21 | One year of open source | skipped — no content |
| 2026-02-05 | Creative destruction in games industry | ✅ |

#### Release announcements — completed

| Date | Version | Notable feature | Status |
|------|---------|-----------------|--------|
| 2025-11-14 | v0.15.0 | Auth framework + OIDC | ✅ |
| 2025-10-23 | v0.14.0 | JSONB migration, SQL-Arrow NULL handling | ✅ |
| 2025-09-03 | v0.12.0 | Async span tracing, 20ns overhead | ✅ |
| 2024-10-04 | v0.2.0 | Unreal Engine observability | ✅ |

#### Future posts — shorter / narrative

| Date | Title |
|------|-------|
| 2024-10-30 | Coding in public / starting open source |
| 2024-10-23 | The FDAP stack for game tech |
| 2024-12-07 | Building a Grafana plugin |

#### Not Micromegas-specific (skip for now)

- 2026-02-17 — Gallant report (French, policy)
- 2021-11-24 — Sisyphe Libéré (personal)
- 2022-01-02 — À lire, ou pas (personal)

#### Unpublished drafts (available for future long-form posts)

- `~/posts/analytics-webapp/analytics-webapp.md` — notebooks vs dashboards
- `~/posts/creative-destruction/` — game industry layoffs (draft expands on the 2026-02-05 post)

### Phase 4: Cleanup (before merge to main) ✅

- ✅ Remove `doc` branch from CI triggers in `publish-docs.yml`
- ✅ Remove `doc` branch from deploy condition

## Commits so far

- `079ca5c` — configure micromegas.info custom domain (site_url + CNAME in CI)
- `d5c3df3` — temp: deploy from doc branch for domain testing
- `bdfade7` — fix welcome page asset paths for custom domain
- `ca4ff46` — add MkDocs Material blog plugin and author config
- `00532df` — add blog post: DataFusion WASM in the browser
- `75184b7` — add blog link to welcome page navbar
- `a01c43d` — fix tracing-in-WASM section tone
- `4de6db6` — credit DataFusion community for WASM support
- `c36ef82` — add blog post: MTBF with unified telemetry
- `661c7bb` — add blog post: notebooks vs dashboards (wrong content)
- `76fac26` — replace post 3 with actual published article on Unreal Engine observability
- `8505063` — add blog post: corporate open source strategy
