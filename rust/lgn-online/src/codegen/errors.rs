use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("complex type `{0}` is not supported")]
    Unsupported(String),
    #[error("parsing error: {0}")]
    Parsing(String),
    #[error("utf8 error: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error(transparent)]
    Unknown(#[from] anyhow::Error),
}

impl Error {
    pub(crate) fn unsupported<T: std::fmt::Debug>(value: T) -> Self {
        Self::Unsupported(format!("{:?}", value))
    }
}

impl serde::ser::Error for Error {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Self::Unknown(anyhow::anyhow!("{}", msg))
    }
}

impl serde::de::Error for Error {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Self::Unknown(anyhow::anyhow!("{}", msg))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
