---
date: 2025-11-14
authors:
  - madesroches
categories:
  - Release
tags:
  - release
  - authentication
  - grafana
  - oidc
---

# Micromegas v0.15.0: Authentication Framework & Enhanced Grafana Integration

Excited to share the latest release of Micromegas, our open-source high-frequency observability platform for logs, metrics, and traces.

<!-- more -->

## New Authentication Framework

Introducing micromegas-auth, a dedicated crate providing enterprise-ready authentication:

- OIDC (OpenID Connect) support with client credentials flow
- API key authentication for service accounts
- Automatic token refresh and secure credential management
- HTTP authentication for ingestion services

## Enhanced Grafana Plugin

Now integrated into the main repository with new capabilities:

- OAuth 2.0/OIDC authentication support
- Variable query editor with automatic time filtering
- Datasource migration tools for schema updates
- Fixed 28 security vulnerabilities

## Unreal Engine Integration

Modernized telemetry sink module with better process properties support for game developers.

## Available Now

- 12 Rust crates on crates.io (including the new micromegas-auth)
- Python library (v0.15.0) on PyPI
- Grafana plugin (manual installation)

Built for teams that need to collect up to 100k events/second per process with minimal overhead (20ns per event), while keeping storage costs low through Parquet-based object storage.

[Release notes on GitHub](https://github.com/madesroches/micromegas/releases)
