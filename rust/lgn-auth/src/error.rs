use std::time::SystemTime;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("invalid authorization URL: {0}")]
    InvalidAuthorizationUrl(String),

    #[error("internal server error: {0}")]
    InternalServer(hyper::Error),

    #[error("failed to execute the interactive login process: {0}")]
    InteractiveProcess(std::io::Error),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("token expired: exp({exp:?}) < now({now:?})")]
    TokenExpired { exp: SystemTime, now: SystemTime },

    #[error("configuration error: {0}")]
    Config(#[from] lgn_config::Error),

    #[error("configuration error: {0}")]
    CustomConfig(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
