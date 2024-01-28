#[cfg(feature = "aws")]
mod aws_dynamo_db_validation;
mod errors;
mod memory_validation;
mod request_authorizer;
#[cfg(feature = "ttl")]
mod ttl_cache_validation;

use std::{fmt::Formatter, sync::Arc};

use async_trait::async_trait;
#[cfg(feature = "aws")]
pub use aws_dynamo_db_validation::AwsDynamoDbValidation;
pub use errors::{Error, Result};
pub use memory_validation::MemoryValidation;
pub use request_authorizer::RequestAuthorizer;
#[cfg(feature = "ttl")]
pub use ttl_cache_validation::TtlCacheValidation;

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub struct ApiKey(String);

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiKey(**redacted**)")
    }
}

impl From<String> for ApiKey {
    fn from(api_key: String) -> Self {
        Self(api_key)
    }
}

/// A trait for types that can validate API keys.
#[async_trait]
pub trait ApiKeyValidator {
    async fn validate_api_key(&self, api_key: ApiKey) -> Result<()>;
}

/// Blanket implementation for &T.
#[async_trait]
impl<T: ApiKeyValidator + Send + Sync + ?Sized> ApiKeyValidator for &T {
    async fn validate_api_key(&self, api_key: ApiKey) -> Result<()> {
        (**self).validate_api_key(api_key).await
    }
}

/// Blanket implementation for Arc<T>.
#[async_trait]
impl<T: ApiKeyValidator + Send + Sync> ApiKeyValidator for Arc<T> {
    async fn validate_api_key(&self, api_key: ApiKey) -> Result<()> {
        (**self).validate_api_key(api_key).await
    }
}
