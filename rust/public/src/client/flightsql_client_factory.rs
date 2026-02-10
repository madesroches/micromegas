use super::flightsql_client::Client;
use anyhow::{Context, Result};
use async_trait::async_trait;
use http::Uri;
use tonic::transport::{Channel, ClientTlsConfig};

/// A trait for creating FlightSQL clients.
#[async_trait]
pub trait FlightSQLClientFactory: Send + Sync {
    async fn make_client(&self) -> Result<Client>;
}

/// A FlightSQL client factory that uses a bearer token for authentication.
pub struct BearerFlightSQLClientFactory {
    url: String,
    token: String,
    client_type: Option<String>,
}

impl BearerFlightSQLClientFactory {
    /// Creates a new `BearerFlightSQLClientFactory`.
    ///
    /// # Arguments
    ///
    /// * `url` - The FlightSQL server URL.
    /// * `token` - The bearer token to use for authentication.
    pub fn new(url: String, token: String) -> Self {
        Self {
            url,
            token,
            client_type: None,
        }
    }

    /// Creates a new `BearerFlightSQLClientFactory` with a specific client type identifier.
    ///
    /// # Arguments
    ///
    /// * `url` - The FlightSQL server URL.
    /// * `token` - The bearer token to use for authentication.
    /// * `client_type` - The client type identifier (e.g., "web", "cli", "python").
    pub fn new_with_client_type(url: String, token: String, client_type: String) -> Self {
        Self {
            url,
            token,
            client_type: Some(client_type),
        }
    }

    /// Creates a new `BearerFlightSQLClientFactory` that reads the URL from the
    /// `MICROMEGAS_FLIGHTSQL_URL` environment variable.
    pub fn from_env(token: String) -> Result<Self> {
        let url = std::env::var("MICROMEGAS_FLIGHTSQL_URL")
            .with_context(|| "error reading MICROMEGAS_FLIGHTSQL_URL environment variable")?;
        Ok(Self {
            url,
            token,
            client_type: None,
        })
    }

    /// Creates a new `BearerFlightSQLClientFactory` that reads the URL from the
    /// `MICROMEGAS_FLIGHTSQL_URL` environment variable, with a client type.
    pub fn from_env_with_client_type(token: String, client_type: String) -> Result<Self> {
        let url = std::env::var("MICROMEGAS_FLIGHTSQL_URL")
            .with_context(|| "error reading MICROMEGAS_FLIGHTSQL_URL environment variable")?;
        Ok(Self {
            url,
            token,
            client_type: Some(client_type),
        })
    }
}

#[async_trait]
impl FlightSQLClientFactory for BearerFlightSQLClientFactory {
    async fn make_client(&self) -> Result<Client> {
        let flight_url = self
            .url
            .parse::<Uri>()
            .with_context(|| "parsing flightsql url")?;
        let tls_config = ClientTlsConfig::new().with_native_roots();
        let channel = Channel::builder(flight_url)
            .tls_config(tls_config)
            .with_context(|| "tls_config")?
            .connect()
            .await
            .with_context(|| "connecting grpc channel")?;
        let mut client = Client::new(channel);
        let auth_value = if self.token.starts_with("Bearer ") {
            self.token.clone()
        } else {
            format!("Bearer {}", self.token)
        };

        client
            .inner_mut()
            .set_header(http::header::AUTHORIZATION.as_str(), auth_value);

        // Set client type header if provided
        if let Some(client_type) = &self.client_type {
            client
                .inner_mut()
                .set_header("x-client-type", client_type.clone());
        }

        // Preserve dictionary encoding for bandwidth efficiency
        client.inner_mut().set_header("preserve_dictionary", "true");

        Ok(client)
    }
}
