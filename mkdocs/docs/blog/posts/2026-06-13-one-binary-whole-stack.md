---
date: 2026-06-13
authors:
  - madesroches
categories:
  - Engineering
tags:
  - monolith
  - deployment
  - docker
  - rust
  - observability
---

# One Binary, Whole Stack

The full Micromegas stack — ingestion, analytics, maintenance, web UI — now fits in a free-tier VM. One binary, one Postgres, one object store. That's it.

This is the `micromegas-monolith`: a single process that runs all four roles on one tokio runtime, sharing a data-lake connection, a cache, and a SIGTERM fanout. Spin it up on a free-tier VM, your laptop, or whatever throwaway instance your cloud provider gives away. Point your instrumented app at it. Get logs, metrics, and traces with a full SQL query interface and a web UI, for essentially zero running cost.

That's the pitch for personal telemetry. For teams it's also the fastest demo path: one compose file, no orchestration.

<!-- more -->

```
docker compose -f docker-compose.monolith.yaml up
```

Postgres + local-volume object store + the monolith. The shipped `docker-compose.monolith.yaml` does the wiring.

Or run it directly against a dev database:

```
cargo run --bin micromegas-monolith -- --roles all \
  --listen-endpoint-http 127.0.0.1:9000 \
  --frontend-dir ../analytics-web-app/dist \
  --disable-auth
```

Ingestion on :9000, FlightSQL on :50051, web on :3000, maintenance daemon materializing views — all in the same process.

## Why one process beats four containers

You could already run all the services on one box with docker-compose. The monolith isn't just a convenience wrapper — it's architecturally better.

## One shared cache

At real telemetry volume (a single game instance generates roughly 100 Mbps sustained), the `LakehouseContext` is hot and large: metadata cache, Parquet file cache, DataFusion runtime, all active. In the split deployment, `flight-sql-srv` and the maintenance daemon each maintain their own — two copies of the same working set competing for page cache, two Postgres connection pools hitting the same backend.

The monolith shares one. One cache, one pool, one jemalloc arena. Every byte of RAM goes into a single unified cache instead of being split across competing processes. The busier the stack, the bigger the advantage.

## One work-stealing runtime

Four separate tokio runtimes each size their worker pool to the core count. On an 8-core machine that's 32+ worker threads fighting for 8 cores.

But the real argument isn't thread count — it's adaptivity. A developer's workload is phase-shifted: run the game (ingestion-heavy, queries quiet), then stop and investigate (FlightSQL-heavy, ingestion quiet). The optimal allocation is all cores on whatever is hot right now.

The monolith does exactly that. Tokio's work-stealing scheduler moves tasks automatically as the phase flips — threads handling ingestion requests steal DataFusion scan work the instant queries start landing. No tuning, no static thread caps, no reserved-but-idle cores. The split deployment can't do this no matter how you configure it; work-stealing across process boundaries isn't possible.

## Scale up, not out

The efficiency wins compound with load. More ingestion volume means a hotter cache — which means sharing it matters more, not less. More cores means more work-stealing headroom. A high-RAM workstation running the monolith will outperform the same hardware split across containers, because all the memory and all the compute are unified behind a single scheduler.

The split deployment is still there for production, HA, and teams that need hard role isolation. The monolith doesn't replace it — it's the first rung: personal stack, demo, dev machine, free-tier VM. One command, full observability pipeline, zero ops.
