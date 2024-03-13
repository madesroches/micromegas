//! ingestion : provides write access to the telemetry data lake


// crate-specific lint exceptions:
#![allow(clippy::missing_errors_doc)]


pub mod data_lake_connection;
pub mod remote_data_lake;
pub mod sql_migration;
pub mod sql_telemetry_db;
pub mod web_ingestion_service;

