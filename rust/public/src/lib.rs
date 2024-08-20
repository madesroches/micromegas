//! Micromegas is a unified and scalable observability stack.
//! It can help you collect and query logs, metrics and traces.
//!
//! # Very high level architecture
//!
//!
//! ```text
//! ┌─────────────────┐       
//! │ rust application│──────▶
//! └─────────────────┘       ┌────────────┐         ┌─────────────┐         ┌─────────────┐
//!                           │ ingestion  │────────▶│  analytics  │────────▶│ python API  │
//! ┌─────────────────┐       └────────────┘         └─────────────┘         └─────────────┘
//! │ unreal engine   │──────▶
//! └─────────────────┘      
//!
//! ```
//! # Rust Instrumentation
//! For rust applications, use micromegas::tracing for minimal overhead. Interoperability with tokio tracing is also enabled by default.
//!
//! # Unreal instrumentation
//! `MicromegasTracing` should be added to Unreal's Core module and `MicromegasTelemetrySink` can be added to a game or to a high level plugin. See <https://github.com/madesroches/micromegas/tree/main/unreal> for implementation.
//! 
//! # Telemetry ingestion server
//! <https://github.com/madesroches/micromegas/blob/main/rust/telemetry-ingestion-srv/src/main.rs>
//! 
//! # Analytics server
//! <https://github.com/madesroches/micromegas/blob/main/rust/analytics-srv/src/main.rs>
//! 
//! # Lakehouse daemon
//! <https://github.com/madesroches/micromegas/blob/main/rust/telemetry-admin-cli/src/telemetry_admin.rs> (with `crond` argument)
//! 
//! # Python API
//! <https://pypi.org/project/micromegas/>
//! 
//! 

pub use chrono;
pub use datafusion;
pub use object_store;
pub use sqlx;
pub use uuid;

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

pub mod analytics {
    pub use micromegas_analytics::*;
}

pub mod axum_utils;
pub mod servers;
