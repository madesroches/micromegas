use std::fmt::Display;

use async_trait::async_trait;

/// An error that can occur when decorating a request.
///
/// Depending on the type of error, the request may be retried at a later time.
#[derive(Debug)]
pub enum RequestDecoratorError {
    /// A permanent error occurred while decorating the request.
    ///
    /// No further attempts to decorate the request should be made.
    Permanent(String),

    /// A transient error occurred while decorating the request.
    ///
    /// The request may be retried at a later time.
    Transient(String),
}

impl Display for RequestDecoratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestDecoratorError::Permanent(msg) => {
                write!(f, "failed to decorate request: permanent error: {msg}")
            }
            RequestDecoratorError::Transient(msg) => {
                write!(f, "failed to decorate request: transient error: {msg}")
            }
        }
    }
}

impl std::error::Error for RequestDecoratorError {}

impl From<RequestDecoratorError> for tokio_retry2::RetryError<anyhow::Error> {
    fn from(value: RequestDecoratorError) -> Self {
        match value {
            RequestDecoratorError::Permanent(msg) => tokio_retry2::RetryError::permanent(
                anyhow::anyhow!("failed to decorate request: {msg}"),
            ),
            RequestDecoratorError::Transient(msg) => tokio_retry2::RetryError::transient(
                anyhow::anyhow!("failed to decorate request: {msg}"),
            ),
        }
    }
}

/// A result type for request decorators.
pub type Result<T> = std::result::Result<T, RequestDecoratorError>;

#[async_trait] // otherwise we get: cannot be made into an object
pub trait RequestDecorator: Send {
    /// Decorates the given `reqwest::Request`.
    ///
    /// This function can modify the request (e.g., add headers, sign the request)
    /// before it is sent.
    ///
    /// # Arguments
    ///
    /// * `request` - A mutable reference to the `reqwest::Request` to decorate.
    async fn decorate(&self, request: &mut reqwest::Request) -> Result<()>;
}

pub struct TrivialRequestDecorator {}

#[async_trait]
impl RequestDecorator for TrivialRequestDecorator {
    async fn decorate(&self, _request: &mut reqwest::Request) -> Result<()> {
        Ok(())
    }
}
