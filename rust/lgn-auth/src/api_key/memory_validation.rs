use async_trait::async_trait;

use super::{Error, Result};

use super::{ApiKey, ApiKeyValidator};

pub struct MemoryValidation {
    api_keys: Vec<ApiKey>,
}

impl MemoryValidation {
    pub fn new(api_keys: Vec<ApiKey>) -> Self {
        Self { api_keys }
    }
}

#[async_trait]
impl ApiKeyValidator for MemoryValidation {
    async fn validate_api_key(&self, api_key: ApiKey) -> Result<()> {
        if self.api_keys.contains(&api_key) {
            Ok(())
        } else {
            Err(Error::InvalidApiKey(api_key))
        }
    }
}
