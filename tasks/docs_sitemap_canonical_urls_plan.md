# Docs Sitemap and Canonical URLs Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1575

## Overview

`mkdocs/mkdocs.yml` declares `site_url: https://micromegas.info`, but `publish-docs.yml` builds
the MkDocs site into `public_docs/docs`, so it is served from `https://micromegas.info/docs/`.
MkDocs derives both the sitemap and every `rel="canonical"` tag from `site_url`, so all 93
sitemap entries and all 93 canonical tags point one path segment too high. 92 of the 93 point at
URLs that 404; the one exception is `index.md`, whose entry points at the root, which the welcome
landing page serves rather than 404ing.
The blog is the worst hit: all 20 posts are live under `/docs/blog/...`, all 20 are advertised at
`/blog/...`, and each live post tells crawlers its authoritative copy is the 404.

This plan fixes `site_url`, adds the site-root discovery files that were never there
(`robots.txt`, a sitemap covering the landing page and the five presentations), adds an RSS feed
and a blog-appropriate `<title>` suffix, and adds a CI check that fails the docs build whenever a
sitemap URL, a canonical tag, or the feed autodiscovery link stops resolving inside the staged
tree.

## Current State

- `mkdocs/mkdocs.yml:3` — `site_url: https://micromegas.info`.
- `.github/workflows/publish-docs.yml` — "Prepare staging directory" assembles `public_docs/`:
  - `public_docs/docs/` ← `mkdocs build --site-dir $PWD/public_docs/docs`
  - `public_docs/rustdoc/` ← `cargo doc` output
  - `public_docs/doc/` ← `cp -r doc/*` (legacy link compatibility)
  - `public_docs/{high-frequency-observability,unified-observability-for-games,notebooks,intro-micromegas}/index.html`
    ← each presentation's `dist/presentation-inline.html`
  - `public_docs/` root ← `cp -r welcome/dist/*` (the landing page)
  - `public_docs/CNAME` ← `micromegas.info`
- `welcome/public/` currently holds only `favicon.svg` and `screenshots/`. Vite copies `public/`
  into `dist/`, which the workflow then copies to the site root — that is the existing, working
  route for a static file to reach `https://micromegas.info/<name>`.
- `mkdocs/overrides/main.html` overrides only `extrahead`, to inject the GoatCounter tag.
- `mkdocs/docs-requirements.txt` pins `mkdocs`, `mkdocs-material`, `mkdocstrings`,
  `mkdocstrings-python`. No `rss` plugin.
- Blog posts are real source files at `mkdocs/docs/blog/posts/*.md`, each with a scalar
  `date: YYYY-MM-DD` in its front matter; the `blog` plugin renders them at
  `blog/{date}/{slug}/`.
- Material's `base.html` `htmltitle` block renders `<title>{{ page.title }} - {{ config.site_name }}</title>`,
  and `config.site_name` is `Micromegas Documentation` — so every blog post's title ends in
  "- Micromegas Documentation".
- Material's `base.html` (`material/templates/base.html:35-38`) already emits
  `<link rel="alternate" type="application/rss+xml">` (and an `updated` counterpart) whenever
  `"rss"` is in `config.plugins` — feed autodiscovery needs no template override once the plugin
  is configured.
- Nothing in CI looks at the generated sitemap or canonical tags.

### Verified against a local build

Building the current docs with `site_url: https://micromegas.info/docs/` was confirmed to produce:

- 93 `<loc>` entries, all prefixed `https://micromegas.info/docs/`, including all 20 blog posts.
- `getting-started/index.html` → `<link rel="canonical" href="https://micromegas.info/docs/getting-started/">`.
- 94 HTML files, 93 of which carry a canonical tag; the one without is `404.html`.
- All 93 canonical URLs map back to the exact file that emitted them.

## Design

### 1. `site_url` (the whole bug)

```yaml
site_url: https://micromegas.info/docs/
```

That single change corrects all 93 sitemap entries and all 93 canonical tags, and it also
dissolves the root collision: MkDocs' `index.md` stops claiming `https://micromegas.info/`, which
the welcome landing page serves.

### 2. Site-root discovery files

Two static files under `welcome/public/`, which Vite copies to `welcome/dist/` and the workflow
copies to the site root. No workflow change is needed for either.

`welcome/public/robots.txt`:

```
User-agent: *
Allow: /

Sitemap: https://micromegas.info/sitemap.xml
Sitemap: https://micromegas.info/docs/sitemap.xml
```

`welcome/public/sitemap.xml` — a hand-written sitemap for the pages MkDocs does not know about:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://micromegas.info/</loc></url>
  <url><loc>https://micromegas.info/intro-micromegas/</loc></url>
  <url><loc>https://micromegas.info/notebooks/</loc></url>
  <url><loc>https://micromegas.info/high-frequency-observability/</loc></url>
  <url><loc>https://micromegas.info/unified-observability-for-games/</loc></url>
  <url><loc>https://micromegas.info/doc/design-presentation/design.html</loc></url>
