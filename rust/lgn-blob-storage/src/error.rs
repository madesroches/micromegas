use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("no blob exists with the hash: {0}")]
    NoSuchBlob(String),
    #[error("{context}: {source}")]
    Other {
        #[source]
        source: anyhow::Error,
        context: String,
    },
}

impl Error {
    pub fn forward_with_context(err: impl Into<anyhow::Error>, context: impl Into<String>) -> Self {
        Self::Other {
            source: err.into(),
            context: context.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
