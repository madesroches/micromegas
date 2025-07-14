use crate::simple::SimpleQueryH;
use crate::startup::StartupH;
use crate::state::ConnectionState;
use crate::{extended::ExtendedQueryH, state::SharedState};
use micromegas::datafusion_postgres::pgwire;
use micromegas::datafusion_postgres::pgwire::api::auth::StartupHandler;
use micromegas::datafusion_postgres::pgwire::api::query::{
    ExtendedQueryHandler, SimpleQueryHandler,
};
use pgwire::api::PgWireServerHandlers;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A factory for creating PostgreSQL protocol handlers.
pub struct HandlerFactory {
    state: SharedState,
}

impl HandlerFactory {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ConnectionState::new())),
        }
    }
}

impl PgWireServerHandlers for HandlerFactory {
    fn simple_query_handler(&self) -> Arc<impl SimpleQueryHandler> {
        Arc::new(SimpleQueryH::new(self.state.clone()))
    }

    fn extended_query_handler(&self) -> Arc<impl ExtendedQueryHandler> {
        Arc::new(ExtendedQueryH::new(self.state.clone()))
    }

    fn startup_handler(&self) -> Arc<impl StartupHandler> {
        Arc::new(StartupH::new(self.state.clone()))
    }
}
