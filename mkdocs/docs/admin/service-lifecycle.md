# Service Lifecycle & Shutdown

All Micromegas services shut down gracefully on `SIGTERM`: they stop accepting
new work, drain what is already in flight, and then exit. This lets orchestrators
(Kubernetes, ECS, systemd) roll deployments without dropping in-flight ingestion
or maintenance work.

## Services

Graceful shutdown applies to every long-running service:

| Service | Binary | What it drains on `SIGTERM` |
|---|---|---|
| Telemetry ingestion | `telemetry-ingestion-srv` | In-flight HTTP ingestion requests |
| FlightSQL | `flight-sql-srv` | In-flight query RPCs |
| Analytics web app | `analytics-web-srv` | In-flight HTTP requests |
| Maintenance daemon | `telemetry-admin crond` | Running materialization / retention tasks |

Each accepts a `--shutdown-grace-period-seconds` flag (default: **25**):

```bash
telemetry-ingestion-srv --shutdown-grace-period-seconds 25
flight-sql-srv          --shutdown-grace-period-seconds 25
analytics-web-srv       --shutdown-grace-period-seconds 25
telemetry-admin crond   --shutdown-grace-period-seconds 25
```

The same value can be set with the `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS`
environment variable, which is often more convenient in container deployments
where the rest of the configuration is already supplied via the environment:

```bash
export MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS=25
```

The flag takes precedence over the environment variable, which in turn takes
precedence over the default.

## How it works

On `SIGTERM`:

1. The service **stops accepting new work** — the HTTP/gRPC listener stops taking
   new connections; the maintenance daemon stops scheduling new tasks.
2. It **drains in-flight work** — already-accepted requests, RPCs, or running
   maintenance tasks are allowed to finish.
3. A **grace-period deadline** bounds the drain. If work is still in flight when
   `--shutdown-grace-period-seconds` elapses, the service logs a warning and exits
   anyway, so shutdown never hangs indefinitely.

A clean drain logs `drain completed`; hitting the deadline logs
`grace period of <N>s elapsed with work still in flight`.

!!! note
    Graceful shutdown triggers on `SIGTERM`, which is what orchestrators send.
    `SIGINT` (Ctrl-C in an interactive shell) exits immediately without draining.

## Tuning for orchestrators

**Keep the grace period shorter than your orchestrator's termination window**, so
the service finishes draining and exits on its own before the platform escalates
to `SIGKILL`. The 25-second default sits under the common 30-second platform
defaults.

=== "Kubernetes"

    ```yaml
    spec:
      # Must be >= the service grace period so the pod isn't SIGKILLed mid-drain
      terminationGracePeriodSeconds: 30
      containers:
        - name: telemetry-ingestion
          env:
            - name: MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS
              value: "25"
    ```

=== "ECS"

    ```jsonc
    {
      // Must be >= the service grace period
      "stopTimeout": 30,
      "environment": [
        { "name": "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS", "value": "25" }
      ]
    }
    ```

If your write latency is high (large blocks, slow object store), raise both the
service grace period and the orchestrator window together. Setting the service
grace period *above* the orchestrator window is pointless — the platform will
`SIGKILL` the process before the drain can complete.

## Data durability

Graceful shutdown is what keeps a rolling deploy from losing telemetry:

- **Ingestion writes are synchronous.** A block is written to object storage and
  recorded in PostgreSQL *before* the request returns `200`. A request that has
  been acknowledged is already durable; a request still in flight at `SIGTERM` is
  given the full grace period to complete.
- **Writes are idempotent.** Blocks are stored at a deterministic path and
  recorded with `ON CONFLICT DO NOTHING`. If a request is cut off at the deadline,
  the client receives an error (no `200`) and can safely retry — no duplication.
- **Maintenance work is re-runnable.** Materialization only reads source data and
  writes derived partitions. A task interrupted at the deadline simply leaves its
  partition unwritten; the next scheduled run redoes it. No source data is lost.

The residual risk is a client that cannot complete its request within the grace
period *and* does not retry. If you need end-to-end guarantees under load, raise
the grace period and ensure your producers retry on connection errors.
