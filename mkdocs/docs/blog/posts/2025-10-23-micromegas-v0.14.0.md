---
date: 2025-10-23
authors:
  - madesroches
categories:
  - Release
tags:
  - release
  - datafusion
  - sql
---

# Micromegas v0.14.0 Released

Key improvements:

<!-- more -->

- Completed JSONB migration for better storage efficiency
- Enhanced SQL analytics with custom table registration
- Fixed NULL handling across the SQL-Arrow bridge
- Security updates (Vite, DataFusion, Arrow Flight)

Micromegas handles up to 100k events/second with 20ns overhead, combining logs, metrics, and traces in a unified SQL-queryable lakehouse.

Available on [crates.io](https://crates.io/crates/micromegas) and [PyPI](https://pypi.org/project/micromegas/).

[Release notes on GitHub](https://github.com/madesroches/micromegas/releases)
