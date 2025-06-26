use crate::extended::NullExtendedQueryHandler;
use crate::simple::SimpleQueryH;
use async_trait::async_trait;
use futures::sink::{Sink, SinkExt};
use micromegas::{
    client::flightsql_client_factory::{DefaultFlightSQLClientFactory, FlightSQLClientFactory},
    tracing::{debug, info},
};
use pgwire::{
    api::{
        auth::StartupHandler, copy::NoopCopyHandler, ClientInfo, NoopErrorHandler,
        PgWireConnectionState, PgWireServerHandlers,
    },
    error::{PgWireError, PgWireResult},
    messages::{
        response::{ReadyForQuery, TransactionStatus},
        startup::Authentication,
        PgWireBackendMessage, PgWireFrontendMessage,
    },
};
use std::{fmt::Debug, sync::Arc};

pub struct StartupH {}

#[async_trait]
impl StartupHandler for StartupH {
    /// A generic frontend message callback during startup phase.
    async fn on_startup<C>(
        &self,
        client: &mut C,
        message: PgWireFrontendMessage,
    ) -> PgWireResult<()>
    where
        C: ClientInfo + Sink<PgWireBackendMessage> + Unpin + Send,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        info!("on_startup message={message:?}");
        client.set_state(PgWireConnectionState::ReadyForQuery);
        client
            .send(PgWireBackendMessage::Authentication(Authentication::Ok))
            .await?;
        client
            .send(PgWireBackendMessage::ReadyForQuery(ReadyForQuery::new(
                TransactionStatus::Idle,
            )))
            .await?;
        Ok(())
    }
}

pub struct ConnectionResources {
    flight_client_factory: Arc<dyn FlightSQLClientFactory>,
}

impl ConnectionResources {
    pub fn new() -> Self {
        let flight_client_factory = Arc::new(DefaultFlightSQLClientFactory::new());
        Self {
            flight_client_factory,
        }
    }
}

impl PgWireServerHandlers for ConnectionResources {
    type StartupHandler = StartupH;
    type SimpleQueryHandler = SimpleQueryH;
    type ExtendedQueryHandler = NullExtendedQueryHandler;
    type CopyHandler = NoopCopyHandler;
    type ErrorHandler = NoopErrorHandler;

    fn simple_query_handler(&self) -> Arc<Self::SimpleQueryHandler> {
        debug!("making simple_query_handler");
        Arc::new(SimpleQueryH::new(self.flight_client_factory.clone()))
    }

    fn extended_query_handler(&self) -> Arc<Self::ExtendedQueryHandler> {
        debug!("making extended_query_handler");
        Arc::new(NullExtendedQueryHandler {})
    }

    fn startup_handler(&self) -> Arc<Self::StartupHandler> {
        debug!("making startup_handler");
        Arc::new(StartupH {})
    }

    fn copy_handler(&self) -> Arc<Self::CopyHandler> {
        debug!("making copy_handler");
        Arc::new(NoopCopyHandler {})
    }

    fn error_handler(&self) -> Arc<Self::ErrorHandler> {
        debug!("making error_handler");
        Arc::new(NoopErrorHandler {})
    }
}
