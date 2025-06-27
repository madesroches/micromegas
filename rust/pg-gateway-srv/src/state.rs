use anyhow::{Context, Result};
use micromegas::client::flightsql_client_factory::FlightSQLClientFactory;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct ConnectionState {
    flight_client_factory: Option<Arc<dyn FlightSQLClientFactory>>,
}

impl ConnectionState {
    pub fn new() -> Self {
        Self {
            flight_client_factory: None,
        }
    }

    pub fn set_factory(&mut self, factory: Arc<dyn FlightSQLClientFactory>) {
        self.flight_client_factory = Some(factory);
    }

    pub fn flight_client_factory(&self) -> Result<Arc<dyn FlightSQLClientFactory>> {
        self.flight_client_factory
            .clone()
            .with_context(|| "flightsql connection unavailable")
    }
}

pub type SharedState = Arc<Mutex<ConnectionState>>;
