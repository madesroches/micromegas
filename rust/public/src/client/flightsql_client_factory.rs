use super::flightsql_client::Client;
use anyhow::{Context, Result};
use async_trait::async_trait;
use http::Uri;
use tonic::transport::{Channel, ClientTlsConfig};

#[async_trait]
pub trait FlightSQLClientFactory: Send + Sync {
    async fn make_client(&self) -> Result<Client>;
}

pub struct DefaultFlightSQLClientFactory {}

impl DefaultFlightSQLClientFactory {
    pub const fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl FlightSQLClientFactory for DefaultFlightSQLClientFactory {
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
        let client = Client::new(channel);
        // client.inner_mut().set_header(
        //     http::header::AUTHORIZATION.as_str(),
        //     token,
        // );

        Ok(client)
    }
}
