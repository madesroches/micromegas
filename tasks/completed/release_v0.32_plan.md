# Release Plan: Micromegas v0.32.0

## Overview

Release version 0.32.0 of Micromegas, 67 commits since v0.31.0 (v0.31.1 was Python-client only).
Highlights:

- Properties on thread and async spans, dynamic log/metric properties, runtime-built span and
  metric names, and a leak-free C API.
- Cheaper instrumentation, benchmarked against OpenTelemetry.
- DDL-defined eagerly materialized views and the `micromegas-views` CLI.
- Access control hardening: API-key IP allowlists, narrower `is_admin`, control-plane audit record.
- Web app: Stacked Bar cell, `$me.*` macros, query-backed markdown cells.
- GCS and Azure object storage backends, DataFusion 55.2.

The `*_API_KEYS` / `*_AUDIENCE_GRANTS` startup warnings were scheduled for removal in this release;
they stay, tracked in #1698.

## Status

- Version 0.32.0 already set across all packages.
- `release` branch identical to `main` at `4ecd820d9`; `origin/release` does not exist (stale ref
  pruned), so the first push creates it.
- No new publishable crate (`otel-comparison-bench` is `publish = false`).
- Python integration tests fail without a server (87 expected failures); black, web app, grafana and
  welcome checks pass.

## Phases

Follow `tasks/release_plan_template.md`: commit docs and plan, tag (four tags), push branch and tags
in two commands, Phase 1 crates, Phase 2 PyPI, Phase 3 GitHub release with Grafana archive and
`--latest`, Phase 3.5 Docker (8 services, both arches), Phase 4 bump to 0.33.0, Phase 5 cleanup,
Phase 6 PR to main.