</urlset>
```

No `lastmod` — a hand-maintained timestamp rots into a lie, and the element is optional. The two
sitemaps are disjoint, so no URL is advertised twice.

The landing page also gains a self-referential canonical in `welcome/index.html`'s head:

```html
<link rel="canonical" href="https://micromegas.info/" />
```

### 3. Blog discoverability

Add `mkdocs-rss-plugin` to `mkdocs/docs-requirements.txt` and configure it in `mkdocs.yml` after
the `tags` plugin:

```yaml
  - rss:
      match_path: blog/posts/.*
      use_git: false
      image: https://micromegas.info/docs/assets/images/micromegas-icon-512.png
      feed_title: Micromegas Blog
      feed_description: Engineering notes from the Micromegas observability platform.
      date_from_meta:
        as_creation: date
        as_update: date
```

`image` matters too: the plugin's Jinja template guards the `<image>` block with
`{% if feed.logo_url is defined %}`, not `is not none`, so leaving `image` unset still renders
the block with a literal `<url>None</url>` — invalid per RSS 2.0, and an aggregator resolving it
relatively would 404 on `https://micromegas.info/docs/None`. The value points at a new
`mkdocs/docs/assets/images/micromegas-icon-512.png` (copied from `branding/`, which
`publish-docs.yml` does not otherwise publish), since RSS 2.0's `<image><url>` wants a raster
(GIF/JPEG/PNG), not the SVG marks already under `assets/images/`.

`use_git: false` matters: the plugin's `date_from_meta.as_creation` and `.as_update` both default
to the string `"git"`, and `actions/checkout@v4` clones at depth 1, so git-derived dates would be
wrong or unavailable in CI. Pointing both at the posts' own `date:` front matter removes the
dependency entirely. This configuration was verified locally: it emits `feed_rss_created.xml`,
`feed_rss_updated.xml` and the two JSON-feed counterparts into the site root (i.e.
`/docs/feed_rss_created.xml`), with 20 items whose `<link>`s are the correct
`https://micromegas.info/docs/blog/...` URLs and whose `<pubDate>`s come from the front matter.

Once the `rss` plugin is enabled, Material's `base.html` automatically emits the feed's
`<link rel="alternate">` autodiscovery tags (both `created` and `updated`) on every page — no
template override is needed for this (see Current State).

Blog-post titles get their own `htmltitle` override in the same file, delegating to `super()` for
every non-post page so the rest of the site is untouched:

```jinja
{% block htmltitle %}
  {% if page and page.file and page.file.src_uri.startswith("blog/posts/") %}
    <title>{{ (page.meta.title if page.meta and page.meta.title else page.title) | striptags }} - Micromegas Blog</title>
  {% else %}
    {{ super() }}
  {% endif %}
{% endblock %}
```

Matching on `blog/posts/` rather than `blog/` keeps the suffix off the blog index, archive, and
category pages — "Blog - Micromegas Blog" would read badly.

### 4. CI guard: `build/check_docs_site.py`

A standalone Python script that validates the **staged tree**, not the live site — so it runs on
pull requests, before anything is published, and needs no network.

```
python3 build/check_docs_site.py public_docs
```

Design:

- Read the expected origin from `<root>/CNAME` (`https://<host>`), so `site_url`, the static
  sitemap, `robots.txt`, and the custom domain cannot drift apart silently.
- `url_to_path(root, url)`: reject a URL not under the origin; `urlsplit` + `unquote` the path;
  a path that is empty or ends in `/` resolves to `index.html` beneath it.
