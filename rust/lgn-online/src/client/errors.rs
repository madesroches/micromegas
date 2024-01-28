use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("authentication error: {0}")]
    Authentication(#[from] lgn_auth::Error),
    #[error("hyper: {0}")]
    Hyper(#[from] hyper::Error),
    #[error("grpc-web: {0}")]
    GrpcWeb(#[from] crate::grpc::web::Error),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("invalid reply: {0}")]
    InvalidReply(String),
    #[error("http: {0}")]
    Http(#[from] http::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
