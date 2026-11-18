use super::config::McpConfig;
use super::flight::{FlightBackend, describe_error};
use super::provider::{CallContext, ToolProvider, error_result, input_schema, text_result};
use super::read_only::check_read_only;
use super::render::{RenderLimits, render_stream};
use arrow::array::{Array, RecordBatch};
use arrow::util::display::array_value_to_string;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use futures::StreamExt;
use micromegas_telemetry::time_range::TimeRange;
use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use serde::Deserialize;
use std::sync::Arc;

const QUERY_RULES: &str = "Rules: always pass begin (and end) to bound the time range, never filter \
on time in the WHERE clause instead; always add a LIMIT unless the query is a small aggregate; \
use view_instance() for single-process or single-stream queries; read describe_view for the \
view's own rules before querying it. Only read-only SELECT queries are accepted.";

const MAX_VIEW_NAME_LEN: usize = 128;

#[derive(Debug, Deserialize)]
struct QueryArgs {
    sql: String,
    begin: Option<String>,
    end: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct DescribeViewArgs {
    name: String,
}

/// Parses an RFC 3339 timestamp or a relative age such as `30m`, `1h`, `7d` (before `now`).
pub fn parse_time(value: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let value = value.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(value) {
        return Ok(t.with_timezone(&Utc));
    }
    let relative = value.char_indices().last().and_then(|(i, unit)| {
        let n: i64 = value[..i].parse().ok()?;
        let delta = match unit {
            's' => Duration::try_seconds(n)?,
            'm' => Duration::try_minutes(n)?,
            'h' => Duration::try_hours(n)?,
            'd' => Duration::try_days(n)?,
            'w' => Duration::try_weeks(n)?,
            _ => return None,
        };
        now.checked_sub_signed(delta)
    });
    relative.ok_or_else(|| {
        format!("invalid time '{value}': use an RFC 3339 timestamp or a relative age like 1h, 7d")
    })
}

/// Resolves the optional `begin`/`end` arguments. A lone `begin` runs up to `now`.
pub fn parse_range(
    begin: Option<&str>,
    end: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Option<TimeRange>, String> {
    match (begin, end) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err("end requires begin".to_string()),
        (Some(begin), end) => {
            let begin = parse_time(begin, now)?;
            let end = match end {
                Some(end) => parse_time(end, now)?,
                None => now,
            };
            if begin > end {
                return Err("begin must not be after end".to_string());
            }
            Ok(Some(TimeRange::new(begin, end)))
        }
    }
}

/// The query and discovery tools, all read-only.
pub struct QueryToolProvider {
    backend: Arc<dyn FlightBackend>,
    config: McpConfig,
}

impl QueryToolProvider {
    pub fn new(backend: Arc<dyn FlightBackend>, config: McpConfig) -> Self {
        Self { backend, config }
    }

    fn query_properties() -> serde_json::Value {
        serde_json::json!({
            "sql": {"type": "string", "description": "A single read-only SELECT query."},
            "begin": {"type": "string", "description": "Start of the time range: RFC 3339 or a relative age such as 1h or 7d."},
            "end": {"type": "string", "description": "End of the time range (defaults to now when begin is set)."},
            "limit": {"type": "integer", "minimum": 1, "description": "Maximum rows to return."}
        })
    }

    async fn prepare_query(&self, ctx: &CallContext, args: JsonObject) -> CallToolResult {
        let (args, range) = match self.parse_query_args(args) {
            Ok(v) => v,
            Err(e) => return error_result(e),
        };
        let estimate = match self.backend.prepare(ctx, &args.sql, range).await {
            Ok(estimate) => estimate,
            Err(e) => return error_result(describe_error(&e)),
        };
        let mut out = String::from("schema:\n");
        for field in estimate.schema.fields() {
            out.push_str(&format!("- {}: {}\n", field.name(), field.data_type()));
        }
        match estimate.total_bytes {
            Some(bytes) => out.push_str(&format!("estimated_scan_bytes: {bytes}\n")),
            None => out.push_str("estimated_scan_bytes: unknown\n"),
        }
        out.push_str(&format!(
            "estimate_precision: {}\n",
            estimate.estimate_precision.as_deref().unwrap_or("unknown")
        ));
        out.push_str(&format!(
            "within_scan_cap: {}\n",
            match estimate.within_scan_cap {
                Some(true) => "true",
                Some(false) => "false",
                None => "unknown",
            }
        ));
        if estimate.within_scan_cap == Some(false) {
            out.push_str(
                "hint: the estimated scan exceeds the cap; narrow begin/end or filter on the \
                 view's sort columns (see describe_view).\n",
            );
        } else if estimate.estimate_precision.as_deref() == Some("opaque") && range.is_none() {
            out.push_str(
                "hint: the scan cannot be estimated without a time range; pass begin/end or \
                 filter on the view's sort columns.\n",
            );
        }
        text_result(out)
    }

    async fn execute_query(&self, ctx: &CallContext, args: JsonObject) -> CallToolResult {
        let (args, range) = match self.parse_query_args(args) {
            Ok(v) => v,
            Err(e) => return error_result(e),
        };
        let limit = self.config.effective_limit(args.limit);
        // One extra row tells the renderer the result was cut without a second request.
        let stream = match self.backend.execute(ctx, &args.sql, range, limit + 1).await {
            Ok(stream) => stream,
            Err(e) => return error_result(describe_error(&e)),
        };
        let limits = RenderLimits {
            max_rows: limit,
            max_bytes: self.config.max_bytes,
        };
        match render_stream(stream, limits).await {
            Ok(rendered) => text_result(rendered.text),
            Err(e) => error_result(describe_error(&e)),
        }
    }

