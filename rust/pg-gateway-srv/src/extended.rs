use crate::{api_error, simple::execute_query, state::SharedState};
use async_trait::async_trait;
use futures::Sink;
use micromegas::{
    datafusion_postgres::{
        arrow_pg::datatypes::arrow_schema_to_pg_fields,
        pgwire::{self, api::portal::Format},
    },
    tracing::info,
};
use pgwire::{
    api::{
        ClientInfo, ClientPortalStore,
        portal::Portal,
        query::ExtendedQueryHandler,
        results::{DescribePortalResponse, DescribeStatementResponse, Response},
        stmt::{NoopQueryParser, StoredStatement},
        store::PortalStore,
    },
    error::{PgWireError, PgWireResult},
    messages::PgWireBackendMessage,
};
use std::fmt::Debug;
use std::sync::Arc;

/// Handles extended queries from PostgreSQL clients.
pub struct ExtendedQueryH {
    state: SharedState,
}

impl ExtendedQueryH {
    pub fn new(state: SharedState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl ExtendedQueryHandler for ExtendedQueryH {
    type Statement = String;
    type QueryParser = NoopQueryParser;

    fn query_parser(&self) -> Arc<Self::QueryParser> {
        info!("query_parser");
        Arc::new(NoopQueryParser {})
    }

    async fn do_describe_statement<C>(
        &self,
        _client: &mut C,
        _target: &StoredStatement<Self::Statement>,
    ) -> PgWireResult<DescribeStatementResponse>
    where
        C: ClientInfo + ClientPortalStore + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::PortalStore: PortalStore<Statement = Self::Statement>,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        info!("do_describe_statement");
        Err(api_error!(
            "ExtendedQueryHandler::do_describe_statement not implemented"
        ))
    }

    async fn do_describe_portal<C>(
        &self,
        _client: &mut C,
        target: &Portal<Self::Statement>,
    ) -> PgWireResult<DescribePortalResponse>
    where
        C: ClientInfo + ClientPortalStore + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::PortalStore: PortalStore<Statement = Self::Statement>,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        info!(
            "do_describe_portal name={} statement={}",
            target.name, target.statement.statement
        );
        let client_factory = self
            .state
            .lock()
            .await
            .flight_client_factory()
            .map_err(api_error!())?;
        let mut flight_client = client_factory.make_client().await.map_err(api_error!())?;
        let prepared = flight_client
            .prepare_statement(target.statement.statement.clone())
            .await
            .map_err(api_error!())?;
        let fields = arrow_schema_to_pg_fields(&prepared.schema, &Format::UnifiedText)
            .map_err(api_error!())?;
        Ok(DescribePortalResponse::new(fields))
    }

    async fn do_query<'a, C>(
        &self,
        _client: &mut C,
        portal: &Portal<Self::Statement>,
        _max_rows: usize,
    ) -> PgWireResult<Response<'a>>
    where
        C: ClientInfo + ClientPortalStore + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::PortalStore: PortalStore<Statement = Self::Statement>,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        info!("do_query");
        //todo: support max_rows
        execute_query(&self.state, &portal.statement.statement).await
    }
}
