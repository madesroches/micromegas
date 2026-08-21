# Telemetry Sink Configuration

Environment variables honored by every process built on the Rust telemetry sink
(`micromegas-telemetry-sink`) — the services, the monolith, the Redis exporter,
and any instrumented application, including native ones through the C ABI.

## Transport tuning

The sink queues process/stream
metadata and log/metrics/thread/image blocks in priority order (Metadata,
Logs, Metrics, Traces) and drains them with a bounded number of concurrent
HTTP requests. Under normal operation nothing is dropped; the environment
variables below only matter if the ingestion service falls behind or becomes
unreachable.

```bash
# Soft cap, in bytes: once the queue holds at least this many bytes, new
# Traces items (thread/image blocks) are dropped first. Default 128 MiB.
export MICROMEGAS_TELEMETRY_MAX_QUEUE_BYTES=134217728

# Hard cap, in bytes: once reached, Logs/Metrics are dropped too. Process
# and stream metadata are never dropped. Default 256 MiB.
export MICROMEGAS_TELEMETRY_HARD_QUEUE_BYTES=268435456

# Maximum number of insert_* HTTP requests in flight at once. Default 3;
# set to 1 to restore strictly serial sends.
export MICROMEGAS_TELEMETRY_MAX_IN_FLIGHT_REQUESTS=3

# Per-request timeout, in seconds. Bounds how long a single send attempt can
# hang against an ingestion service that accepts connections but never
# responds. Default 10.
export MICROMEGAS_TELEMETRY_REQUEST_TIMEOUT_SECS=10
```

A stream's metadata (`insert_stream`) is only sent once that stream produces
its first block, so short-lived or idle streams cost nothing on the wire.
Each stream does retain a small pending-metadata entry in the sink's memory
for its lifetime, even if it never produces a block.

## Process properties

`MICROMEGAS_PROCESS_PROPERTIES` attaches arbitrary deployment tags to the
process record — a comma-separated `key=value` list, read once at startup:

```bash
export MICROMEGAS_PROCESS_PROPERTIES=cluster=prod,region=eu-west,role=cache
```

The tags land on the `processes` row and are queryable through the
`process_properties` column that `log_entries`, `measures`, and the other event
views already expose:

```sql
SELECT time, msg FROM log_entries
WHERE property_get(process_properties, 'cluster') = 'prod';
```

Prefer a process property over a per-event one for anything constant for the
lifetime of the process: it is stored once on the `processes` row, where the
equivalent event property is carried on every row the process emits.

Notes:

- Entries are trimmed and blank ones ignored, so a trailing comma is harmless
  and a Kubernetes manifest rendering an unset optional variable to `""` is
  not an error.
- Only the first `=` separates, so values may contain `=` — but never a comma.
- A malformed entry (no `=`, an empty key, or a key in the reserved
  `micromegas.` namespace, which the ingestion service strips on write) fails
  startup rather than being silently ignored.
- Keys the process sets itself win over this variable: `version` and the
  host-derived `exe`, `username`, `realname`, `computer`, `distro`,
  `cpu_brand`, `cpu_count`, and `total_memory` cannot be overridden here.
  Within the list itself, the first occurrence of a key wins.

To interpolate values Kubernetes only knows at pod creation, bind them to
their own variables first and reference those:

```yaml
env:
  - name: POD_NAMESPACE
    valueFrom: { fieldRef: { fieldPath: metadata.namespace } }
  - name: POD_NAME
    valueFrom: { fieldRef: { fieldPath: metadata.name } }
  - name: MICROMEGAS_PROCESS_PROPERTIES
    value: cluster=prod,namespace=$(POD_NAMESPACE),pod=$(POD_NAME)
```
