pushd ..\rust
cargo release --exclude analytics-srv --exclude telemetry-ingestion-srv --exclude telemetry-admin
popd