    fn parse_query_args(&self, args: JsonObject) -> Result<(QueryArgs, Option<TimeRange>), String> {
        let args: QueryArgs = serde_json::from_value(serde_json::Value::Object(args))
            .map_err(|e| format!("invalid arguments: {e}"))?;
        check_read_only(&args.sql)?;
        let range = parse_range(args.begin.as_deref(), args.end.as_deref(), Utc::now())?;
        Ok((args, range))
    }

    async fn fetch_rows(
        &self,
        ctx: &CallContext,
        sql: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        let mut stream = self.backend.execute(ctx, sql, None, limit).await?;
        let mut rows = vec![];
        while let Some(batch) = stream.next().await {
            let batch: RecordBatch = batch?;
            for row in 0..batch.num_rows() {
                let mut cells = vec![];
                for column in batch.columns() {
                    cells.push(if column.is_null(row) {
                        None
                    } else {
                        Some(array_value_to_string(column, row)?)
                    });
                }
                rows.push(cells);
            }
        }
        Ok(rows)
    }

    async fn list_views(&self, ctx: &CallContext) -> CallToolResult {
        let sql = "SELECT view_set_name, description FROM list_view_sets() \
                   ORDER BY view_set_name LIMIT 1000";
        match self.fetch_rows(ctx, sql, 1000).await {
            Ok(rows) => {
                let mut out = String::new();
                for row in rows {
                    let name = row.first().cloned().flatten().unwrap_or_default();
                    let summary = row
                        .get(1)
                        .cloned()
                        .flatten()
                        .and_then(|d| d.lines().find(|l| !l.trim().is_empty()).map(str::to_string))
                        .unwrap_or_default();
                    out.push_str(&format!("- {name}: {summary}\n"));
                }
                text_result(out)
            }
            Err(e) => error_result(describe_error(&e)),
        }
    }

    async fn describe_view(&self, ctx: &CallContext, args: JsonObject) -> CallToolResult {
        let args: DescribeViewArgs = match serde_json::from_value(serde_json::Value::Object(args)) {
            Ok(a) => a,
            Err(e) => return error_result(format!("invalid arguments: {e}")),
        };
        let name = args.name.trim();
        if name.is_empty()
            || name.len() > MAX_VIEW_NAME_LEN
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return error_result("invalid view name");
        }
        let sql = format!(
            "SELECT view_set_name, description, columns, sort_order, has_view_maker, \
             global_instance_available FROM list_view_sets() WHERE view_set_name = '{name}' LIMIT 1"
        );
        let rows = match self.fetch_rows(ctx, &sql, 1).await {
            Ok(rows) => rows,
            Err(e) => return error_result(describe_error(&e)),
        };
        let Some(row) = rows.first() else {
            return error_result(format!("no view named '{name}'; use list_views"));
        };
        let cell = |i: usize| row.get(i).cloned().flatten();
        let mut out = format!("# {name}\n\n");
        out.push_str(&cell(1).unwrap_or_else(|| "(no description)".to_string()));
        out.push_str("\n\n## columns\n");
        out.push_str(&cell(2).unwrap_or_default());
        out.push_str("\n\n## sort order\n");
        out.push_str(&cell(3).unwrap_or_else(|| "unordered".to_string()));
        out.push_str(&format!(
            "\n\nglobal_view_available: {}\nview_instance_available: {}\n",
            cell(5).as_deref().unwrap_or("false"),
            cell(4).as_deref().unwrap_or("false"),
        ));
        if cell(5).as_deref() == Some("false") {
            out.push_str("There is no global view: query it with view_instance().\n");
        }
        text_result(out)
    }
}

#[async_trait]
impl ToolProvider for QueryToolProvider {
    fn tools(&self, _ctx: &CallContext) -> Vec<Tool> {
        let read_only = || ToolAnnotations::new().read_only(true).open_world(false);
        let prepare_desc = format!(
            "Plan a query without running it: returns the result schema and, when the server \
             reports one, the estimated scan size and whether it is within the scan cap. \
             {QUERY_RULES}"
        );
        let execute_desc = format!(
            "Run a read-only query and return a markdown table, cut at the row and byte caps. \
             Takes the same arguments as prepare_query; re-send the full query. {QUERY_RULES}"
        );
        vec![
            Tool::new(
                "prepare_query",
                prepare_desc,
                input_schema(Self::query_properties(), &["sql"]),
            )
            .with_annotations(read_only()),
            Tool::new(
                "execute_query",
                execute_desc,
                input_schema(Self::query_properties(), &["sql"]),
            )
            .with_annotations(read_only()),
            Tool::new(
                "list_views",
                "List the queryable views with the first line of their description.",
                input_schema(serde_json::json!({}), &[]),
            )
            .with_annotations(read_only()),
            Tool::new(
                "describe_view",
                "Describe a view: usage rules (binding), columns and sort order. Read this \
                 before querying a view.",
                input_schema(
                    serde_json::json!({"name": {"type": "string", "description": "View name from list_views."}}),
                    &["name"],
                ),
            )
            .with_annotations(read_only()),
        ]
    }

    async fn call_tool(
        &self,
        ctx: &CallContext,
        name: &str,
        arguments: JsonObject,
    ) -> Option<CallToolResult> {
        Some(match name {
            "prepare_query" => self.prepare_query(ctx, arguments).await,
            "execute_query" => self.execute_query(ctx, arguments).await,
            "list_views" => self.list_views(ctx).await,
            "describe_view" => self.describe_view(ctx, arguments).await,
            _ => return None,
        })
    }
}
