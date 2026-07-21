pub mod axum_utils;

/// connection info utilities for Tonic services
pub mod connect_info_layer;

/// http utilities
pub mod http_utils;

/// scheduled task for daemon
pub mod cron_task;

/// routes for ingestion server based on axum
pub mod ingestion;

/// OTLP/HTTP routes (logs, metrics, traces) for the ingestion server
pub mod otlp;

/// generic header-described webhook ingestion route
pub mod webhook;

/// Kinesis Data Firehose HTTP Endpoint Delivery route for OTLP metrics (CloudWatch Metric
/// Streams)
pub mod firehose;

/// shared auth/response/request-id plumbing for every Firehose-backed ingestion route
pub mod firehose_common;

/// Kinesis Data Firehose HTTP Endpoint Delivery route for CloudWatch Logs subscription
/// filters
pub mod firehose_cloudwatch_logs;

/// shared body-limit / decompression layers for ingestion routers (OTLP, webhook)
pub(crate) mod ingestion_limits;

/// implementation of maintenance daemon keeping the lakehouse updated
pub mod maintenance;

/// periodic self-observability collector for the metadata Postgres's pg_stat_* views
pub mod pg_stats;

/// minimal FlightSQL protocol implementation
pub mod flight_sql_service_impl;

/// structured per-query audit record emitted by the FlightSQL service
pub mod query_audit;

/// FlightSQL server builder
pub mod flight_sql_server;

/// web server for perfetto traces
pub mod perfetto;

/// metadata about this implementation of FlightSQL
pub mod sqlinfo;

/// named keys for authentication
pub mod key_ring;

/// log uris of http requests
pub mod log_uri_service;

/// http server that redirects queries to the analytics server translating the response into json
pub mod http_gateway;

/// authentication for the gRPC stack
pub mod tonic_auth_interceptor;

/// health check service for gRPC servers
pub mod grpc_health_service;

/// SIGTERM-driven graceful shutdown primitives shared by all services
pub mod shutdown;

/// shared readiness probe logic (DB + blob, 1 s success cache)
pub mod readiness;
