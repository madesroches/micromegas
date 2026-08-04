use super::scoped_memory_pool::ScopedMemoryPool;
use anyhow::Result;
use datafusion::execution::{
    memory_pool::{GreedyMemoryPool, MemoryPool, TrackConsumersPool, UnboundedMemoryPool},
    runtime_env::{RuntimeEnv, RuntimeEnvBuilder},
};
use std::{num::NonZeroUsize, sync::Arc};

/// Applies `mb` (megabytes) as the disk manager's max temp directory size, in bytes.
/// Factored out of `make_runtime_env` so it can be unit-tested without touching env vars.
pub fn apply_max_temp_directory_mb(builder: RuntimeEnvBuilder, mb: u64) -> RuntimeEnvBuilder {
    builder.with_max_temp_directory_size(mb * 1024 * 1024)
}

/// Parses the raw `MICROMEGAS_DATAFUSION_MAX_TEMP_DIRECTORY_MB` value (as returned by
/// `std::env::var`) into an optional MB budget: `Ok(None)` when the variable is unset,
/// `Err` when it is set to something that doesn't parse as `u64`. Factored out of
/// `make_runtime_env` so the parse-failure path can be unit-tested without setting a real
/// process env var, which would race with other tests running in parallel in the same
/// process.
pub fn parse_max_temp_directory_mb(var: Result<String, std::env::VarError>) -> Result<Option<u64>> {
    match var {
        Ok(mb_str) => Ok(Some(mb_str.parse::<u64>()?)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err @ std::env::VarError::NotUnicode(_)) => Err(err.into()),
    }
}

/// Creates a new DataFusion `RuntimeEnv` with a configurable memory pool.
pub fn make_runtime_env() -> Result<RuntimeEnv> {
    let nb_top_consumers = NonZeroUsize::new(5).unwrap();
    let pool: Arc<dyn MemoryPool> = match std::env::var("MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB") {
        Ok(mb_str) => {
            let bytes = mb_str.parse::<usize>()? * 1024 * 1024;
            Arc::new(TrackConsumersPool::new(
                GreedyMemoryPool::new(bytes),
                nb_top_consumers,
            ))
        }
        Err(_) => Arc::new(TrackConsumersPool::new(
            UnboundedMemoryPool::default(),
            nb_top_consumers,
        )),
    };
    let mut builder = RuntimeEnvBuilder::new().with_memory_pool(pool);
    if let Some(mb) =
        parse_max_temp_directory_mb(std::env::var("MICROMEGAS_DATAFUSION_MAX_TEMP_DIRECTORY_MB"))?
    {
        builder = apply_max_temp_directory_mb(builder, mb);
    }
    Ok(builder.build()?)
}

/// Builds a `RuntimeEnv` that reuses `shared`'s disk manager, caches and object-store
/// registry but installs `scoped_pool` (already wrapping `shared`'s memory pool) as its
/// memory pool. Takes the pool as a parameter, rather than constructing it internally, so
/// callers can hand the (infallible) pool to `QueryAuditState` before this fallible step runs.
pub fn scoped_runtime(
    shared: &RuntimeEnv,
    scoped_pool: Arc<ScopedMemoryPool>,
) -> Result<Arc<RuntimeEnv>> {
    Ok(Arc::new(
        RuntimeEnvBuilder::from_runtime_env(shared)
            .with_memory_pool(scoped_pool)
            .build()?,
    ))
}
