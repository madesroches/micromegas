# Release Plan: Micromegas v0.34.0

## Overview

Release version 0.34.0 of Micromegas, 40 commits since v0.33.0.
Highlights:

- MCP server on `analytics-web-srv` and the monolith (`/mcp`), with OAuth discovery.
- On-demand materialized views and JIT partitions sized from the query range.
- Fair per-identity query admission; audit record gains scan estimates and output bytes.
- Concurrent update-group materialization, materialization spans, cgroup CPU metrics.
- Bounded LZ4 decompression, transit recursion and call-tree depth.
- Admin -> View Sets page, admin mode for destructive statements, Bar Chart cell.

## Status

- Version 0.34.0 already set across all packages; `release` branch identical to `main` at `ea362e0f2`.
- No new publishable crate or Docker service.
- Web app, Grafana, welcome and Python checks pass (Python integration tests need a live server).

## Phases

Follow `tasks/release_plan_template.md`: commit docs and plan, tag (four tags), push branch and tags
in two commands, Phase 1 crates, Phase 2 PyPI, Phase 3 GitHub release with Grafana archive and
`--latest`, Phase 3.5 Docker (8 services, both arches), Phase 4 bump to 0.35.0, Phase 5 cleanup,
Phase 6 PR to main.

## Outcome

All artifacts published on 2026-11-26: 16 crates on crates.io, `micromegas` 0.34.0 on PyPI, the
`v0.34.0` GitHub release with the Grafana archive (holding "Latest"), the `capi-v0.34.0` and
`blender-v0.34.0` releases, and 8 Docker services for amd64 (`:0.34.0`) and arm64 (`:0.34.0-arm64`).
arm64 waited on a user-run binfmt install; it was built from a worktree at the tag. Workspace bumped
to 0.35.0.
