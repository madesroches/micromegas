pub mod axum_utils;

/// scheduled task for daemon
pub mod cron_task;

/// routes for ingestion server based on axum
pub mod ingestion;

/// implementation of maintenance daemon keeping the lakehouse updated
pub mod maintenance;

/// minimal FlightSQL protocol implementation
pub mod flight_sql_service_impl;

/// web server for perfetto traces
pub mod perfetto;

/// metadata about this implementation of FlightSQL
pub mod sqlinfo;

/// named keys for authentication
pub mod key_ring;

/// log uris of http requests
pub mod log_uri_service;

/// pg_ tables expected when connecting using pg-gateway-srv
#[cfg(feature = "pg_catalog")]
pub mod pg_catalog;

/// authentication for the gRPC stack
pub mod tonic_auth_interceptor;
