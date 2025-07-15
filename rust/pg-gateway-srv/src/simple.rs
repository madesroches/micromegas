use async_stream::try_stream;
use async_trait::async_trait;
use futures::Sink;
use futures::stream::StreamExt;
use micromegas::datafusion_postgres::arrow_pg::datatypes::{
    arrow_schema_to_pg_fields, encode_recordbatch,
};
use micromegas::datafusion_postgres::pgwire;
use micromegas::datafusion_postgres::pgwire::api::portal::Format;
use micromegas::tracing::info;
use pgwire::api::results::QueryResponse;
use pgwire::{
    api::{ClientInfo, ClientPortalStore, query::SimpleQueryHandler, results::Response},
    error::{PgWireError, PgWireResult},
    messages::PgWireBackendMessage,
};
use std::fmt::Debug;
use std::sync::Arc;

use crate::state::SharedState;

/// Handles simple queries from PostgreSQL clients.
pub struct SimpleQueryH {
    state: SharedState,
}

impl SimpleQueryH {
    pub fn new(state: SharedState) -> Self {
        Self { state }
    }
}

/// Executes a SQL query against the FlightSQL server.
pub async fn execute_query<'a>(state: &SharedState, sql: &str) -> PgWireResult<Response<'a>> {
    info!("sql={sql}");
    let client_factory = state
        .lock()
        .await
        .flight_client_factory()
        .map_err(|e| PgWireError::ApiError(e.into()))?;
    let mut flight_client = client_factory
        .make_client()
        .await
        .map_err(|e| PgWireError::ApiError(e.into()))?;
    let mut record_batch_stream = flight_client
        .query_stream(sql.into(), None)
        .await
        .map_err(|e| PgWireError::ApiError(e.into()))?;
    // we fetch the first record batch to make sure the schema is accessible in the stream
    let mut opt_record_batch = record_batch_stream.next().await;
    let arrow_schema = record_batch_stream
        .schema()
        .ok_or_else(|| PgWireError::ApiError("no schema in record batch stream".into()))?;
    let schema = Arc::new(
        arrow_schema_to_pg_fields(arrow_schema, &Format::UnifiedText)
            .map_err(|e| PgWireError::ApiError(e.into()))?,
    );
    let schema_copy = schema.clone();
    let pg_row_stream = Box::pin(try_stream!({
        while let Some(record_batch_res) = opt_record_batch {
            for row in encode_recordbatch(
                schema.clone(),
                record_batch_res.map_err(|e| PgWireError::ApiError(e.into()))?,
            ) {
                yield row?;
            }
            opt_record_batch = record_batch_stream.next().await;
        }
    }));
    Ok(Response::Query(QueryResponse::new(
        schema_copy,
        pg_row_stream,
    )))
}

#[async_trait]
impl SimpleQueryHandler for SimpleQueryH {
    async fn do_query<'a, C>(&self, _client: &mut C, sql: &str) -> PgWireResult<Vec<Response<'a>>>
    where
        C: ClientInfo + ClientPortalStore + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        Ok(vec![execute_query(&self.state, sql).await?])
    }
}
