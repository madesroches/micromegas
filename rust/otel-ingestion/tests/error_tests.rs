//! Tests for `OtelError`'s mapping from `IngestionServiceError` (AbAC Stage 5, #1373).

use micromegas_ingestion::web_ingestion_service::IngestionServiceError;
use micromegas_otel_ingestion::{OtelError, Signal};
use uuid::Uuid;

/// `IngestionServiceError::AudienceConflict` -- raised by `register_otel_process`'s conflict
/// guard (§6) -- must map to `OtelError::Denied`, so an OTLP client actually sees a 403/
/// PERMISSION_DENIED rather than a generic failure.
#[test]
fn audience_conflict_maps_to_denied_403() {
    let err = IngestionServiceError::AudienceConflict {
        process_id: Uuid::new_v4(),
        existing: "team-a".to_string(),
        incoming: "team-b".to_string(),
    };

    let otel_err = OtelError::from_ingestion(err, Signal::Logs);

    assert!(matches!(otel_err, OtelError::Denied { .. }));
    assert_eq!(otel_err.http_status(), 403);
    assert_eq!(otel_err.grpc_code(), 7);
}
