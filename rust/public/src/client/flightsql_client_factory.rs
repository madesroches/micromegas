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
    token: String,
}

impl BearerFlightSQLClientFactory {
    /// Creates a new `BearerFlightSQLClientFactory`.
    ///
    /// # Arguments
    ///
    /// * `token` - The bearer token to use for authentication.
    pub const fn new(token: String) -> Self {
        Self { token }
    }
}

#[async_trait]
impl FlightSQLClientFactory for BearerFlightSQLClientFactory {
    async fn make_client(&self) -> Result<Client> {
        let flight_url = std::env::var("MICROMEGAS_FLIGHTSQL_URL")
            .with_context(|| "error reading MICROMEGAS_FLIGHTSQL_URL environment variable")?
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
        client
            .inner_mut()
            .set_header(http::header::AUTHORIZATION.as_str(), self.token.clone());

        Ok(client)
    }
}
