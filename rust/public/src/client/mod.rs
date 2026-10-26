/// FlightSQL client
pub mod flightsql_client;

/// FlightSQLClientFactory allows the creation of authenticated clients
pub mod flightsql_client_factory;

#[cfg(feature = "server")]
/// Fetch cpu traces and transform them into perfetto format
pub mod perfetto_trace_client;
#[cfg(feature = "server")]
pub use perfetto_trace_client::SpanTypes;

#[cfg(feature = "server")]
/// Process query builder for finding processes with various filters
pub mod query_processes;
