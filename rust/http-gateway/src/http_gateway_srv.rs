mod error;

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::{Json, Router, routing::post};
use clap::Parser;
use error::GatewayError;
use micromegas::axum::http::HeaderMap;
use micromegas::axum::http::header::AUTHORIZATION;
use micromegas::datafusion::arrow::array::RecordBatch;
use micromegas::datafusion::arrow::json::writer::{JsonArray, Writer};
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tracing::levels::LevelFilter;
use micromegas::{
    axum::{self, http::Uri},
    client::flightsql_client::Client,
    tonic::transport::{Channel, ClientTlsConfig},
    tracing::info,
};
use serde::Deserialize;

#[derive(Parser, Debug)]
#[clap(name = "Micromegas http gateway server")]
#[clap(about = "Micromegas http gateway server", version, author)]
struct Cli {
    #[clap(long, default_value = "0.0.0.0:3000")]
    listen_endpoint_http: SocketAddr,
}

#[derive(Debug, Deserialize)]
struct QueryRequest {
    sql: String,
}

async fn handle_query(
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

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .with_local_sink_max_level(LevelFilter::Debug)
        .with_interop_max_level_override(LevelFilter::Info)
        .build();
    let args = Cli::parse();
    let app = Router::new().route("/query", post(handle_query));
    let listener = tokio::net::TcpListener::bind(args.listen_endpoint_http).await?;
    info!("Server running on {}", args.listen_endpoint_http);
    axum::serve(listener, app).await?;
    Ok(())
}
