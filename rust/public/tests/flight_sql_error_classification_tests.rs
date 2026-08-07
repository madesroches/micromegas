// Unit tests for `micromegas::servers::flight_sql_service_impl`'s error-classification
// helpers (issue #1435):
// - `classify_datafusion_error`: gRPC `Code` for every `DataFusionError` variant the mapping
//   table cares about, including wrapped in `Context`/`Diagnostic`/`Collection`.
// - `client_error`: the returned `Status`'s code/message -- no `.rs:` file:line pattern, a
//   diagnostic span rendered in the message when present, and the physical plan text never
//   appearing in the client-facing message under any circumstance.
// - `build_log_line`: the pure helper behind the server-side log line -- the physical plan
//   section's presence/absence and its truncation boundary.
// - `classify_flight_error`: recovering a `DataFusionError` from the per-batch
//   `FlightError::ExternalError` rewrap.
// - `error_class`: the `Code` -> `"user"/"resource"/"internal"` mapping.

use micromegas::arrow_flight::error::FlightError;
use micromegas::datafusion::arrow::datatypes::Schema;
use micromegas::datafusion::common::{Diagnostic, Location, SchemaError, Span};
use micromegas::datafusion::error::{DataFusionError, Result as DataFusionResult};
use micromegas::datafusion::execution::TaskContext;
use micromegas::datafusion::physical_expr::EquivalenceProperties;
use micromegas::datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use micromegas::datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
    SendableRecordBatchStream,
};
use micromegas::datafusion::sql::sqlparser::parser::ParserError;
use micromegas::servers::flight_sql_service_impl::{
    MAX_PLAN_CHARS, build_log_line, classify_datafusion_error, classify_flight_error, client_error,
    error_class,
};
use micromegas::tonic::Code;
use std::sync::Arc;

/// Minimal `ExecutionPlan` with a real `DisplayAs` impl (unlike
/// `query_audit_tests.rs`'s `FakeExec`, whose `fmt_as` is `unimplemented!()` --
/// it was never written to be displayed), so `displayable(plan).indent(true)`
/// renders predictable, known text: exactly `"{display_text}\n"` for a
/// childless plan, since `IndentVisitor::pre_visit` writes the node's own
/// `fmt_as` output followed by a single newline, with no leading indent at
/// depth 0 and (for `displayable()`, as opposed to `with_metrics()`) no
/// metrics/statistics/schema suffix.
#[derive(Debug)]
struct FakePlan {
    display_text: String,
    properties: Arc<PlanProperties>,
}

impl FakePlan {
    fn new_arc(display_text: impl Into<String>) -> Arc<dyn ExecutionPlan> {
        let schema = Arc::new(Schema::empty());
        let properties = Arc::new(PlanProperties::new(
            EquivalenceProperties::new(schema),
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Bounded,
        ));
        Arc::new(Self {
            display_text: display_text.into(),
            properties,
        })
    }
}

impl DisplayAs for FakePlan {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.display_text)
    }
}

