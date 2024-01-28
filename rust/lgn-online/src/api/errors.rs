use thiserror::Error;
use tonic::codegen::StdError;

#[derive(Error, Debug)]
pub enum Error {
    #[error("failed to run server: {0}")]
    RunServerFailure(String),
    #[error("authentication error: {0}")]
    Authentication(lgn_auth::Error),
    #[error(transparent)]
    Other(#[from] StdError),
}

pub type Result<T> = std::result::Result<T, Error>;
