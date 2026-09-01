# Memory Allocator Configuration

Every service binary (`object-cache-srv`, `flight-sql-srv`, `http-gateway`, `analytics-web-srv`,
`monolith`, `telemetry-maintenance-srv`, `telemetry-ingestion-srv`, `redis-exporter`) declares
[`tikv-jemallocator`](https://github.com/tikv/jemallocator) as its global allocator and is built
with the same `malloc_conf`:

```
background_thread:true,dirty_decay_ms:5000,muzzy_decay_ms:0
```

- **`background_thread:true`** — jemalloc spawns purge threads at startup and decays dirty pages
  on a timer, instead of only when an allocation happens to land in the arena. Without it, a
  process that just finished a burst of churn can leave dirty pages mapped indefinitely, since
  nothing else prompts jemalloc to return them.
- **`dirty_decay_ms:5000`** — halves jemalloc's stock 10s idle window before dirty pages are
  handed back to the OS.
- **`muzzy_decay_ms:0`** — jemalloc 5.3's own default, pinned explicitly so it can't drift with an
  upstream release; `0` means muzzy pages are `MADV_DONTNEED`'d immediately.

This is compiled in, not read from an environment variable: it's exported as the weak
`_rjem_malloc_conf` symbol jemalloc's `conf.c` resolves at startup (`tikv-jemalloc-sys` builds
with the `_rjem_` prefix).

## Overriding at deployment time

`_RJEM_MALLOC_CONF` outranks the compiled-in value and needs no rebuild:

```bash
export _RJEM_MALLOC_CONF="background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:0"
```

(Note the `_RJEM_` prefix — the unprefixed `MALLOC_CONF` has no effect on these binaries.)

## What to watch

The `jemalloc-metrics` feature these binaries build with emits four gauges every ~5s through the
shared `system_monitor` sampler:

| Metric | Meaning |
|---|---|
| `jemalloc_allocated_bytes` | Bytes actually in use by the application. |
| `jemalloc_resident_bytes` | Bytes physically resident (allocated + retained-but-dirty). |
| `jemalloc_mapped_bytes` | Bytes mapped from the OS, including retained pages not yet purged. |
| `jemalloc_retained_bytes` | Bytes jemalloc has unmapped but kept reserved in its own address space. |

`jemalloc_resident_bytes - jemalloc_allocated_bytes` is the gap this configuration targets: it
should fall relative to the stock config, and — the more important signal — recover after a burst
of churn ends rather than staying at its peak. See `admin/object-cache.md` for a worked example of
diagnosing a process-memory climb with these gauges.
