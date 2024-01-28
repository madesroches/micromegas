use std::path::PathBuf;

use thiserror::Error;

use crate::openapi_loader::{OpenApiRef, OpenApiRefLocation};

#[derive(Error, Debug)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde_json: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("serde_yaml: {0}")]
    SerdeYaml(#[from] serde_yaml::Error),
    #[error("askama: {0}")]
    Askama(#[from] askama::Error),
    #[error("uri: {0}")]
    Uri(#[from] http::uri::InvalidUri),
    #[error("rust format: {0}")]
    RustFormat(#[from] rust_format::Error),
    #[error("invalid typescript filename \"index\" is reserved")]
    TypeScriptFilename,
    #[error("typescript format: {0}")]
    TypeScriptFormat(anyhow::Error),
    #[error("at {0}: {1} are invalid")]
    Invalid(OpenApiRef, String),
    #[error("broken reference: {0}")]
    BrokenReference(OpenApiRef),
    #[error("path is missing operation id: {0}")]
    MissingOperationID(OpenApiRef),
    #[error("document already exists at location: {0}")]
    DocumentAlreadyExists(OpenApiRefLocation),
    #[error("at {0}: {1} are not supported")]
    Unsupported(OpenApiRef, String),
    #[error("invalid OpenAPI reference: {0} (did you forget the `#`?)")]
    InvalidOpenApiRef(String),
    #[error("invalid OpenAPI reference location: {0}")]
    InvalidOpenApiRefLocation(String),
    #[error("invalid json pointer: {0}")]
    InvalidJsonPointer(String),
    #[error("invalid header name: {0}")]
    InvalidHeaderName(String),
    #[error("unsupported media-type: {0}")]
    UnsupportedMediaType(String),
    #[error("unsupported type: {0}")]
    UnsupportedType(String),
    #[error("unsupported method: {0}")]
    UnsupportedMethod(String),
    #[error("the document `{document_path}` is out of the root `{root}`")]
    DocumentOutOfRoot {
        document_path: PathBuf,
        root: PathBuf,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<Error> for askama::Error {
    fn from(err: Error) -> Self {
        askama::Error::Custom(Box::new(err))
    }
}
