pushd ..\rust
set PUBLISH_GRACE_SLEEP=2
cargo release --exclude analytics-srv --exclude telemetry-ingestion-srv --exclude telemetry-admin
popd
