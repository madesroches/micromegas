//! API Key request decorator for HttpEventSink authentication
//!
//! Adds Bearer token authentication header to HTTP requests sent to the ingestion service.

use crate::request_decorator::{RequestDecorator, RequestDecoratorError, Result};
use async_trait::async_trait;

/// Request decorator that adds API key as Bearer token
///
/// Reads API key from environment variable `MICROMEGAS_INGESTION_API_KEY`
/// and adds it as an Authorization header to all requests.
pub struct ApiKeyRequestDecorator {
    api_key: String,
}

impl ApiKeyRequestDecorator {
    /// Create a new API key decorator from environment variable
    ///
    /// Reads `MICROMEGAS_INGESTION_API_KEY` environment variable.
    /// Returns error if environment variable is not set.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("MICROMEGAS_INGESTION_API_KEY").map_err(|_| {
            RequestDecoratorError::Permanent(
                "MICROMEGAS_INGESTION_API_KEY environment variable not set".to_string(),
            )
        })?;

        if api_key.is_empty() {
            return Err(RequestDecoratorError::Permanent(
                "MICROMEGAS_INGESTION_API_KEY is empty".to_string(),
            ));
        }

        Ok(Self { api_key })
    }

    /// Create a new API key decorator with explicit key
    ///
    /// # Arguments
    /// * `api_key` - The API key to use for authentication
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

#[async_trait]
impl RequestDecorator for ApiKeyRequestDecorator {
    async fn decorate(&self, request: &mut reqwest::Request) -> Result<()> {
        // Add Authorization header with Bearer token
        let auth_value = format!("Bearer {}", self.api_key);
        request.headers_mut().insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&auth_value).map_err(|e| {
                RequestDecoratorError::Permanent(format!("Invalid API key format: {}", e))
            })?,
        );
        Ok(())
    }
}
