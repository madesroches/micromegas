# Micromegas Object Cache Crate

This crate provides the object range cache for the Micromegas observability platform: a range-aware read cache engine over an origin object store (used by the cache service) and `CacheClientStore`, an `object_store::ObjectStore` client that routes byte-range reads through the cache service and falls back to the origin on a miss.

## Documentation

- 📖 [Complete Documentation](https://micromegas.info/)
- 🏗️ [Architecture Overview](https://micromegas.info/docs/architecture/)
- 💻 [GitHub Repository](https://github.com/madesroches/micromegas)
