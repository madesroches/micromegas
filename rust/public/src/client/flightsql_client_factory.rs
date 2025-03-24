use super::flightsql_client::Client;
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait FlightSQLClientFactory: Send {
    async fn make_client(&self) -> Result<Client>;
}
