//! Micromegas is a unified and scalable observability stack.
//! It can help you collect and query logs, metrics and traces.
//!
//! # Very high level architecture
//!
//!
//! ```text
//! ┌─────────────────┐       
//! │ rust application│──────▶
//! └─────────────────┘       ┌─────────┐     ┌───────┐     ┌─────────┐     ┌──────────┐
//!                           │ingestion│────▶│pg & S3│◀────│analytics│◀────│python API│
//! ┌─────────────────┐       └─────────┘     └───────┘     └─────────┘     └──────────┘
//! │ unreal engine   │──────▶
//! └─────────────────┘      
//!
//! ```
//! ## Rust Instrumentation
//! For rust applications, use micromegas::tracing for minimal overhead. Interoperability with tokio tracing's logs is also enabled by default.
//!
//! ## Unreal instrumentation
//! `MicromegasTracing` should be added to Unreal's Core module and `MicromegasTelemetrySink` can be added to a game or to a high level plugin. See <https://github.com/madesroches/micromegas/tree/main/unreal> for implementation. It has been tested in editor, client and server builds on multiple platforms.
//! 
//! ## Telemetry ingestion server
//! <https://github.com/madesroches/micromegas/blob/main/rust/telemetry-ingestion-srv/src/main.rs>
//! 
//! ## Analytics server
//! <https://github.com/madesroches/micromegas/blob/main/rust/analytics-srv/src/main.rs>
//! 
//! ## Lakehouse daemon
//! <https://github.com/madesroches/micromegas/blob/main/rust/telemetry-admin-cli/src/telemetry_admin.rs> (with `crond` argument)
//! 
//! ## Python API
//! <https://pypi.org/project/micromegas/>
//! 
//! 
//! # Local developer configuration
//! 
//! For testing purposes, you can run the entire stack on your local workstation.
//! 
//! ## Environment variables
//! 
//!  - `MICROMEGAS_DB_USERNAME` and `MICROMEGAS_DB_PASSWD`: used by the database configuration script
//!  - `export MICROMEGAS_TELEMETRY_URL=http://localhost:9000`
//!  - `export MICROMEGAS_SQL_CONNECTION_STRING=postgres://{uname}:{passwd}@localhost:5432`
//!  - `export MICROMEGAS_OBJECT_STORE_URI=file:///some/local/path`
//! 
//! 1. Clone the github repository
//! `git clone https://github.com/madesroches/micromegas.git`
//! 
//! 2. Start a local instance of postgresql (requires docker and python)
//! 
//! Initialize the  environment variables to your liking
//! ```text
//! > cd micromegas/local_test_env/db
//! > ./run.py
//! ```
//! 
//! 3. In a new shell, start the ingestion server
//! ```text
//! > cd micromegas/rust
//! > cargo run -p telemetry-ingestion-srv -- --listen-endpoint-http 127.0.0.1:9000
//! ```
//! 
//! 
//! 4. In a new shell, start the analytics server
//! ```text
//! > cd micromegas/rust
//! > export MICROMEGAS_TELEMETRY_URL=http://localhost:9000
//! > cargo run -p analytics-srv
//! ```
//! 
//! 5. In a new shell, start the daemon
//! ```text
//! > cd micromegas/rust
//! > export MICROMEGAS_TELEMETRY_URL=http://localhost:9000
//! > cargo run -p telemetry-admin -- crond
//! ```
//! 
//! 6. In a python interpreter, query the analytics service
//! ```python
//! # local connection test
//! import datetime
//! import micromegas
//! BASE_URL = "http://localhost:8082/"
//! client = micromegas.client.Client(BASE_URL)
//! now = datetime.datetime.now(datetime.timezone.utc)
//! begin = now - datetime.timedelta(days=1)
//! end = now
//! sql = """
//! SELECT *
//! FROM log_entries
//! ORDER BY time DESC
//! LIMIT 10
//! ;"""
//! client.query_view("log_entries", "global", begin, end, sql)
//! ```
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
