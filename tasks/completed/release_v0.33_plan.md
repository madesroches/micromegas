# Release Plan: Micromegas v0.33.0

## Overview

Release version 0.33.0 of Micromegas, 30 commits since v0.32.0.
Highlights:

- Access control: `*` all-audiences read grant and `key:<key_id>` selectors for analytics keys.
- Deployment settings stored in the database, with an Admin -> Settings page; per-stream-tag retention.
- `tracing` spans recorded as async spans; HLL sketch SQL functions.
- Notebook Stop button, single-select chart cells, Log cell repeat collapsing.
- `reqwest` 0.13 (OS trust store), Blender autosave fix.

## Status

- Version 0.33.0 already set across all packages; `release` branch identical to `main` at `a7af2b2c1`.
- No new publishable crate.
- Web app, Grafana, welcome and Python formatting checks pass.

## Phases

Follow `tasks/release_plan_template.md`: commit docs and plan, tag (four tags), push branch and tags
in two commands, Phase 1 crates, Phase 2 PyPI, Phase 3 GitHub release with Grafana archive and
`--latest`, Phase 3.5 Docker (8 services, both arches), Phase 4 bump to 0.34.0, Phase 5 cleanup,
Phase 6 PR to main.
