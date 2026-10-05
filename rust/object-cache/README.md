# micromegas-object-cache

A range-aware read cache for [`object_store`](https://docs.rs/object_store). The crate has two halves:

- the **range cache engine**, with memory and (behind the `foyer` feature) [foyer](https://docs.rs/foyer) backends, used by the cache service;
- `CacheClientStore`, an `object_store::ObjectStore` client that routes byte-range reads through a shared cache service, with a circuit breaker, and falls back to the origin store when the cache is unavailable.

Use it when several processes read overlapping ranges of the same immutable objects, such as Parquet files on S3. It assumes objects are write-once: there is no invalidation, so an object must never change under the same path.

## Example

```rust
use micromegas_object_cache::client::CacheClientStore;
use object_store::{ObjectStore, memory::InMemory};
use std::sync::Arc;

let origin: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
let store = CacheClientStore::new(
    "http://127.0.0.1:8080".to_string(), // cache service base URL
    None,                                // optional API key
    origin,                              // fallback when the cache is unavailable
);
// `store` implements `ObjectStore`; pass it wherever one is expected.
let _store: Arc<dyn ObjectStore> = Arc::new(store);
```

## Documentation

- [API documentation on docs.rs](https://docs.rs/micromegas-object-cache)
- [Caching architecture](https://micromegas.info/docs/architecture/caching/)
- [Object cache administration](https://micromegas.info/docs/admin/object-cache/)
- [GitHub Repository](https://github.com/madesroches/micromegas)
