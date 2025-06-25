use crate::extended::NullExtendedQueryHandler;
use async_trait::async_trait;
use futures::sink::{Sink, SinkExt};
use micromegas::tracing::{debug, info};
use pgwire::{
    api::{
        auth::StartupHandler, copy::NoopCopyHandler, query::SimpleQueryHandler, results::Response,
        ClientInfo, ClientPortalStore, NoopErrorHandler, PgWireConnectionState,
        PgWireServerHandlers,
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

pub struct SimpleQueryH {}

#[async_trait]
impl SimpleQueryHandler for SimpleQueryH {
    /// Provide your query implementation using the incoming query string.
    async fn do_query<'a, C>(&self, _client: &mut C, query: &str) -> PgWireResult<Vec<Response<'a>>>
    where
        C: ClientInfo + ClientPortalStore + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        info!("query={query}");
        Ok(vec![])
    }
}

pub struct HandlerFactory {}

impl PgWireServerHandlers for HandlerFactory {
    type StartupHandler = StartupH;
    type SimpleQueryHandler = SimpleQueryH;
    type ExtendedQueryHandler = NullExtendedQueryHandler;
    type CopyHandler = NoopCopyHandler;
    type ErrorHandler = NoopErrorHandler;

    fn simple_query_handler(&self) -> Arc<Self::SimpleQueryHandler> {
        debug!("making simple_query_handler");
        Arc::new(SimpleQueryH {})
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
