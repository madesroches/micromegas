use thiserror::Error;

pub type StdError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("authentication error: {0}")]
    Authentication(#[from] lgn_auth::Error),
    #[error("configuration error: {0}")]
    Config(#[from] lgn_config::Error),
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("address parsing error: {0}")]
    AddrParse(#[from] std::net::AddrParseError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
