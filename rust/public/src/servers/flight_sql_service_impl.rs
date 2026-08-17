use super::http_utils::get_client_ip;
use super::query_audit::{QueryAuditRecord, ScanMetrics, aggregate_scan_metrics};
use super::sqlinfo::{
    SQL_INFO_DATE_TIME_FUNCTIONS, SQL_INFO_NUMERIC_FUNCTIONS, SQL_INFO_SQL_KEYWORDS,
    SQL_INFO_STRING_FUNCTIONS, SQL_INFO_SYSTEM_FUNCTIONS,
};
use anyhow::Result;
use arrow_flight::decode::FlightRecordBatchStream;
use arrow_flight::encode::{DictionaryHandling, FlightDataEncoderBuilder};
use arrow_flight::error::FlightError;
use arrow_flight::sql::DoPutPreparedStatementResult;
use arrow_flight::sql::metadata::{SqlInfoData, SqlInfoDataBuilder};
use arrow_flight::sql::server::PeekableFlightDataStream;
use arrow_flight::sql::{
    ActionBeginSavepointRequest, ActionBeginSavepointResult, ActionBeginTransactionRequest,
    ActionBeginTransactionResult, ActionCancelQueryRequest, ActionCancelQueryResult,
    ActionClosePreparedStatementRequest, ActionCreatePreparedStatementRequest,
    ActionCreatePreparedStatementResult, ActionCreatePreparedSubstraitPlanRequest,
    ActionEndSavepointRequest, ActionEndTransactionRequest, Any, CommandGetCatalogs,
    CommandGetCrossReference, CommandGetDbSchemas, CommandGetExportedKeys, CommandGetImportedKeys,
    CommandGetPrimaryKeys, CommandGetSqlInfo, CommandGetTableTypes, CommandGetTables,
    CommandGetXdbcTypeInfo, CommandPreparedStatementQuery, CommandPreparedStatementUpdate,
    CommandStatementIngest, CommandStatementQuery, CommandStatementSubstraitPlan,
    CommandStatementUpdate, ProstMessageExt, SqlInfo, TicketStatementQuery,
    server::FlightSqlService,
};
use arrow_flight::{
    Action, FlightDescriptor, FlightEndpoint, FlightInfo, HandshakeRequest, HandshakeResponse,
    Ticket, flight_service_server::FlightService,
};
use core::str;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::ipc::writer::StreamWriter;
use datafusion::error::DataFusionError;
use datafusion::physical_plan::{ExecutionPlan, execute_stream};
use futures::StreamExt;
use futures::{Stream, TryStreamExt};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::QueryPartitionProvider;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::read_scope::{CallerContext, IsolationConfig, ReadScope};
use micromegas_analytics::lakehouse::runtime::scoped_runtime;
use micromegas_analytics::lakehouse::scoped_memory_pool::ScopedMemoryPool;
use micromegas_analytics::lakehouse::session_configurator::SessionConfigurator;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::replication::bulk_ingest;
use micromegas_analytics::time::TimeRange;
use micromegas_auth::policy::ReadPolicy;
use micromegas_auth::types::{AuthContext, ProviderUnavailable};
use micromegas_auth::user_attribution::{is_admin, validate_and_resolve_user_attribution_grpc};
use micromegas_tracing::prelude::*;
use once_cell::sync::Lazy;
use prost::Message;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

type FlightDataStream =
    Pin<Box<dyn Stream<Item = Result<arrow_flight::FlightData, Status>> + Send>>;

macro_rules! status {
    ($desc:expr, $err:expr) => {
        Status::internal(format!("{}: {} at {}:{}", $desc, $err, file!(), line!()))
    };
}

macro_rules! api_entry_not_implemented {
    () => {{
        let function_name = micromegas_tracing::__function_name!();
        error!("not implemented: {function_name}");
        Err(Status::unimplemented(format!(
            "{}:{} not implemented: {function_name}",
            file!(),
            line!()
        )))
    }};
}

/// Design §6: for gRPC-metadata parses that are unambiguously caller-supplied
/// input (range-header/limit-header values the caller set directly) with no
/// `DataFusionError` involved -- `Status::invalid_argument` directly, no
/// file:line/path suffix (nothing here ever went through `status!`'s own
/// `file!()/line!()`).
macro_rules! client_input_error {
    ($desc:expr, $err:expr) => {
        Status::invalid_argument(format!("{}: {}", $desc, $err))
    };
}

/// Classifies the root cause of a `DataFusionError` into the gRPC status code
/// that best distinguishes "the caller's SQL/input was bad" from "the server
/// broke" -- see the FlightSQL error-classification plan (issue #1435) for the
/// full rationale, including why `Execution`/`Configuration` land under
/// `InvalidArgument` and why `ArrowError` stays `Internal`.
pub fn classify_datafusion_error(err: &DataFusionError) -> tonic::Code {
    use datafusion::error::DataFusionError as DFE;
    match err.find_root() {
        DFE::SQL(..)
        | DFE::Plan(_)
        | DFE::SchemaError(..)
        | DFE::Execution(_)
        | DFE::Configuration(_) => tonic::Code::InvalidArgument,
        DFE::ResourcesExhausted(_) => tonic::Code::ResourceExhausted,
        DFE::NotImplemented(_) => tonic::Code::Unimplemented,
        _ => tonic::Code::Internal,
    }
}

/// Maps a gRPC status code to the `QueryAuditRecord.error_class` bucket used
/// both by the audit log and to gate the `query_failed`/
/// `query_duration_with_error` metrics (see Design §4).
pub fn error_class(code: tonic::Code) -> &'static str {
    match code {
        tonic::Code::InvalidArgument | tonic::Code::Unimplemented => "user",
        tonic::Code::ResourceExhausted => "resource",
        _ => "internal",
    }
}

