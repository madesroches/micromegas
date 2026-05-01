//! Re-exports of the OTLP proto message types we use.
//!
//! Pulled from `opentelemetry-proto` without the `gen-tonic` feature — we want
//! the prost-generated `*_data` modules but not the gRPC service stubs.
//!
//! Also defines the small `google.rpc.Status` proto we use as the body of 4xx/5xx
//! OTLP/HTTP responses (per spec). Hand-rolled rather than pulling in `tonic-types`
//! to keep tonic out of the dependency graph.

pub use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsPartialSuccess, ExportLogsServiceRequest, ExportLogsServiceResponse,
};
pub use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsPartialSuccess, ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
pub use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse,
};
pub use opentelemetry_proto::tonic::common::v1::any_value;
pub use opentelemetry_proto::tonic::common::v1::{
    AnyValue, ArrayValue, InstrumentationScope, KeyValue, KeyValueList,
};
pub use opentelemetry_proto::tonic::logs::v1::{
    LogRecord, ResourceLogs, ScopeLogs, SeverityNumber,
};
pub use opentelemetry_proto::tonic::metrics::v1::{Metric, ResourceMetrics, ScopeMetrics, metric};
pub use opentelemetry_proto::tonic::resource::v1::Resource;
pub use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, span, status};

/// Hand-rolled `google.rpc.Status` proto. The OTLP/HTTP spec mandates this on 4xx/5xx
/// responses ("The response body for all HTTP 4xx and HTTP 5xx responses MUST be a
/// Protobuf-encoded Status message that describes the problem.").
///
/// `details` is `repeated google.protobuf.Any` per the upstream proto; we omit it in v1.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Status {
    #[prost(int32, tag = "1")]
    pub code: i32,
    #[prost(string, tag = "2")]
    pub message: String,
}
