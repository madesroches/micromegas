# Micromegas C ABI Crate

`micromegas-capi` is a thin C ABI over the Micromegas telemetry producer stack. It
lets any non-Rust process — a Python script, a C/C++ tool, a game-engine plugin —
emit logs and metrics into Micromegas without building a Rust project.

The crate builds as a `cdylib`/`staticlib`/`rlib` exposing flat `extern "C"`
functions (`mm_init`, `mm_log`, `mm_metric_i`, `mm_metric_f`, `mm_flush`,
`mm_shutdown`). The transport runs on its own OS thread inside the library, so the
calling thread never blocks on the network, and all functions are safe to call from
any thread.

## Building

```bash
cd rust/
cargo build --package micromegas-capi --release
# Linux:   rust/target/release/libmicromegas_capi.{so,a}
# Windows: rust/target/<triple>/release/micromegas_capi.{dll,lib}
```

## Usage

Include `include/micromegas.h` and load the library (via `dlopen`/`LoadLibrary`/
ctypes or static linking):

```c
#include "micromegas.h"

MmConfig cfg = { .sink_url = "http://localhost:9000", .property_count = 0 };
MmHandle *h = mm_init(&cfg);          /* NULL on failure */
mm_log(h, MM_LEVEL_INFO, "myapp", "started");
mm_metric_f(h, "frame_time", "ms", 16.7);
mm_flush(h);
mm_shutdown(h);                        /* flushes and frees */
```

`sink_url` may be NULL to fall back to `MICROMEGAS_TELEMETRY_URL`. Authentication is
always read from the environment (`MICROMEGAS_INGESTION_API_KEY` or the OIDC vars).
Each unique metric `(name, unit)` pair is interned permanently, so keep metric names
low-cardinality — never pass unbounded values (session IDs, asset names) as names.

The C header is generated with cbindgen (`cbindgen.toml`) and kept in sync with
`src/lib.rs`.

## Documentation

- 📖 [Native / Non-Rust Integration guide](https://micromegas.info/docs/native/)
- 📖 [Complete Documentation](https://micromegas.info/)
- 💻 [GitHub Repository](https://github.com/madesroches/micromegas)
