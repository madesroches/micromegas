use anyhow::{Context, Result};

/// The two env vars every lake-backed role needs.
#[derive(Debug, Clone)]
pub struct DataLakeConfig {
    pub sql_connection_string: String,
    pub object_store_uri: String,
}

impl DataLakeConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            sql_connection_string: std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
                .context("reading MICROMEGAS_SQL_CONNECTION_STRING")?,
            object_store_uri: std::env::var("MICROMEGAS_OBJECT_STORE_URI")
                .context("reading MICROMEGAS_OBJECT_STORE_URI")?,
        })
    }
}
