use std::fmt::Display;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use micromegas_tracing::error;

use crate::StdError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    status_code: StatusCode,
    msg: String,
}

impl Error {
    pub fn internal(msg: impl Into<String>) -> Self {
        Error {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            msg: msg.into(),
        }
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Error {
            status_code: StatusCode::BAD_REQUEST,
            msg: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Error {
            status_code: StatusCode::NOT_FOUND,
            msg: msg.into(),
        }
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Error {
            status_code: StatusCode::CONFLICT,
            msg: msg.into(),
        }
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Error {
            status_code: StatusCode::UNAUTHORIZED,
            msg: msg.into(),
        }
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Error {
            status_code: StatusCode::FORBIDDEN,
            msg: msg.into(),
        }
    }
}

impl std::error::Error for Error {}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status_code, self.msg)
    }
}

impl From<hyper::Error> for Error {
    fn from(err: hyper::Error) -> Self {
        Self {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            msg: format!("hyper: {}", err),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            msg: format!("serde json: {}", err),
        }
    }
}

impl From<serde_qs::Error> for Error {
    fn from(err: serde_qs::Error) -> Self {
        Self {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            msg: format!("serde qs: {}", err),
        }
    }
}

impl From<crate::codegen::encoding::Error> for Error {
    fn from(err: crate::codegen::encoding::Error) -> Self {
        Self {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            msg: format!("encoding: {}", err),
        }
    }
}

impl From<StdError> for Error {
    fn from(err: StdError) -> Self {
        Self {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            msg: format!("generic: {}", err),
        }
    }
}

impl From<http::Error> for Error {
    fn from(err: http::Error) -> Self {
        Self {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            msg: format!("http: {}", err),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        if let 0..=499 = self.status_code.as_u16() {
            (self.status_code, self.msg).into_response()
        } else {
            error!("server error {}: {}", self.status_code, self.msg);

            self.status_code.into_response()
        }
    }
}

pub trait ErrorExt<T>: Sized {
    /// Converts to a server error.
    fn into_server_error(self, status_code: StatusCode) -> Result<T, Error>;

    /// Converts to an internal server error.
    fn into_internal_server_error(self) -> Result<T, Error> {
        self.into_server_error(StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// Converts to a bad request error.
    fn into_bad_request(self) -> Result<T, Error> {
        self.into_server_error(StatusCode::BAD_REQUEST)
    }
}

impl<T, E: std::error::Error> ErrorExt<T> for Result<T, E> {
    fn into_server_error(self, status_code: StatusCode) -> Result<T, Error> {
        self.map_err(|err| Error {
            status_code,
            msg: err.to_string(),
        })
    }
}
