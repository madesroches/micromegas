# Redis Metrics Exporter

`micromegas-redis-exporter` samples a single Redis server over one persistent
connection and sends its metrics to a Micromegas stack through the standard
ingestion service — no Prometheus layer involved. Run one exporter per Redis
instance (e.g. a sidecar or one Deployment per node).

## Quick start

```bash
docker run -d --name redis-exporter \
  -e MICROMEGAS_TELEMETRY_URL=http://ingestion:9000 \
  -e MICROMEGAS_INGESTION_API_KEY=... \
  -e MICROMEGAS_REDIS_EXPORTER_REDIS_URL=redis://my-redis:6379 \
  -e MICROMEGAS_PROCESS_PROPERTIES=cluster=eu-west,role=cache \
  marcantoinedesroches/micromegas-redis-exporter:latest
```

## Metric presets

Presets are cumulative; select with `--metrics` / `MICROMEGAS_REDIS_EXPORTER_METRICS`:

| Preset | Collected |
|--------|-----------|
| `core` | One `INFO` per tick: `redis_up`, memory, clients, throughput, keyspace hits/misses, replication, persistence, per-db key counts |
| `extended` | core + `SLOWLOG LEN` (`redis_slowlog_length`) |
| `full` (default) | extended + per-command stats (`redis_command_*` tagged `command=`) + `LATENCY LATEST` (`redis_latency_*` tagged `event=`) |

At the default settings (`full`, 1s interval), a busy server with many
distinct commands and latency events can emit several hundred measures per
second per exporter — roughly 50M rows/day. Drop to `extended`/`core` or
widen `--sample-interval-seconds` to trade detail for volume. On managed
Redis offerings that restrict `SLOWLOG`/`LATENCY` via ACLs, `extended`
and/or `full` will report `redis_up=0` on every tick (the exporter fails
the whole sample if a required command is denied) — drop to `core`, or to
`extended` if only `LATENCY` is restricted, on such deployments.

Every metric carries an `instance` property (from `--target-name`, default
`host:port`) plus any user-supplied properties, so several exporters can share
one stack:

```sql
SELECT time, value FROM measures
WHERE name = 'redis_used_memory_bytes'
  AND property_get(properties, 'instance') = 'my-redis:6379';
```

### Process properties vs metric properties

There are two places to hang a tag, and the right one depends on whether the
tag varies:

- `MICROMEGAS_PROCESS_PROPERTIES` (comma-separated `key=value`) tags the
  **exporter process**. Stored once on its `processes` row and readable from
  the `process_properties` column of `measures` and `log_entries`, so it also
  covers the exporter's own log output. Use it for everything constant —
  `cluster`, `region`, `role`, `namespace`, `pod`. This is the standard
  variable every micromegas process honors; see
  [Telemetry Sink Configuration](telemetry-sink-tuning.md#process-properties)
  for the full format and precedence rules.
- `--property` / `MICROMEGAS_REDIS_EXPORTER_PROPERTIES` tags **every metric**,
  alongside the `instance`/`command`/`db`/`event` tags the exporter attaches
  itself. Costs storage on every measure the exporter emits, so reach for it
  only when a query needs the tag on the measure row itself rather than
  through a join on the process.

```sql
-- constant tag: read it off the process
SELECT time, value FROM measures
WHERE name = 'redis_used_memory_bytes'
  AND property_get(process_properties, 'cluster') = 'prod';
```

## Configuration

| Flag | Env | Default |
|------|-----|---------|
| `--redis-url` | `MICROMEGAS_REDIS_EXPORTER_REDIS_URL` | `redis://127.0.0.1:6379` |
| — | `MICROMEGAS_REDIS_EXPORTER_REDIS_PASSWORD` | unset (overrides URL password) |
| `--metrics` | `MICROMEGAS_REDIS_EXPORTER_METRICS` | `full` |
| `--sample-interval-seconds` | `MICROMEGAS_REDIS_EXPORTER_SAMPLE_INTERVAL_SECONDS` | `1` |
| `--target-name` | `MICROMEGAS_REDIS_EXPORTER_TARGET_NAME` | `host:port` from the URL |
| `--property k=v` (repeatable) | `MICROMEGAS_REDIS_EXPORTER_PROPERTIES` (comma-separated) | none |
| `--health-listen-addr` | `MICROMEGAS_REDIS_EXPORTER_HEALTH_LISTEN_ADDR` | off |
| — | `MICROMEGAS_PROCESS_PROPERTIES` (comma-separated) | none |

Telemetry destination, authentication, and process properties use the standard
client contract: `MICROMEGAS_TELEMETRY_URL` plus either
`MICROMEGAS_INGESTION_API_KEY` or the OIDC client-credentials variables
(`MICROMEGAS_OIDC_TOKEN_ENDPOINT`, `MICROMEGAS_OIDC_CLIENT_ID`,
`MICROMEGAS_OIDC_CLIENT_SECRET`), and `MICROMEGAS_PROCESS_PROPERTIES` for
process-level tags. `MICROMEGAS_PROCESS_PROPERTIES` has no flag equivalent: the
telemetry guard is built before command-line parsing, and a process's
properties are sent once at startup.

## Kubernetes

The exporter is Kubernetes-friendly but never requires it. Probes are opt-in:
`/health` (liveness) is always 200 while the process runs; `/ready` is 200 once
sampling has started. A Redis outage does **not** flip readiness — the exporter
stays up to report `redis_up=0`.

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: redis-exporter
spec:
  replicas: 1
  selector:
    matchLabels: { app: redis-exporter }
  template:
    metadata:
      labels: { app: redis-exporter }
    spec:
      containers:
        - name: redis-exporter
          image: marcantoinedesroches/micromegas-redis-exporter:latest
          env:
            - name: MICROMEGAS_TELEMETRY_URL
              value: http://ingestion:9000
            - name: MICROMEGAS_INGESTION_API_KEY
              valueFrom:
                secretKeyRef: { name: micromegas-ingestion, key: api-key }
            - name: MICROMEGAS_REDIS_EXPORTER_REDIS_URL
              value: redis://redis:6379
            - name: MICROMEGAS_REDIS_EXPORTER_REDIS_PASSWORD
              valueFrom:
                secretKeyRef: { name: redis, key: password }
            - name: MICROMEGAS_REDIS_EXPORTER_TARGET_NAME
              value: redis-main
            - name: POD_NAMESPACE
              valueFrom:
                fieldRef: { fieldPath: metadata.namespace }
            - name: POD_NAME
              valueFrom:
                fieldRef: { fieldPath: metadata.name }
            - name: MICROMEGAS_PROCESS_PROPERTIES
              value: cluster=prod,namespace=$(POD_NAMESPACE),pod=$(POD_NAME)
            - name: MICROMEGAS_REDIS_EXPORTER_HEALTH_LISTEN_ADDR
              value: 0.0.0.0:8081
          ports:
            - containerPort: 8081
              name: health
          livenessProbe:
            httpGet: { path: /health, port: health }
          readinessProbe:
            httpGet: { path: /ready, port: health }
```
