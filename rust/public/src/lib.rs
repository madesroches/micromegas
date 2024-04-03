//! micromegas : scalable telemetry

pub use object_store;

pub mod telemetry {
    pub use micromegas_telemetry::*;
}

pub mod telemetry_sink {
    pub use micromegas_telemetry_sink::*;
}

pub mod tracing {
    pub use micromegas_tracing::*;
}

pub mod ingestion {
    pub use micromegas_ingestion::*;
}
