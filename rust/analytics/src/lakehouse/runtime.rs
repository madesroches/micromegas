use anyhow::Result;
use datafusion::execution::{
    memory_pool::{GreedyMemoryPool, MemoryPool, TrackConsumersPool, UnboundedMemoryPool},
    runtime_env::{RuntimeEnv, RuntimeEnvBuilder},
};
use std::{num::NonZeroUsize, sync::Arc};

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
    Ok(RuntimeEnvBuilder::new().with_memory_pool(pool).build()?)
}