/// Cap on the physical-plan text appended to the server-side log line (Design
/// §2). The plan can render arbitrarily long `file_groups=` sections for wide
/// scans, and this text is server-log-only, never returned to the client.
pub const MAX_PLAN_CHARS: usize = 2000;

/// Truncate `text` at a char boundary once it exceeds `MAX_PLAN_CHARS`, tagging
/// the cut with a trailing marker. Driven by `char_indices()` (not byte
/// length) so a multibyte plan text (e.g. a non-ASCII string literal from a
/// query predicate, rendered via `ScalarValue`'s `Display` impl) can't panic
/// on a byte-offset slice landing mid-character.
fn truncate_plan_text(text: &str) -> String {
    match text.char_indices().nth(MAX_PLAN_CHARS) {
        None => text.to_string(),
        Some((i, _)) => format!("{}... (truncated)", &text[..i]),
    }
}

/// Builds the full server-side log line for a classified `DataFusionError`:
/// `desc`, the error's own `Display` (its outer `Context` chain intact, with
/// backtrace if enabled -- unlike `find_root()`, which is for classification
/// only), the `query_id`, and -- when there's no diagnostic span to show the
/// caller instead (Design §2) -- the truncated physical plan text. `pub` so
/// step 9's tests can assert its content, including the truncation marker,
/// without capturing log output.
pub fn build_log_line(
    desc: &str,
    err: &DataFusionError,
    query_id: &str,
    plan: Option<&Arc<dyn ExecutionPlan>>,
) -> String {
    let mut full = format!("{desc}: {err} (query_id={query_id})");
    if let Some(plan) = plan {
        let plan_text = format!(
            "{}",
            datafusion::physical_plan::displayable(plan.as_ref()).indent(true)
        );
        full.push_str(&format!(
            "\nphysical plan:\n{}",
            truncate_plan_text(&plan_text)
        ));
    }
    full
}

/// Logs `build_log_line`'s output at `error!` for `error_class == "internal"`,
/// `warn!` otherwise -- the single log point for every `DataFusionError`
/// reaching `client_error`/`classify_flight_error`.
fn error_or_warn_log(
    code: tonic::Code,
    desc: &str,
    err: &DataFusionError,
    query_id: &str,
    plan: Option<&Arc<dyn ExecutionPlan>>,
) {
    let full = build_log_line(desc, err, query_id, plan);
    match error_class(code) {
        "internal" => error!("{full}"),
        _ => warn!("{full}"),
    }
}

/// Classifies `err`, builds the client-facing `Status` -- `desc` + root-error
/// text (backtrace stripped) + an optional diagnostic span/notes/helps +
/// `query_id` -- and logs the full error server-side (Design §2). The physical
/// plan, when present and there's no diagnostic span to show instead, only
/// ever reaches the server log, never the returned `Status`: `displayable`
/// can render object-store partition paths from Micromegas view scans, which
/// would leak internal lakehouse details to every caller hitting an
/// execution-time error.
pub fn client_error(
    desc: &str,
    err: DataFusionError,
    query_id: &str,
    plan: Option<&Arc<dyn ExecutionPlan>>,
) -> Status {
    let code = classify_datafusion_error(&err);
    let mut msg = format!("{desc}: {}", err.find_root().strip_backtrace());
    let mut has_span = false;
    if let Some(diag) = err.diagnostic() {
        if let Some(span) = diag.span {
            has_span = true;
            msg.push_str(&format!(
                " (at line {}, column {})",
                span.start.line, span.start.column
            ));
        }
        for note in &diag.notes {
            msg.push_str(&format!("\nnote: {}", note.message));
        }
        for help in &diag.helps {
            msg.push_str(&format!("\nhelp: {}", help.message));
        }
    }
    msg.push_str(&format!(" (query_id={query_id})"));
    error_or_warn_log(
        code,
        desc,
        &err,
        query_id,
        if has_span { None } else { plan },
    );
    Status::new(code, msg)
}

/// Recovers the `DataFusionError` from the per-batch `FlightError` re-wrap
/// (`FlightError::ExternalError(Box::new(e))`, applied once the raw
/// `DataFusionError` stream is wrapped for Arrow Flight) and classifies it via
/// `client_error`. Every branch logs exactly once before returning, so this is
/// a single, unconditional log point for the per-batch stream-error site
/// regardless of which branch is taken. `pub` so step 9's external
/// integration tests can exercise this recovery path directly.
pub fn classify_flight_error(
    err: FlightError,
    query_id: &str,
    plan: Option<&Arc<dyn ExecutionPlan>>,
) -> Status {
    match err {
        FlightError::ExternalError(inner) => match inner.downcast::<DataFusionError>() {
            Ok(df_err) => client_error("error building data stream", *df_err, query_id, plan),
            Err(inner) => {
                error!("error building data stream: {inner} (query_id={query_id})");
                Status::internal(format!(
                    "error building data stream: {inner} (query_id={query_id})"
                ))
            }
        },
        other => {
            error!("error building data stream: {other} (query_id={query_id})");
            Status::internal(format!(
                "error building data stream: {other} (query_id={query_id})"
            ))
        }
    }
}

