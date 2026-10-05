# micromegas-tracing

Low-overhead logs, metrics and spans for Rust applications and game engines. Recording an event costs about 20 ns, with predictable performance on the critical path of execution. Originally designed for video game engines, it is the instrumentation library of [Micromegas](https://micromegas.info/).

## Usage

```toml
[dependencies]
micromegas-tracing = "0.32"
```

```rust
use micromegas_tracing::prelude::*;

#[span_fn]
async fn fetch_user(id: u64) {
    span_scope!("lookup");
    info!("fetching user {id}");
}
```

`#[span_fn]` records the wall-clock time of sync and async functions, `span_scope!` times a block, and `info!` (and its siblings) emit log entries. See the crate documentation for metrics, sinks and initialization.

## Documentation

- [API documentation on docs.rs](https://docs.rs/micromegas-tracing)
- [Getting Started Guide](https://micromegas.info/docs/getting-started/)
- [Unreal Engine integration](https://micromegas.info/docs/unreal/)
- [GitHub Repository](https://github.com/madesroches/micromegas)