impl ExecutionPlan for FakePlan {
    fn name(&self) -> &str {
        "FakePlan"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    fn with_new_children(
        self: Arc<Self>,
        _children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> DataFusionResult<Arc<dyn ExecutionPlan>> {
        unimplemented!("not exercised by these tests")
    }

    fn execute(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> DataFusionResult<SendableRecordBatchStream> {
        unimplemented!("not exercised by these tests")
    }
}

fn sql_error() -> DataFusionError {
    DataFusionError::SQL(
        Box::new(ParserError::ParserError("bad token".to_string())),
        None,
    )
}

fn schema_error() -> DataFusionError {
    DataFusionError::SchemaError(
        Box::new(SchemaError::DuplicateUnqualifiedField {
            name: "col".to_string(),
        }),
        Box::new(None),
    )
}

// --- classify_datafusion_error --------------------------------------------

#[test]
fn classify_datafusion_error_maps_caller_mistake_variants_to_invalid_argument() {
    assert_eq!(
        classify_datafusion_error(&sql_error()),
        Code::InvalidArgument
    );
    assert_eq!(
        classify_datafusion_error(&DataFusionError::Plan("bad plan".to_string())),
        Code::InvalidArgument
    );
    assert_eq!(
        classify_datafusion_error(&schema_error()),
        Code::InvalidArgument
    );
    assert_eq!(
        classify_datafusion_error(&DataFusionError::Execution("bad execution".to_string())),
        Code::InvalidArgument
    );
    assert_eq!(
        classify_datafusion_error(&DataFusionError::Configuration("bad config".to_string())),
        Code::InvalidArgument
    );
}

#[test]
fn classify_datafusion_error_maps_resources_exhausted_to_resource_exhausted() {
    assert_eq!(
        classify_datafusion_error(&DataFusionError::ResourcesExhausted(
            "memory budget exceeded".to_string()
        )),
        Code::ResourceExhausted
    );
}

#[test]
fn classify_datafusion_error_maps_not_implemented_to_unimplemented() {
    assert_eq!(
        classify_datafusion_error(&DataFusionError::NotImplemented("not yet".to_string())),
        Code::Unimplemented
    );
}

#[test]
fn classify_datafusion_error_maps_internal_to_internal() {
    assert_eq!(
        classify_datafusion_error(&DataFusionError::Internal("this is a bug".to_string())),
        Code::Internal
    );
}

#[test]
fn classify_datafusion_error_unwraps_context() {
    let wrapped = DataFusionError::Context(
        "while planning".to_string(),
        Box::new(DataFusionError::Plan("bad plan".to_string())),
    );
    assert_eq!(classify_datafusion_error(&wrapped), Code::InvalidArgument);
}

#[test]
fn classify_datafusion_error_unwraps_diagnostic() {
    let diag = Diagnostic::new_error("bad column", None);
    let wrapped = DataFusionError::Diagnostic(
        Box::new(diag),
        Box::new(DataFusionError::ResourcesExhausted(
            "too much memory".to_string(),
        )),
    );
    assert_eq!(classify_datafusion_error(&wrapped), Code::ResourceExhausted);
}

#[test]
fn classify_datafusion_error_unwraps_collection_via_first_element() {
    let wrapped = DataFusionError::Collection(vec![
        DataFusionError::NotImplemented("feature X".to_string()),
        DataFusionError::Internal("unrelated".to_string()),
    ]);
    assert_eq!(classify_datafusion_error(&wrapped), Code::Unimplemented);
}

// --- error_class -------------------------------------------------------

#[test]
fn error_class_maps_every_code_to_its_bucket() {
    assert_eq!(error_class(Code::InvalidArgument), "user");
    assert_eq!(error_class(Code::Unimplemented), "user");
    assert_eq!(error_class(Code::ResourceExhausted), "resource");
    assert_eq!(error_class(Code::Internal), "internal");
    // Everything else not explicitly bucketed also falls to "internal".
    assert_eq!(error_class(Code::Unknown), "internal");
    assert_eq!(error_class(Code::Unauthenticated), "internal");
}

// --- client_error --------------------------------------------------------

#[test]
fn client_error_message_never_contains_a_file_line_pattern() {
    for err in [
        sql_error(),
        DataFusionError::Plan("bad plan".to_string()),
        schema_error(),
        DataFusionError::Execution("bad execution".to_string()),
        DataFusionError::ResourcesExhausted("too much memory".to_string()),
        DataFusionError::NotImplemented("not yet".to_string()),
        DataFusionError::Internal("this is a bug".to_string()),
        DataFusionError::Context(
            "while planning".to_string(),
            Box::new(DataFusionError::Plan("bad plan".to_string())),
        ),
        DataFusionError::Collection(vec![DataFusionError::Plan("bad plan".to_string())]),
    ] {
        let status = client_error("error building dataframe", err, "query-123", None);
        assert!(
            !status.message().contains(".rs:"),
            "message should not contain a file:line suffix: {}",
            status.message()
        );
        assert!(status.message().contains("query-123"));
    }
}

#[test]
fn client_error_classifies_and_includes_query_id() {
    let status = client_error(
        "error building dataframe",
        DataFusionError::Plan("no such function".to_string()),
        "abc-123",
        None,
    );
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(status.message().contains("no such function"));
    assert!(status.message().contains("(query_id=abc-123)"));
}

#[test]
fn client_error_includes_diagnostic_span_and_never_the_physical_plan() {
    let span = Some(Span {
        start: Location { line: 2, column: 5 },
        end: Location {
            line: 2,
            column: 10,
        },
    });
    let diag = Diagnostic::new_error("unknown column 'foo'", span)
        .with_note("did you mean 'bar'?", None)
        .with_help("check the column name", None);
    let err = DataFusionError::Diagnostic(
        Box::new(diag),
        Box::new(DataFusionError::Plan("unknown column 'foo'".to_string())),
    );
    let plan = FakePlan::new_arc("ProjectionExec: expr=[jsonb_format_json(x)]");
    let status = client_error(
        "error building dataframe",
        err,
        "query-with-span",
        Some(&plan),
    );

    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(status.message().contains("at line 2, column 5"));
    assert!(status.message().contains("note: did you mean 'bar'?"));
    assert!(status.message().contains("help: check the column name"));
    assert!(!status.message().contains("physical plan:"));
}

#[test]
fn client_error_with_plan_and_no_diagnostic_never_puts_the_plan_in_the_message() {
    let plan = FakePlan::new_arc("ProjectionExec: expr=[jsonb_format_json(x)]");
    let status = client_error(
        "error building dataframe",
        DataFusionError::Execution("division by zero".to_string()),
        "query-exec",
        Some(&plan),
    );

    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(status.message().contains("division by zero"));
    assert!(!status.message().contains("physical plan:"));
    assert!(!status.message().contains("ProjectionExec"));
}

// --- build_log_line --------------------------------------------------------

#[test]
fn build_log_line_includes_physical_plan_section_when_plan_is_some() {
    let plan = FakePlan::new_arc("ProjectionExec: expr=[jsonb_format_json(x)] -- marker-42");
    let line = build_log_line(
        "error building dataframe",
        &DataFusionError::Execution("boom".to_string()),
        "query-1",
        Some(&plan),
    );

    assert!(line.contains("physical plan:"));
    assert!(line.contains("marker-42"));
    assert!(line.contains("(query_id=query-1)"));
}

#[test]
fn build_log_line_omits_physical_plan_section_when_plan_is_none() {
    let line = build_log_line(
        "error building dataframe",
        &DataFusionError::Execution("boom".to_string()),
        "query-1",
        None,
    );

    assert!(!line.contains("physical plan:"));
}

#[test]
fn build_log_line_truncates_oversized_plan_text_at_max_plan_chars() {
    let oversized = "X".repeat(MAX_PLAN_CHARS + 500);
    let plan = FakePlan::new_arc(oversized);
    let line = build_log_line(
        "error building dataframe",
        &DataFusionError::Execution("boom".to_string()),
        "query-1",
        Some(&plan),
    );

    let expected_truncated = format!("{}... (truncated)", "X".repeat(MAX_PLAN_CHARS));
    assert!(
        line.contains(&expected_truncated),
        "expected the plan text truncated at exactly MAX_PLAN_CHARS chars"
    );
    // The untruncated, over-limit run of X's should not appear anywhere.
    assert!(!line.contains(&"X".repeat(MAX_PLAN_CHARS + 1)));
}

#[test]
fn build_log_line_truncates_multibyte_plan_text_without_panicking() {
    // `truncate_plan_text` uses `char_indices()`, not a byte-offset slice, so
    // this must not panic even though "é" is 2 bytes wide -- a byte-offset cut
    // at `MAX_PLAN_CHARS` bytes would land mid-character here.
    let oversized = "é".repeat(MAX_PLAN_CHARS + 100);
    let plan = FakePlan::new_arc(oversized);
    let line = build_log_line(
        "error building dataframe",
        &DataFusionError::Execution("boom".to_string()),
        "query-1",
        Some(&plan),
    );

    let expected_truncated = format!("{}... (truncated)", "é".repeat(MAX_PLAN_CHARS));
    assert!(
        line.contains(&expected_truncated),
        "expected the plan text truncated at exactly MAX_PLAN_CHARS chars"
    );
}

// --- classify_flight_error --------------------------------------------------------

#[test]
fn classify_flight_error_recovers_data_fusion_error_from_external_error() {
    let inner = DataFusionError::Plan("no such function".to_string());
    let flight_err = FlightError::ExternalError(Box::new(inner));
    let status = classify_flight_error(flight_err, "query-batch", None);
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(status.message().contains("no such function"));
}

#[test]
fn classify_flight_error_maps_resources_exhausted_the_same_as_classify_datafusion_error() {
    let inner = DataFusionError::ResourcesExhausted("too much memory".to_string());
    let expected_code = classify_datafusion_error(&inner);
    let flight_err = FlightError::ExternalError(Box::new(inner));
    let status = classify_flight_error(flight_err, "query-batch", None);
    assert_eq!(status.code(), expected_code);
    assert_eq!(status.code(), Code::ResourceExhausted);
}

#[test]
fn classify_flight_error_external_error_with_non_data_fusion_payload_is_internal() {
    #[derive(Debug)]
    struct OtherError;
    impl std::fmt::Display for OtherError {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "some other error")
        }
    }
    impl std::error::Error for OtherError {}

    let flight_err = FlightError::ExternalError(Box::new(OtherError));
    let status = classify_flight_error(flight_err, "query-batch", None);
    assert_eq!(status.code(), Code::Internal);
}

#[test]
fn classify_flight_error_non_external_variant_is_internal() {
    let flight_err = FlightError::ProtocolError("unexpected message".to_string());
    let status = classify_flight_error(flight_err, "query-batch", None);
    assert_eq!(status.code(), Code::Internal);
}
