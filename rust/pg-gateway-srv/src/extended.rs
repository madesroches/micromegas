use anyhow::anyhow;
use async_trait::async_trait;
use futures::Sink;
use micromegas::tracing::info;
use pgwire::{
    api::{
        portal::Portal,
        query::ExtendedQueryHandler,
        results::{DescribePortalResponse, DescribeStatementResponse, Response},
        stmt::{NoopQueryParser, StoredStatement},
        store::PortalStore,
        ClientInfo, ClientPortalStore,
    },
    error::{PgWireError, PgWireResult},
    messages::PgWireBackendMessage,
};
use std::fmt::Debug;
use std::sync::Arc;

pub struct NullExtendedQueryHandler {}

#[async_trait]
impl ExtendedQueryHandler for NullExtendedQueryHandler {
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
        Err(PgWireError::ApiError(anyhow!("not implemented").into()))
    }

    async fn do_describe_portal<C>(
        &self,
        _client: &mut C,
        _target: &Portal<Self::Statement>,
    ) -> PgWireResult<DescribePortalResponse>
    where
        C: ClientInfo + ClientPortalStore + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::PortalStore: PortalStore<Statement = Self::Statement>,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        info!("do_describe_portal");
        Err(PgWireError::ApiError(anyhow!("not implemented").into()))
    }

    async fn do_query<'a, C>(
        &self,
        _client: &mut C,
        _portal: &Portal<Self::Statement>,
        _max_rows: usize,
    ) -> PgWireResult<Response<'a>>
    where
        C: ClientInfo + ClientPortalStore + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::PortalStore: PortalStore<Statement = Self::Statement>,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        info!("do_query");
        Err(PgWireError::ApiError(anyhow!("not implemented").into()))
    }
}
