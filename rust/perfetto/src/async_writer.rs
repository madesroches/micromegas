/// Trait for async write operations to abstract the underlying data sink
#[async_trait::async_trait]
pub trait AsyncWriter {
    async fn write(&mut self, buf: &[u8]) -> anyhow::Result<()>;
    async fn flush(&mut self) -> anyhow::Result<()>;
}