/// Attribution, per-stage timing, and (once created) the physical plan for
/// one query. Built as soon as attribution is resolved, updated as each
/// setup stage completes, and emitted exactly once as a [`QueryAuditRecord`]
/// under the `flightsql_query_audit` log target — either on an early setup
/// failure, at stream completion/error, or (if the stream is dropped mid-drain)
/// from `CompletionTrackedStream`'s `Drop` impl.
struct QueryAuditState {
    /// Minted as the very first statement of `execute_query`, before any
    /// fallible step -- included in every client-facing `Status` built by
    /// `client_error`/`classify_flight_error` and in this query's audit
    /// record, so a failure's server-log line and its audit record can be
    /// correlated by grepping this id.
    query_id: String,
    client_ip: String,
    client: String,
    agent: String,
    entrypoint: String,
    session: Option<String>,
    notebook: Option<String>,
    cell: Option<String>,
    user: String,
    email: String,
    name: Option<String>,
    service_account: bool,
    service_account_name: Option<String>,
    sql: String,
    range_begin: Option<String>,
    range_end: Option<String>,
    limit: Option<u64>,
    context_init_ms: f64,
    planning_ms: f64,
    execution_ms: f64,
    setup_ms: f64,
    request_start: Instant,
    /// `None` until the physical plan is created; set as soon as it is, and
    /// still `None` for a record emitted on a setup failure that happens
    /// before that point.
    plan: Option<Arc<dyn ExecutionPlan>>,
    /// This query's per-query memory-pool wrapper; owned from construction so
    /// `emit()` can read its peak on every terminal path, including setup
    /// failures.
    pool: Arc<ScopedMemoryPool>,
}

impl QueryAuditState {
    /// Aggregate the plan's DataFusion metrics (if a physical plan was
    /// created before the failure/completion), assemble the audit record,
    /// and emit it as a single JSON log line.
    ///
    /// Takes `&self` (rather than consuming) so it can be called from a
    /// setup-error `map_err` closure while leaving the state available for
    /// further updates/completion on the success path, and so `Drop` can
    /// call it on an abandoned/cancelled stream without needing to
    /// reconstruct anything.
    fn emit(&self, status: &'static str, error: Option<String>, error_class: Option<&'static str>) {
        let scan = match &self.plan {
            Some(plan) => aggregate_scan_metrics(plan.as_ref()),
            None => ScanMetrics {
                output_rows: None,
                bytes_scanned: 0,
                spilled_bytes: 0,
                spill_count: 0,
            },
        };
        let peak_memory_bytes = self.pool.peak() as u64;
        imetric!("query_peak_memory_bytes", "bytes", peak_memory_bytes);
        let total_ms = self.request_start.elapsed().as_secs_f64() * 1000.0;
        let record = QueryAuditRecord {
            query_id: self.query_id.clone(),
            client_ip: self.client_ip.clone(),
            client: self.client.clone(),
            agent: self.agent.clone(),
            entrypoint: self.entrypoint.clone(),
            session: self.session.clone(),
            notebook: self.notebook.clone(),
            cell: self.cell.clone(),
            user: self.user.clone(),
            email: self.email.clone(),
            name: self.name.clone(),
            service_account: self.service_account,
            service_account_name: self.service_account_name.clone(),
            sql: self.sql.clone(),
            range_begin: self.range_begin.clone(),
            range_end: self.range_end.clone(),
            limit: self.limit,
            context_init_ms: self.context_init_ms,
            planning_ms: self.planning_ms,
            execution_ms: self.execution_ms,
            setup_ms: self.setup_ms,
            total_ms,
            status,
            error,
            error_class,
            output_rows: scan.output_rows,
            bytes_scanned: scan.bytes_scanned,
            peak_memory_bytes,
            spilled_bytes: scan.spilled_bytes,
            spill_count: scan.spill_count,
        };
        match serde_json::to_string(&record) {
            Ok(json) => info!(target: "flightsql_query_audit", "{json}"),
            Err(e) => warn!("failed to serialize query audit record: {e}"),
        }
    }

    /// Emits an "error" audit record for an already-built client-facing
    /// `status` (message + `error_class` derived from its code) and returns it
    /// unchanged -- the shared tail of every setup-phase `map_err` in
    /// `execute_query`.
    fn fail(&self, status: Status) -> Status {
        self.emit(
            "error",
            Some(status.message().to_string()),
            Some(error_class(status.code())),
        );
        status
    }
}

/// Stream wrapper that tracks when the stream is fully consumed
struct CompletionTrackedStream<S> {
    inner: S,
    start_time: i64,
    completed: bool,
    audit: Option<QueryAuditState>,
}

impl<S> CompletionTrackedStream<S> {
    fn new(inner: S, start_time: i64, audit: QueryAuditState) -> Self {
        Self {
            inner,
            start_time,
            completed: false,
            audit: Some(audit),
        }
    }
}

impl<S> Drop for CompletionTrackedStream<S> {
    /// If the stream is dropped before yielding `None` or an `Err` (client
    /// disconnect/cancel mid-drain), `poll_next` never ran its completion
    /// arms, so `self.audit` is still `Some(...)`. Emit it here with a
    /// terminal "incomplete" status so cancelled/abandoned queries are still
    /// audited instead of silently vanishing. A no-op for streams that
    /// completed or errored normally, since those arms already `take()` the
    /// audit state.
    fn drop(&mut self) {
        if let Some(state) = self.audit.take() {
            state.emit("incomplete", None, None);
        }
    }
}

