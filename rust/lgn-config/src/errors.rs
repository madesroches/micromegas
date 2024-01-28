use thiserror::Error;

/// An error type.
#[derive(Error, Debug)]
pub enum Error {
    #[error("config error: {0}")]
    Config(#[from] config::ConfigError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// A result type.
pub type Result<T, E = Error> = std::result::Result<T, E>;
