# Telemetry Sink Transport Tuning

The Rust telemetry sink (`micromegas-telemetry-sink`) queues process/stream
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
