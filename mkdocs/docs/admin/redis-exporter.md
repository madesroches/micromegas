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
  -e MICROMEGAS_REDIS_EXPORTER_PROPERTIES=cluster=eu-west,role=cache \
  marcantoinedesroches/micromegas-redis-exporter:latest
```

## Metric presets

Presets are cumulative; select with `--metrics` / `MICROMEGAS_REDIS_EXPORTER_METRICS`:

| Preset | Collected |
|--------|-----------|
| `core` | One `INFO` per tick: `redis_up`, memory, clients, throughput, keyspace hits/misses, replication, persistence, per-db key counts |
| `extended` | core + `SLOWLOG LEN` (`redis_slowlog_length`) |
| `full` (default) | extended + per-command stats (`redis_command_*` tagged `command=`) + `LATENCY LATEST` (`redis_latency_*` tagged `event=`) |

Every metric carries an `instance` property (from `--target-name`, default
`host:port`) plus any user-supplied properties, so several exporters can share
one stack:

```sql
SELECT time, value FROM measures
WHERE name = 'redis_used_memory_bytes'
  AND property_get(properties, 'instance') = 'my-redis:6379';
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

Telemetry destination and authentication use the standard client contract:
`MICROMEGAS_TELEMETRY_URL` plus either `MICROMEGAS_INGESTION_API_KEY` or the
OIDC client-credentials variables (`MICROMEGAS_OIDC_TOKEN_ENDPOINT`,
`MICROMEGAS_OIDC_CLIENT_ID`, `MICROMEGAS_OIDC_CLIENT_SECRET`).

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
            - name: MICROMEGAS_REDIS_EXPORTER_PROPERTIES
              value: cluster=prod,namespace=$(POD_NAMESPACE)
            - name: MICROMEGAS_REDIS_EXPORTER_HEALTH_LISTEN_ADDR
              value: 0.0.0.0:8081
            - name: POD_NAMESPACE
              valueFrom:
                fieldRef: { fieldPath: metadata.namespace }
          ports:
            - containerPort: 8081
              name: health
          livenessProbe:
            httpGet: { path: /health, port: health }
          readinessProbe:
            httpGet: { path: /ready, port: health }
```