- Checks, each accumulating failures rather than aborting on the first:
  1. **Every `<loc>` in every `**/sitemap.xml` under the root resolves to a file that exists.**
     This is the check that would have caught the reported bug.
  2. **No `<loc>` appears twice**, within a sitemap or across sitemaps — pins the root collision
     the issue describes.
  3. **`robots.txt` exists at the root**, every `Sitemap:` line resolves to an existing file, and
     the advertised set equals the set of `sitemap.xml` files actually found — so adding or
     moving a sitemap without updating `robots.txt` fails the build.
  4. **Every canonical tag points at the file that emitted it.** Scan `<root>/docs/**/*.html` plus
     `<root>/index.html`; extract `<link rel="canonical" href="...">`; skip files with no such tag
     (`404.html`). Deliberately not the whole tree: `public_docs/rustdoc/` alone
     is thousands of generated HTML files, and nothing under either `public_docs/rustdoc/` or
     `public_docs/doc/` carries a canonical tag, so walking them would only add cost with nothing
     to check. (The sitemap checks above do scan the whole tree via `**/sitemap.xml`, since there
     are at most two such files — cost isn't a concern there.)
  5. **Every feed autodiscovery link resolves to a file that exists.** From the same HTML files as
     check 4, extract `<link rel="alternate" type="application/rss+xml" href="...">`. These are the
     two feed links Material emits automatically once the `rss` plugin is enabled (see Current
     State), not anything this plan adds to a template. On ordinary pages the href is genuinely
     relative (Material's `url` filter emits e.g. `../feed_rss_created.xml`), but on `404.html` it
     is root-absolute instead (e.g. `/docs/feed_rss_created.xml`, because `build.py` sets the error
     template's `base_url` to `site_url`'s path). Resolve an href starting with `/` against the
     staged root — the same rule `url_to_path` applies to a root-relative path — and resolve any
     other href against the HTML file's own directory.
- Print every failure, exit 1 if there were any.

The checker itself is wired into `publish-docs.yml` as a step **after** "Prepare staging directory"
(so `CNAME` is already written) and **before** "Deploy to GitHub Pages". Its unit-test step runs
separately and earlier — immediately after "Checkout code", before the Rust/Node build steps — so
a broken checker fails fast instead of surfacing after ~10 minutes of unrelated build work.
`build/check_docs_site.py` and `build/test_check_docs_site.py` are added to the workflow's
`pull_request.paths` filter, otherwise a change to the checker would not run it.

## Implementation Steps

### Phase 1 — Fix the URLs

1. `mkdocs/mkdocs.yml`: set `site_url: https://micromegas.info/docs/`.

### Phase 2 — Site-root discovery files

2. Add `welcome/public/robots.txt` (content above).
3. Add `welcome/public/sitemap.xml` (content above).
4. `welcome/index.html`: add the self-referential `<link rel="canonical">` to the head.

### Phase 3 — Blog discoverability

5. `mkdocs/docs-requirements.txt`: add `mkdocs-rss-plugin>=1.19.0` under "Additional plugins".
6. `mkdocs/mkdocs.yml`: add the `rss` plugin block after `- tags`.
7. `mkdocs/overrides/main.html`: add the `htmltitle` block override.

### Phase 4 — CI guard

8. Add `build/check_docs_site.py` implementing the five checks.
9. Add `build/test_check_docs_site.py` covering the checker's logic against synthetic trees.
10. `.github/workflows/publish-docs.yml`:
    - add `build/check_docs_site.py` and `build/test_check_docs_site.py` to `pull_request.paths`;
    - add a step installing `pytest` and running `python3 -m pytest build/test_check_docs_site.py`
      immediately after "Checkout code", before the Rust/Node setup steps;
    - add a step running `python3 build/check_docs_site.py public_docs` after staging and before
      deploy.

### Phase 5 — Documentation

11. `mkdocs/docs/development/build.md`: in the documentation-build section, note that
    `python3 build/check_docs_site.py public_docs` is a CI step, run from the repo root against
    the tree `publish-docs.yml` stages — not a command to run from `mkdocs/`.
12. `CHANGELOG.md`: an `## Unreleased` entry under **Website**.

## Files to Modify

| File | Change |
| --- | --- |
| `mkdocs/mkdocs.yml` | `site_url` → `/docs/`; add `rss` plugin |
| `mkdocs/docs-requirements.txt` | add `mkdocs-rss-plugin` |
| `mkdocs/docs/assets/images/micromegas-icon-512.png` | new (copied from `branding/`; RSS feed image) |
| `mkdocs/overrides/main.html` | blog-post `htmltitle` override |
| `welcome/public/robots.txt` | new |
| `welcome/public/sitemap.xml` | new |
| `welcome/index.html` | self-referential canonical |
| `build/check_docs_site.py` | new |
| `build/test_check_docs_site.py` | new |
| `.github/workflows/publish-docs.yml` | paths filter; pytest step; checker step |
| `mkdocs/docs/development/build.md` | document the checker |
| `CHANGELOG.md` | Unreleased entry |

## Trade-offs

- **`site_url` with the `/docs/` prefix vs. serving MkDocs at the site root.** Moving the docs to
  the root would break every existing `/docs/...` URL, including the ones the `github.io` 301
  already redirects to. Changing `site_url` matches reality and breaks nothing.
- **Two `Sitemap:` lines in `robots.txt` vs. a `<sitemapindex>` at the root.** A sitemap index is
  arguably tidier but adds a third file and a level of indirection; two lines are equally valid
  per the sitemap protocol and simpler to keep correct.
- **A static `welcome/public/sitemap.xml` vs. generating it.** The presentation list is already
  hardcoded in the workflow, so generation would move duplication rather than remove it. The CI
  check keeps the static file honest about URLs that stop resolving.
- **Validating the staged tree offline vs. curling the live site.** Curling only detects breakage
  after it ships and needs network access in CI; the staged tree is available on every pull
  request that touches these paths.
- **`use_git: false` for the RSS plugin vs. deepening the checkout.** `fetch-depth: 0` would slow
  every docs build to give the plugin dates the front matter already carries.

## Decisions

- Keep the RSS plugin's default `length: 20`. A feed is a recency window, not an archive; the
  sitemap and the blog archive carry full history. Revisit only if a reader actually needs more.
- Leave the JSON feed enabled (plugin default). It costs one extra generated file and some
  aggregators prefer it.
- `/rustdoc/` stays crawlable but out of every sitemap, per the issue's judgment call: it's
  thousands of generated pages of low standalone value.
- Accepted risk: a new presentation added to `publish-docs.yml` will not be added to
  `welcome/public/sitemap.xml` automatically, and nothing fails if it is forgotten — the page is
  merely unlisted. Catching that would need an allowlist of root directories that is itself
  maintenance.
- `robots.txt` carries no AI-crawler `Disallow` blocks — default-allow is the intended posture;
  the file exists only to advertise the two sitemaps.
- RSS `image` points at the 512px icon PNG as-is, despite RSS 2.0's nominal 144x400px cap on
  `<image>` — most aggregators ignore the cap, and adding a resized asset just for this one
  element isn't worth the extra file.

## Documentation

- `mkdocs/docs/development/build.md` — document `build/check_docs_site.py` in the documentation
  build section, as a CI step run from the repo root against the workflow's staged tree.
- `CHANGELOG.md` — `## Unreleased`, **Website** entry covering the `site_url` fix, `robots.txt`,
  the root sitemap, the RSS feed, the blog title suffix, and the CI check.

## Testing Strategy

**Unit tests — `build/test_check_docs_site.py`** (pytest, `tmp_path` fixtures building a tiny
staged tree: a `CNAME`, a root `sitemap.xml`, a `docs/sitemap.xml`, a `robots.txt`, and a couple
of HTML files with canonical tags and a feed autodiscovery link):

1. A well-formed tree passes (exit 0).
2. A `<loc>` whose target file is absent fails, and the message names the URL — this is the
   reported bug reduced to a fixture.
3. An HTML file whose canonical points at a different path fails — the blog defect reduced to a
   fixture.
4. The same `<loc>` in two sitemaps fails — the root collision reduced to a fixture.
5. A `sitemap.xml` present in the tree but absent from `robots.txt` fails.
6. An HTML file whose `<link rel="alternate" type="application/rss+xml">` href resolves (relative
   to the file's own directory) to a file that does not exist fails.
7. A `404.html`-shaped fixture whose feed link href is root-absolute (e.g.
   `/docs/feed_rss_created.xml`) resolves correctly against the staged root, and a broken
   root-absolute href still fails — pins the check 5 fix for error templates.

The permissive direction is what these pin: a checker that silently passes a broken tree is worse
than no checker, and nothing else in CI would notice.

**Whole-site check in CI** — `python3 build/check_docs_site.py public_docs` in `publish-docs.yml`
runs the same logic against the real staged tree of 94 HTML files, two sitemaps, and a
`robots.txt`. This catches what a unit test against synthetic fixtures cannot: that MkDocs,
Material, the `blog` plugin, the `rss` plugin, Vite's `public/` copy, and the workflow's `cp`
steps actually compose into a tree whose advertised URLs exist.

No live-DB or service test applies here.

## Manual Verification

Each step needs a human to look at generated output that no assertion in this plan pins.

1. Build the docs locally and confirm the feed and titles:
   ```
   cd mkdocs && python build-docs.py
   ```
   Expect `site/feed_rss_created.xml` to exist with 20 `<item>` entries, and
   `site/blog/2026/05/04/micromegas-speaks-open-telemetry-now/index.html` to contain
   `<title>Micromegas Speaks Open Telemetry Now - Micromegas Blog</title>`. The wording of a
   `<title>` is an editorial judgment, not an invariant worth asserting.
2. Open `site/feed_rss_created.xml` in a feed reader (or `https://validator.w3.org/feed/`) and
   confirm the items render with readable titles, dates, and abstracts. Feed rendering is the
   reader's behavior, not ours.
3. After the PR merges and Pages redeploys, confirm the acceptance criteria against the live site:
   ```
   curl -s https://micromegas.info/docs/getting-started/ | grep canonical
   curl -so /dev/null -w "%{http_code}\n" https://micromegas.info/robots.txt
   curl -so /dev/null -w "%{http_code}\n" https://micromegas.info/sitemap.xml
   ```
   Expect the canonical to read `https://micromegas.info/docs/getting-started/` and both curls to
   print `200`. This is the one check that exercises GitHub Pages' own serving behavior rather
   than the staged tree.
