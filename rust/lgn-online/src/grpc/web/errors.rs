use std::convert::Infallible;

use thiserror::Error;
use tonic::codegen::StdError;

#[derive(Error, Debug)]
pub enum Error {
    #[error("invalid gRPC-web body: {0}")]
    InvalidGrpcWebBody(String),
    #[error("unsupported gRPC protocol: {0}")]
    UnsupportedGrpcProtocol(String),
    #[error(transparent)]
    Other(#[from] StdError),
    #[error(transparent)]
    Never(Infallible),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
