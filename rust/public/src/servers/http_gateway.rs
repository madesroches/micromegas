use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    http::{Response, StatusCode},
    response::IntoResponse,
    routing::post,
};
use datafusion::arrow::{
    array::RecordBatch,
    json::{Writer, writer::JsonArray},
};
use http::{HeaderMap, Uri, header::AUTHORIZATION};
use micromegas_tracing::info;
use serde::Deserialize;
use thiserror::Error;
use tonic::transport::{Channel, ClientTlsConfig};

use crate::client::flightsql_client::Client;

#[derive(Error, Debug)]
pub enum GatewayError {
    #[error("Internal server error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response<Body> {
        let (status, message) = match &self {
            GatewayError::Internal(err) => {
                let msg = format!("{err:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
        };
        (status, message).into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    sql: String,
}

pub async fn handle_query(
    headers: HeaderMap,
    Json(request): Json<QueryRequest>,
) -> Result<String, GatewayError> {
    info!("request={request:?}");
    let flight_url = std::env::var("MICROMEGAS_FLIGHTSQL_URL")
        .with_context(|| "error reading MICROMEGAS_FLIGHTSQL_URL environment variable")?
        .parse::<Uri>()
        .with_context(|| "parsing flightsql url")?;
    let tls_config = ClientTlsConfig::new().with_native_roots();
    let channel = Channel::builder(flight_url)
        .tls_config(tls_config)
        .with_context(|| "tls_config")?
        .connect()
        .await
        .with_context(|| "connecting grpc channel")?;
    let mut client = Client::new(channel);
    if let Some(auth_header) = headers.get(AUTHORIZATION) {
        client.inner_mut().set_header(
            AUTHORIZATION.as_str(),
            auth_header
                .to_str()
                .with_context(|| "converting auth header to a string")?,
        );
    }
    let batches = client.query(request.sql, None).await?;
    if batches.is_empty() {
        return Ok("[]".to_string());
    }

    let mut buffer = Vec::new();
    let mut json_writer = Writer::<_, JsonArray>::new(&mut buffer);
    let batch_refs: Vec<&RecordBatch> = batches.iter().collect();
    json_writer
        .write_batches(&batch_refs)
        .with_context(|| "json_writer.write_batches")?;
    json_writer.finish().unwrap();
    Ok(String::from_utf8(buffer).with_context(|| "converting json buffer to utf8")?)
}

pub fn register_routes(router: Router) -> Router {
    router.route("/gateway/query", post(handle_query))
}
