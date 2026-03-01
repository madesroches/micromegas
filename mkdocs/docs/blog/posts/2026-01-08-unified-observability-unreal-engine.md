---
date: 2026-01-08
authors:
  - madesroches
categories:
  - Engineering
tags:
  - observability
  - unreal-engine
  - game-development
  - open-source
  - telemetry
---

# Unified Observability for Unreal Engine Games

Just presented our unified observability approach for Unreal Engine games. The debugging loop we all know — and how to break out of it.

<!-- more -->

The debugging loop we all know:

1. Receive bug report: "Game hitched during combat"
2. Try to reproduce... fail
3. Mark as "cannot reproduce"

What if you could understand issues without reproducing them?

With Micromegas + Unreal:

- UE_LOG, metrics, and traces captured automatically
- Every event correlated: process, thread, session, build
- One tool gets you: CPU trace + logs + metrics from the exact frame where a player hitched

## The Workflow

- Browse clients, filter by exe or time range.
- Spot the spike in frame time.
- One click to Perfetto for the full CPU trace.
- Or check the logs from the same process and time.

It's open source, uses SQL (via FlightSQL), and works with Grafana out of the box.

[Watch the presentation](https://micromegas.info/unified-observability-for-games/)
