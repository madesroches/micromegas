use micromegas::axum::{
    body::Body,
    http::{Response, StatusCode},
    response::IntoResponse,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GatewayError {
    #[error("Internal server error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response<Body> {
        let (status, message) = match &self {
            GatewayError::Internal(err) => {
                let msg = format!("{err:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
        };

        (status, message).into_response()
    }
}