impl<S> Stream for CompletionTrackedStream<S>
where
    S: Stream<Item = Result<arrow_flight::FlightData, Status>> + Unpin + Send,
{
    type Item = Result<arrow_flight::FlightData, Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(result)) => {
                // Every `Status` reaching this stream was already logged once by
                // `classify_flight_error` (the sole `map_err` feeding it) when it
                // built that `Status` -- this arm only reads the code to derive
                // the audit/metric class, it never logs again (Design §4).
                if let Err(ref err) = result
                    && !self.completed
                {
                    let class = error_class(err.code());
                    let total_duration = now() - self.start_time;
                    match class {
                        "internal" => {
                            imetric!("query_duration_with_error", "ticks", total_duration as u64);
                            imetric!("query_failed", "count", 1);
                        }
                        "resource" => {
                            imetric!("query_failed_resource", "count", 1);
                        }
                        _ => {
                            imetric!("query_failed_user", "count", 1);
                        }
                    }
                    self.completed = true;
                    if let Some(state) = self.audit.take() {
                        state.emit("error", Some(err.message().to_string()), Some(class));
                    }
                }
                Poll::Ready(Some(result))
            }
            Poll::Ready(None) => {
                // Stream completed successfully
                if !self.completed {
                    let total_duration = now() - self.start_time;
                    imetric!("query_duration_total", "ticks", total_duration as u64);
                    imetric!("query_completed_successfully", "count", 1);
                    self.completed = true;
                    if let Some(state) = self.audit.take() {
                        state.emit("ok", None, None);
                    }
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

static INSTANCE_SQL_DATA: Lazy<SqlInfoData> = Lazy::new(|| {
    let mut builder = SqlInfoDataBuilder::new();
    // Server information
    builder.append(SqlInfo::FlightSqlServerName, "Micromegas Flight SQL Server");
    builder.append(SqlInfo::FlightSqlServerVersion, "1");
    // 1.3 comes from https://github.com/apache/arrow/blob/f9324b79bf4fc1ec7e97b32e3cce16e75ef0f5e3/format/Schema.fbs#L24
    builder.append(SqlInfo::FlightSqlServerArrowVersion, "1.3");
    builder.append(SqlInfo::SqlKeywords, SQL_INFO_SQL_KEYWORDS);
    builder.append(SqlInfo::SqlNumericFunctions, SQL_INFO_NUMERIC_FUNCTIONS);
    builder.append(SqlInfo::SqlStringFunctions, SQL_INFO_STRING_FUNCTIONS);
    builder.append(SqlInfo::SqlSystemFunctions, SQL_INFO_SYSTEM_FUNCTIONS);
    builder.append(SqlInfo::SqlDatetimeFunctions, SQL_INFO_DATE_TIME_FUNCTIONS);
    builder.build().unwrap()
});

/// Implementation of the Flight SQL service.
#[derive(Clone)]
pub struct FlightSqlServiceImpl {
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    view_factory: Arc<ViewFactory>,
    session_configurator: Arc<dyn SessionConfigurator>,
    read_policy: Arc<dyn ReadPolicy>,
    isolation_config: Arc<IsolationConfig>,
}

impl FlightSqlServiceImpl {
    pub fn new(
        lakehouse: Arc<LakehouseContext>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        view_factory: Arc<ViewFactory>,
        session_configurator: Arc<dyn SessionConfigurator>,
        read_policy: Arc<dyn ReadPolicy>,
        isolation_config: Arc<IsolationConfig>,
    ) -> Self {
        Self {
            lakehouse,
            part_provider,
            view_factory,
            session_configurator,
            read_policy,
            isolation_config,
        }
    }

    /// Resolves the [`CallerContext`] a request should plan under -- the seam's fail-closed
    /// resolver (#1369, AbAC Stage 1 §2/§8).
    ///
    /// **Absent-extension convention.** When no `AuthContext` extension is present at all (no
    /// auth provider configured, e.g. `--disable-auth`), the resolved scope is
    /// [`ReadScope::All`]. This is the *only* permissive branch here: `AuthService::call`
    /// (`rust/auth/src/tower.rs`) rejects the request before this inner service ever runs
    /// whenever a provider *is* configured, so the extension is always present in that case --
    /// the same safety argument `is_admin`'s absent-header-⇒-trusted convention already relies
    /// on (`user_attribution.rs`).
    ///
    /// **Failure convention.** A [`ReadPolicy::resolve`] that returns `Err` is a hard failure and
    /// must never become a scope -- not `ReadScope::Audiences(Arc::from([]))` (which would read
    /// as a legitimate, audited fail-closed decision to Stage 2/3) and not `ReadScope::All` (a
    /// silent fail-open bypass). Mirrors `tower.rs`'s own discriminator: `Status::unavailable`
    /// when the error downcasts to [`ProviderUnavailable`] (a store/provider outage),
    /// `Status::permission_denied` otherwise.
    ///
    /// `is_admin` is read from `md` unchanged (`is_admin(md)`), equivalent to reading
    /// `AuthContext.is_admin` off `ext` when the extension is present, since the header is
    /// derived from the same `AuthContext` with client-supplied copies stripped
    /// (`tower.rs:107-139`) -- and it is what preserves today's `--disable-auth`
    /// absent-header-⇒-trusted convention when `ext` has none.
    async fn caller_context(
        &self,
        ext: &http::Extensions,
        md: &MetadataMap,
    ) -> Result<CallerContext, Status> {
        let read_scope = match ext.get::<AuthContext>() {
            Some(auth_ctx) => match self.read_policy.resolve(auth_ctx).await {
                Ok(audiences) => ReadScope::Audiences(audiences.into_inner()),
                Err(e) => {
                    return Err(if e.downcast_ref::<ProviderUnavailable>().is_some() {
                        Status::unavailable(format!("read policy unavailable: {e:#}"))
                    } else {
                        Status::permission_denied(format!("read scope denied: {e:#}"))
                    });
                }
            },
            None => ReadScope::All,
        };
        Ok(CallerContext {
            read_scope,
            is_admin: is_admin(md),
            // Not permission-sensitive the way `read_scope` is (it's deployment config, not
            // derived from the caller's identity), so it is copied verbatim on both branches
            // above rather than participating in the absent-extension/`Err` distinction.
            isolation_config: self.isolation_config.clone(),
        })
    }

    fn should_preserve_dictionary(metadata: &MetadataMap) -> bool {
        metadata
            .get("preserve_dictionary")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    /// Extracts an optional client-attribution header, treating an empty string as absent.
    fn optional_metadata(metadata: &MetadataMap, key: &str) -> Option<String> {
        metadata
            .get(key)
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    #[span_fn]
    async fn execute_query(
        &self,
        ticket_stmt: TicketStatementQuery,
        metadata: &MetadataMap,
        extensions: &http::Extensions,
        client_ip: &str,
    ) -> Result<Response<FlightDataStream>, Status> {
        // Minted first, before any fallible step, so it's Always available --
        // for every client-facing `Status` built by `client_error`/
        // `classify_flight_error` and for this query's audit record (Design §3).
        let query_id = Uuid::new_v4().to_string();
        let begin_request = now();
        let request_start = Instant::now();
        let sql = std::str::from_utf8(&ticket_stmt.statement_handle)
            .map_err(|e| status!("Unable to parse query", e))?;

        let mut begin = metadata.get("query_range_begin");
        if let Some(s) = &begin
            && s.is_empty()
        {
            begin = None;
        }
        let mut end = metadata.get("query_range_end");
        if let Some(s) = &end
            && s.is_empty()
        {
            end = None;
        }
        let query_range = if begin.is_some() && end.is_some() {
            let begin_datetime =
                chrono::DateTime::parse_from_rfc3339(begin.unwrap().to_str().map_err(|e| {
                    client_input_error!("Unable to convert query_range_begin to string", e)
                })?)
                .map_err(|e| {
                    client_input_error!(
                        "Unable to parse query_range_begin as a rfc3339 datetime",
                        e
                    )
                })?;
            let end_datetime =
                chrono::DateTime::parse_from_rfc3339(end.unwrap().to_str().map_err(|e| {
                    client_input_error!("Unable to convert query_range_end to string", e)
                })?)
                .map_err(|e| {
                    client_input_error!("Unable to parse query_range_end as a rfc3339 datetime", e)
                })?;
            Some(TimeRange::new(begin_datetime.into(), end_datetime.into()))
        } else {
            None
        };

        // Validate and resolve user attribution
        let attr = validate_and_resolve_user_attribution_grpc(metadata).map_err(|e| *e)?;

        let client_type = metadata
            .get("x-client-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown");
        let client_agent = metadata
            .get("x-client-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown");
        let client_entrypoint = metadata
            .get("x-client-entrypoint")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown");
        let client_session = Self::optional_metadata(metadata, "x-client-session");
        let client_notebook = Self::optional_metadata(metadata, "x-client-notebook");
        let client_cell = Self::optional_metadata(metadata, "x-client-cell");

        let user_name_display = attr.user_name.as_deref().unwrap_or("");

        // Log query with full attribution
        if let Some(service_account_name) = &attr.service_account {
            info!(
                "execute_query range={query_range:?} sql={sql:?} limit={:?} user={} email={} name={user_name_display:?} service_account={service_account_name} client={client_type} agent={client_agent} entrypoint={client_entrypoint} notebook={client_notebook:?} cell={client_cell:?} client_ip={client_ip}",
                metadata.get("limit"),
                attr.user_id,
                attr.user_email
            );
        } else {
            info!(
                "execute_query range={query_range:?} sql={sql:?} limit={:?} user={} email={} name={user_name_display:?} client={client_type} agent={client_agent} entrypoint={client_entrypoint} notebook={client_notebook:?} cell={client_cell:?} client_ip={client_ip}",
                metadata.get("limit"),
                attr.user_id,
                attr.user_email
            );
        }

        // The per-query memory-pool wrapper is infallible to construct, so it's built
        // before `audit_state` and owned by it from the start -- every subsequent
        // setup failure (session context, planning, limit, physical plan, stream
        // construction) can then still emit an "error" audit record carrying whatever
        // peak the pool had accrued by that point, instead of silently disappearing on
        // an early `?` return.
        let scoped_pool = Arc::new(ScopedMemoryPool::new(
            self.lakehouse.runtime().memory_pool.clone(),
        ));

        // Attribution is resolved from here on, so build the audit state now
        // (durations/limit/plan filled in as they become known) instead of
        // only after the physical plan exists.
        let mut audit_state = QueryAuditState {
            query_id: query_id.clone(),
            client_ip: client_ip.to_string(),
            client: client_type.to_string(),
            agent: client_agent.to_string(),
            entrypoint: client_entrypoint.to_string(),
            session: client_session,
            notebook: client_notebook,
            cell: client_cell,
            user: attr.user_id.clone(),
            email: attr.user_email.clone(),
            name: attr.user_name.clone(),
            service_account: attr.service_account.is_some(),
            service_account_name: attr.service_account.clone(),
            sql: sql.to_string(),
            range_begin: query_range.as_ref().map(|r| r.begin.to_rfc3339()),
            range_end: query_range.as_ref().map(|r| r.end.to_rfc3339()),
            limit: None,
            context_init_ms: 0.0,
            planning_ms: 0.0,
            execution_ms: 0.0,
            setup_ms: 0.0,
            request_start,
            plan: None,
            pool: scoped_pool.clone(),
        };

        // Build a `RuntimeEnv`/`LakehouseContext` scoped to this query's memory-pool
        // wrapper, so every session context created from it (including nested ones,
        // e.g. Perfetto trace queries and JIT materialization) attributes its memory
        // to this query alone instead of the process-shared pool.
        let scoped_env = scoped_runtime(self.lakehouse.runtime(), scoped_pool.clone())
            .map_err(|e| audit_state.fail(status!("error building scoped runtime", e)))?;
        let lakehouse = self.lakehouse.with_runtime(scoped_env);

        // Session context creation phase
        let session_begin = now();
        let session_begin_instant = Instant::now();
        let caller = self
            .caller_context(extensions, metadata)
            .await
            .map_err(|status| audit_state.fail(status))?;
        let ctx = make_session_context(
            lakehouse,
            self.part_provider.clone(),
            query_range,
            self.view_factory.clone(),
            self.session_configurator.clone(),
            caller,
        )
        .await
        .map_err(|e| audit_state.fail(status!("error in make_session_context", e)))?;
        let context_init_duration = now() - session_begin;
        audit_state.context_init_ms = session_begin_instant.elapsed().as_secs_f64() * 1000.0;

        // Query planning phase
        let planning_begin = now();
        let planning_begin_instant = Instant::now();
        let mut df = ctx.sql(sql).await.map_err(|e| {
            audit_state.fail(client_error("error building dataframe", e, &query_id, None))
        })?;
        let planning_duration = now() - planning_begin;
        audit_state.planning_ms = planning_begin_instant.elapsed().as_secs_f64() * 1000.0;

        if let Some(limit_str) = metadata.get("limit") {
            let parsed_limit: usize = usize::from_str(limit_str.to_str().map_err(|e| {
                audit_state.fail(client_input_error!("error converting limit to str", e))
            })?)
            .map_err(|e| audit_state.fail(client_input_error!("error parsing limit", e)))?;
            audit_state.limit = Some(parsed_limit as u64);
            df = df.limit(0, Some(parsed_limit)).map_err(|e| {
                audit_state.fail(client_error(
                    "error building dataframe with limit",
                    e,
                    &query_id,
                    None,
                ))
            })?;
        }

        // Query execution phase: build the physical plan (kept for post-drain metrics)
        // and run it via the free-function `execute_stream`, which is what
        // `DataFrame::execute_stream` does internally, minus dropping the plan.
        let execution_begin = now();
        let execution_begin_instant = Instant::now();
        let schema = Arc::new(df.schema().as_arrow().clone());
        let task_ctx = Arc::new(df.task_ctx());
        let plan = df.create_physical_plan().await.map_err(|e| {
            audit_state.fail(client_error(
                "error creating physical plan",
                e,
                &query_id,
                None,
            ))
        })?;
        audit_state.plan = Some(plan.clone());
        // `plan`/`query_id` are cloned here because both are moved shortly:
        // `plan` into `execute_stream` below, `audit_state` (which owns
        // `query_id`'s sibling on the struct) into `CompletionTrackedStream::new`
        // further down. The per-batch closure below outlives this function call
        // (it's polled from the returned stream), so it needs its own owned
        // clones rather than borrowing `plan`/`query_id` directly (Design §2).
        let plan_for_errors = plan.clone();
        let query_id_for_stream = query_id.clone();
        let stream = execute_stream(plan, task_ctx)
            .map_err(|e| {
                audit_state.fail(client_error(
                    "error executing plan",
                    e,
                    &query_id_for_stream,
                    Some(&plan_for_errors),
                ))
            })?
            .map_err(|e| FlightError::ExternalError(Box::new(e)));
        let builder = if Self::should_preserve_dictionary(metadata) {
            FlightDataEncoderBuilder::new()
                .with_schema(schema.clone())
                .with_dictionary_handling(DictionaryHandling::Resend)
        } else {
            FlightDataEncoderBuilder::new().with_schema(schema.clone())
        };
        let flight_data_stream = builder.build(stream);
        let execution_duration = now() - execution_begin;
        audit_state.execution_ms = execution_begin_instant.elapsed().as_secs_f64() * 1000.0;

        // Calculate total setup time and record detailed metrics
        let total_setup_duration = now() - begin_request;
        audit_state.setup_ms = request_start.elapsed().as_secs_f64() * 1000.0;

        // Record detailed timing metrics
        imetric!(
            "context_init_duration",
            "ticks",
            context_init_duration as u64
        );
        imetric!("query_planning_duration", "ticks", planning_duration as u64);
        imetric!(
            "query_execution_duration",
            "ticks",
            execution_duration as u64
        );
        imetric!("query_setup_duration", "ticks", total_setup_duration as u64);

        // Create instrumented stream that tracks completion. This closure owns
        // `plan_for_errors`/`query_id_for_stream` (moved in) since it's polled
        // from the returned stream, long after this function returns -- see the
        // comment above `plan_for_errors`'s definition.
        let instrumented_stream = flight_data_stream.map_err(move |e| {
            classify_flight_error(e, &query_id_for_stream, Some(&plan_for_errors))
        });
        let completion_tracked_stream =
            CompletionTrackedStream::new(instrumented_stream.boxed(), begin_request, audit_state);
        Ok(Response::new(
            Box::pin(completion_tracked_stream) as FlightDataStream
        ))
    }
}

#[tonic::async_trait]
impl FlightSqlService for FlightSqlServiceImpl {
    type FlightService = FlightSqlServiceImpl;

    async fn do_handshake(
        &self,
        _request: Request<Streaming<HandshakeRequest>>,
    ) -> Result<
        Response<Pin<Box<dyn Stream<Item = Result<HandshakeResponse, Status>> + Send>>>,
        Status,
    > {
        api_entry_not_implemented!()
    }

    #[span_fn]
    async fn do_get_fallback(
        &self,
        request: Request<Ticket>,
        _message: Any,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let client_ip = get_client_ip(request.metadata().as_ref(), request.extensions());
        let ticket_stmt = TicketStatementQuery::decode(request.get_ref().ticket.clone())
            .map_err(|e| status!("Could not read ticket", e))?;
        self.execute_query(
            ticket_stmt,
            request.metadata(),
            request.extensions(),
            &client_ip,
        )
        .await
    }

    #[span_fn]
    async fn get_flight_info_statement(
        &self,
        query: CommandStatementQuery,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let begin_request = now();
        info!("get_flight_info_statement {query:?} ");
        let CommandStatementQuery { query, .. } = query;
        let schema = Schema::empty();
        let ticket = TicketStatementQuery {
            statement_handle: query.into(),
        };
        let mut bytes: Vec<u8> = Vec::new();
        if ticket.encode(&mut bytes).is_ok() {
            let info = FlightInfo::new()
                .try_with_schema(&schema)
                .unwrap()
                .with_endpoint(FlightEndpoint::new().with_ticket(Ticket::new(bytes)));
            let duration = now() - begin_request;
            imetric!("request_duration", "ticks", duration as u64);
            Ok(Response::new(info))
        } else {
            error!("Error encoding ticket");
            Err(Status::internal("Error encoding ticket"))
        }
    }

    async fn get_flight_info_substrait_plan(
        &self,
        _query: CommandStatementSubstraitPlan,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_prepared_statement(
        &self,
        _cmd: CommandPreparedStatementQuery,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_catalogs(
        &self,
        _query: CommandGetCatalogs,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_schemas(
        &self,
        _query: CommandGetDbSchemas,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    #[span_fn]
    async fn get_flight_info_tables(
        &self,
        query: CommandGetTables,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let begin_request = now();
        info!("get_flight_info_tables");
        let flight_descriptor = request.into_inner();
        let ticket = Ticket {
            ticket: query.as_any().encode_to_vec().into(),
        };
        let endpoint = FlightEndpoint::new().with_ticket(ticket);
        let flight_info = FlightInfo::new()
            .try_with_schema(&query.into_builder().schema())
            .map_err(|e| status!("Unable to encode schema", e))?
            .with_endpoint(endpoint)
            .with_descriptor(flight_descriptor);
        let duration = now() - begin_request;
        imetric!("request_duration", "ticks", duration as u64);
        Ok(tonic::Response::new(flight_info))
    }

    async fn get_flight_info_table_types(
        &self,
        _query: CommandGetTableTypes,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    #[span_fn]
    async fn get_flight_info_sql_info(
        &self,
        query: CommandGetSqlInfo,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let begin_request = now();
        info!("get_flight_info_sql_info");
        let flight_descriptor = request.into_inner();
        let ticket = Ticket::new(query.as_any().encode_to_vec());
        let endpoint = FlightEndpoint::new().with_ticket(ticket);
        let flight_info = FlightInfo::new()
            .try_with_schema(query.into_builder(&INSTANCE_SQL_DATA).schema().as_ref())
            .map_err(|e| status!("Unable to encode schema", e))?
            .with_endpoint(endpoint)
            .with_descriptor(flight_descriptor);
        let duration = now() - begin_request;
        imetric!("request_duration", "ticks", duration as u64);
        Ok(tonic::Response::new(flight_info))
    }

    async fn get_flight_info_primary_keys(
        &self,
        _query: CommandGetPrimaryKeys,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_exported_keys(
        &self,
        _query: CommandGetExportedKeys,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_imported_keys(
        &self,
        _query: CommandGetImportedKeys,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_cross_reference(
        &self,
        _query: CommandGetCrossReference,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    async fn get_flight_info_xdbc_type_info(
        &self,
        _query: CommandGetXdbcTypeInfo,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        api_entry_not_implemented!()
    }

    #[span_fn]
    async fn do_get_statement(
        &self,
        ticket: TicketStatementQuery,
        request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let client_ip = get_client_ip(request.metadata().as_ref(), request.extensions());
        self.execute_query(ticket, request.metadata(), request.extensions(), &client_ip)
            .await
    }

    async fn do_get_prepared_statement(
        &self,
        _query: CommandPreparedStatementQuery,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_catalogs(
        &self,
        _query: CommandGetCatalogs,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_schemas(
        &self,
        _query: CommandGetDbSchemas,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    #[span_fn]
    async fn do_get_tables(
        &self,
        query: CommandGetTables,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let begin_request = now();
        info!("do_get_tables {query:?}");
        let mut builder = query.into_builder();
        for view in self.view_factory.get_global_views() {
            let catalog_name = "";
            let schema_name = "";
            builder
                .append(
                    catalog_name,
                    schema_name,
                    &*view.get_view_set_name(),
                    "table",
                    &view.get_file_schema(),
                )
                .map_err(Status::from)?;
        }
        let schema = builder.schema();
        let batch = builder.build();
        let stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(futures::stream::once(async { batch }))
            .map_err(Status::from);
        let duration = now() - begin_request;
        imetric!("request_duration", "ticks", duration as u64);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn do_get_table_types(
        &self,
        _query: CommandGetTableTypes,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    #[span_fn]
    async fn do_get_sql_info(
        &self,
        query: CommandGetSqlInfo,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        info!("do_get_sql_info");
        let builder = query.into_builder(&INSTANCE_SQL_DATA);
        let schema = builder.schema();
        let batch = builder.build();
        let stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(futures::stream::once(async { batch }))
            .map_err(Status::from);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn do_get_primary_keys(
        &self,
        _query: CommandGetPrimaryKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_exported_keys(
        &self,
        _query: CommandGetExportedKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_imported_keys(
        &self,
        _query: CommandGetImportedKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_cross_reference(
        &self,
        _query: CommandGetCrossReference,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_get_xdbc_type_info(
        &self,
        _query: CommandGetXdbcTypeInfo,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        api_entry_not_implemented!()
    }

    async fn do_put_statement_update(
        &self,
        _ticket: CommandStatementUpdate,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        api_entry_not_implemented!()
    }

    /// `bulk_ingest` (routed here from `CommandStatementIngest`) is a replication/administrative
    /// API, not an ordinary analytics write path: it writes row properties -- including
    /// `micromegas.audience` on `processes` rows -- verbatim, with none of the server-side
    /// stamping or reserved-namespace stripping the HTTP ingestion paths apply. Gating this RPC
    /// on `is_admin` is what keeps that verbatim write safe: an admin-run replication tool
    /// re-ingesting rows already stamped at their origin lake is exactly the case the docs
    /// describe (`mkdocs/docs/query-guide/python-api.md`), while an ordinary authenticated
    /// analytics client must not be able to set `micromegas.audience` directly (#1373, AbAC
    /// Stage 5).
    #[span_fn]
    async fn do_put_statement_ingest(
        &self,
        command: CommandStatementIngest,
        request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        let table_name = command.table;
        info!("do_put_statement_ingest table_name={table_name}");
        if !is_admin(request.metadata()) {
            return Err(Status::permission_denied(
                "bulk_ingest requires admin privileges",
            ));
        }
        let stream = FlightRecordBatchStream::new_from_flight_data(
            request.into_inner().map_err(|e| e.into()),
        );
        bulk_ingest(self.lakehouse.lake().clone(), &table_name, stream)
            .await
            .map_err(|e| {
                let msg = format!("error ingesting into {table_name}: {e:?}");
                error!("{msg}");
                status!(msg, e)
            })
    }

    async fn do_put_substrait_plan(
        &self,
        _ticket: CommandStatementSubstraitPlan,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        api_entry_not_implemented!()
    }

    async fn do_put_prepared_statement_query(
        &self,
        _query: CommandPreparedStatementQuery,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<DoPutPreparedStatementResult, Status> {
        api_entry_not_implemented!()
    }

    async fn do_put_prepared_statement_update(
        &self,
        _query: CommandPreparedStatementUpdate,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        api_entry_not_implemented!()
    }

    #[span_fn]
    async fn do_action_create_prepared_statement(
        &self,
        query: ActionCreatePreparedStatementRequest,
        request: Request<Action>,
    ) -> Result<ActionCreatePreparedStatementResult, Status> {
        info!("do_action_create_prepared_statement query={}", &query.query);

        // Closes hole #1 (#1369): prepared statements now resolve the same CallerContext as the
        // do_get execute path, instead of reading only is_admin and no identity at all.
        let caller = self
            .caller_context(request.extensions(), request.metadata())
            .await?;
        let ctx = make_session_context(
            self.lakehouse.clone(),
            self.part_provider.clone(),
            None,
            self.view_factory.clone(),
            self.session_configurator.clone(),
            caller,
        )
        .await
        .map_err(|e| status!("error in make_session_context", e))?;

        // No `QueryAuditState`/audit record exists on this RPC (it only plans the
        // query, never executes it) -- this id exists solely so the client-facing
        // message and a server log line can be correlated with each other.
        let local_query_id = Uuid::new_v4().to_string();
        let df = ctx
            .sql(&query.query)
            .await
            .map_err(|e| client_error("error building dataframe", e, &local_query_id, None))?;
        let schema = df.schema().as_arrow();
        let mut schema_buffer = Vec::new();
        let mut writer = StreamWriter::try_new(&mut schema_buffer, schema)
            .map_err(|e| status!("error writing schema to in-memory buffer", e))?;
        writer
            .finish()
            .map_err(|e| status!("error closing arrow ipc stream writer", e))?;
        // here we could serialize the logical plan and return that as the prepared statement, but we would
        // need to register LogicalExtensionCodec for user-defined functions
        // instead, we are sending back the sql as we received it
        let result = ActionCreatePreparedStatementResult {
            prepared_statement_handle: query.query.into(),
            dataset_schema: schema_buffer.into(),
            parameter_schema: "".into(),
        };
        Ok(result)
    }

    async fn do_action_close_prepared_statement(
        &self,
        _query: ActionClosePreparedStatementRequest,
        _request: Request<Action>,
    ) -> Result<(), Status> {
        info!("do_action_close_prepared_statement");
        Ok(())
    }

    async fn do_action_create_prepared_substrait_plan(
        &self,
        _query: ActionCreatePreparedSubstraitPlanRequest,
        _request: Request<Action>,
    ) -> Result<ActionCreatePreparedStatementResult, Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_begin_transaction(
        &self,
        _query: ActionBeginTransactionRequest,
        _request: Request<Action>,
    ) -> Result<ActionBeginTransactionResult, Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_end_transaction(
        &self,
        _query: ActionEndTransactionRequest,
        _request: Request<Action>,
    ) -> Result<(), Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_begin_savepoint(
        &self,
        _query: ActionBeginSavepointRequest,
        _request: Request<Action>,
    ) -> Result<ActionBeginSavepointResult, Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_end_savepoint(
        &self,
        _query: ActionEndSavepointRequest,
        _request: Request<Action>,
    ) -> Result<(), Status> {
        api_entry_not_implemented!()
    }

    async fn do_action_cancel_query(
        &self,
        _query: ActionCancelQueryRequest,
        _request: Request<Action>,
    ) -> Result<ActionCancelQueryResult, Status> {
        api_entry_not_implemented!()
    }

    async fn register_sql_info(&self, _id: i32, _result: &SqlInfo) {
        info!("register_sql_info");
    }
}
