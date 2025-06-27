use crate::simple::SimpleQueryH;
use crate::state::ConnectionState;
use crate::{extended::NullExtendedQueryHandler, state::SharedState};
use async_trait::async_trait;
use futures::sink::{Sink, SinkExt};
use micromegas::client::flightsql_client_factory::BearerFlightSQLClientFactory;
use micromegas::tracing::info;
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
use tokio::sync::Mutex;

pub struct StartupH {
    state: SharedState,
}

impl StartupH {
    pub fn new(state: SharedState) -> Self {
        Self { state }
    }
}

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
        self.state
            .lock()
            .await
            .set_factory(Arc::new(BearerFlightSQLClientFactory::new("".into())));
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
