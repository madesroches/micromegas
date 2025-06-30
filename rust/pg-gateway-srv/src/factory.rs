use crate::simple::SimpleQueryH;
use crate::startup::StartupH;
use crate::state::ConnectionState;
use crate::{extended::NullExtendedQueryHandler, state::SharedState};
use pgwire::api::{copy::NoopCopyHandler, NoopErrorHandler, PgWireServerHandlers};
use std::sync::Arc;
use tokio::sync::Mutex;

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
    type StartupHandler = StartupH;
    type SimpleQueryHandler = SimpleQueryH;
    type ExtendedQueryHandler = NullExtendedQueryHandler;
    type CopyHandler = NoopCopyHandler;
    type ErrorHandler = NoopErrorHandler;

    fn simple_query_handler(&self) -> Arc<Self::SimpleQueryHandler> {
        Arc::new(SimpleQueryH::new(self.state.clone()))
    }

    fn extended_query_handler(&self) -> Arc<Self::ExtendedQueryHandler> {
        Arc::new(NullExtendedQueryHandler {})
    }

    fn startup_handler(&self) -> Arc<Self::StartupHandler> {
        Arc::new(StartupH::new(self.state.clone()))
    }

    fn copy_handler(&self) -> Arc<Self::CopyHandler> {
        Arc::new(NoopCopyHandler {})
    }

    fn error_handler(&self) -> Arc<Self::ErrorHandler> {
        Arc::new(NoopErrorHandler {})
    }
}
