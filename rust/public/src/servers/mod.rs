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
