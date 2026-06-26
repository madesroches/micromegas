# Native / Non-Rust Integration via `micromegas-capi`

`micromegas-capi` is a thin C ABI shim over the Micromegas telemetry producer stack.
It lets any non-Rust process — a Python script, a C/C++ tool, a game engine plugin —
emit logs and metrics into Micromegas without building a Rust project.

## How it works

```
Your process
└── libmicromegas_capi.{so,dll,dylib}   (loaded at runtime via dlopen/ctypes/LoadLibrary)
    ├── flat C ABI  (mm_init / mm_log / mm_metric_* / mm_flush / mm_shutdown)
    ├── holds TelemetryGuard (buffers + flush state)
    ├── string interner  (runtime metric names → &'static metadata, bounded)
    └── HttpEventSink  ── own OS thread + tokio runtime ──► ingestion-srv
```

The transport thread is internal to the library and starts when `mm_init` succeeds.
Your calling thread is never blocked waiting on the network.

## Building the library

```bash
# Linux / WSL
cd rust/
cargo build --package micromegas-capi --release
# produces:  rust/target/release/libmicromegas_capi.so
#            rust/target/release/libmicromegas_capi.a

# Windows (from a MSVC-capable shell)
cargo build --package micromegas-capi --release --target x86_64-pc-windows-msvc
# produces:  rust/target/x86_64-pc-windows-msvc/release/micromegas_capi.dll
#            rust/target/x86_64-pc-windows-msvc/release/micromegas_capi.lib
```

## C API reference

Include `rust/capi/include/micromegas.h`.

### Level constants

| Constant | Value | Meaning |
|---|---|---|
| `MM_LEVEL_FATAL` | 1 | Crash / panic |
| `MM_LEVEL_ERROR` | 2 | Serious error |
| `MM_LEVEL_WARN`  | 3 | Warning |
| `MM_LEVEL_INFO`  | 4 | Informational |
| `MM_LEVEL_DEBUG` | 5 | Debug detail |
| `MM_LEVEL_TRACE` | 6 | Very verbose |

### `mm_init`

```c
MmHandle *mm_init(const MmConfig *cfg);
```

Initializes the telemetry system and returns an opaque handle.
Returns `NULL` on failure.

`MmConfig` fields:

| Field | Type | Meaning |
|---|---|---|
| `sink_url` | `const char *` | Ingestion endpoint (NULL → reads `MICROMEGAS_TELEMETRY_URL`) |
| `property_keys` | `const char **` | Parallel array of process-property key strings |
| `property_values` | `const char **` | Parallel array of process-property value strings |
| `property_count` | `unsigned int` | Length of the arrays above (0 = no properties) |

Authentication is always read from environment variables:

- `MICROMEGAS_INGESTION_API_KEY` — API key (simplest option)
- `MICROMEGAS_OIDC_TOKEN_ENDPOINT` / `MICROMEGAS_OIDC_CLIENT_ID` / `MICROMEGAS_OIDC_CLIENT_SECRET` — OIDC client credentials

### `mm_shutdown`

```c
void mm_shutdown(MmHandle *handle);
```

Flushes all pending events, joins the transport thread, and frees the handle.
Must be called before process exit.  Safe to call with `NULL`.

### `mm_log`

```c
void mm_log(MmHandle *handle, int level, const char *target, const char *msg);
```

Emits a log event.  `target` is a subsystem name (e.g. `"myapp.render"`).
Both `target` and `msg` may be `NULL` (silently ignored).

### `mm_metric_i` / `mm_metric_f`

```c
void mm_metric_i(MmHandle *handle, const char *name, const char *unit, uint64_t value);
void mm_metric_f(MmHandle *handle, const char *name, const char *unit, double  value);
```

Emit an integer or floating-point metric.

!!! warning "Cardinality contract"
    Each unique `(name, unit)` pair is permanently interned in memory
    (`Box::leak`).  Keep metric names **low-cardinality and bounded** —
    never pass session IDs, per-frame counters, or asset paths as metric names.

### `mm_flush`

```c
void mm_flush(MmHandle *handle);
```

Flushes in-memory log and metric buffers.  The transport thread then ships
them to the server.  Call this periodically (e.g., every 30 s) and always
before shutdown.  Safe to call with `NULL`.

## Minimal C example

```c
#include "micromegas.h"
#include <stdint.h>
#include <stdio.h>

int main(void) {
    const char *keys[]   = { "app", "version" };
    const char *values[] = { "my-tool", "1.0"  };

    MmConfig cfg = {
        .sink_url       = "http://my-ingestion-server:9000",
        .property_keys  = keys,
        .property_values = values,
        .property_count  = 2,
    };

    MmHandle *h = mm_init(&cfg);
    if (!h) { fprintf(stderr, "mm_init failed\n"); return 1; }

    mm_log(h, MM_LEVEL_INFO, "my-tool.main", "starting up");
    mm_metric_i(h, "my_tool.items_processed", "count", 42);
    mm_metric_f(h, "my_tool.latency_ms",      "ms",    7.5);

    mm_flush(h);
    mm_shutdown(h);
    return 0;
}
```

## Python / ctypes example

The Blender add-on ships a ready-to-use ctypes binding in
`blender/micromegas_blender/binding.py`.  You can adapt it for any Python
project:

```python
from micromegas_blender.binding import MicromegasLib, LEVEL_INFO

lib    = MicromegasLib("/path/to/libmicromegas_capi.so")
handle = lib.init(
    sink_url="http://my-server:9000",
    properties={"app": "my-script", "version": "1.0"},
)
lib.log(handle,   LEVEL_INFO, "my-script", "hello from Python")
lib.metric_f(handle, "latency_ms", "ms", 12.3)
lib.flush(handle)
lib.shutdown(handle)
```

## Threading model

- `mm_init` / `mm_shutdown` must be called from a single thread (the init/cleanup thread).
- `mm_log`, `mm_metric_i`, `mm_metric_f`, `mm_flush` are safe to call from any thread
  simultaneously; internal dispatch uses a per-stream `Mutex`.
- The HTTP transport runs on a dedicated OS thread spawned by `mm_init`.
  Your threads never block on network I/O.

## Platform matrix

| Target | Artifact | Notes |
|---|---|---|
| `x86_64-unknown-linux-gnu` | `libmicromegas_capi.so` | WSL2 counts as Linux |
| `x86_64-pc-windows-msvc`   | `micromegas_capi.dll`  | CRT statically linked |

macOS and ARM64 are not currently supported.
