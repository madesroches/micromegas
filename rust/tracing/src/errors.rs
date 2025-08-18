//! Error types and handling for tracing operations

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Event dispatch already initialized")]
    AlreadyInitialized(),
}

pub type Result<T> = std::result::Result<T, Error>;
