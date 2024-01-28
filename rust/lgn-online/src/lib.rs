//! Legion online crate.

// crate-specific lint exceptions:
#![allow(clippy::implicit_hasher, clippy::missing_errors_doc)]

pub mod api;
pub mod client;
pub mod cloud;
pub mod codegen;
pub mod grpc;
pub mod server;

mod config;
mod errors;

pub use config::{AuthenticationConfig, Config, OAuthClientConfig, SignatureValidationConfig};
pub use errors::{Error, Result, StdError};
