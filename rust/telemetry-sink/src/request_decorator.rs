use anyhow::Result;
use async_trait::async_trait;

#[async_trait] // otherwise we get: cannot be made into an object
pub trait RequestDecorator: Send {
    async fn decorate(&self, request: &mut reqwest::Request) -> Result<()>;
}

pub struct TrivialRequestDecorator {}

#[async_trait]
impl RequestDecorator for TrivialRequestDecorator {
    async fn decorate(&self, _request: &mut reqwest::Request) -> Result<()> {
        Ok(())
    }
}
