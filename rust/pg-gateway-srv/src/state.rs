use anyhow::{Context, Result};
use micromegas::client::flightsql_client_factory::FlightSQLClientFactory;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Represents the connection state for a PostgreSQL client.
pub struct ConnectionState {
    flight_client_factory: Option<Arc<dyn FlightSQLClientFactory>>,
}

impl ConnectionState {
    pub fn new() -> Self {
        Self {
            flight_client_factory: None,
        }
    }

    /// Sets the FlightSQL client factory.
    pub fn set_factory(&mut self, factory: Arc<dyn FlightSQLClientFactory>) {
        self.flight_client_factory = Some(factory);
    }

    /// Returns the FlightSQL client factory.
    pub fn flight_client_factory(&self) -> Result<Arc<dyn FlightSQLClientFactory>> {
        self.flight_client_factory
            .clone()
            .with_context(|| "flightsql connection unavailable")
    }
}

/// A shared, mutable reference to `ConnectionState`.
pub type SharedState = Arc<Mutex<ConnectionState>>;
