use super::flightsql_client::Client;
use anyhow::Result;
use chrono::{DateTime, Utc};
use datafusion::arrow::array::BinaryArray;
use micromegas_analytics::dfext::typed_column::typed_column_by_name;
use micromegas_analytics::time::TimeRange;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

/// Span types to include in the trace
pub enum SpanTypes {
    Thread,
    Async,
    Both,
}

impl SpanTypes {
    fn as_str(&self) -> &'static str {
        match self {
            SpanTypes::Thread => "thread",
            SpanTypes::Async => "async",
            SpanTypes::Both => "both",
        }
    }
}

/// Formats a Perfetto trace with configurable span types using server-side perfetto_trace_chunks function.
///
/// This function queries the FlightSQL server using the perfetto_trace_chunks table function
/// which generates Perfetto trace data server-side and streams it back as binary chunks.
///
/// # Arguments
/// * `span_types` - Types of spans to include: Thread, Async, or Both
pub async fn format_perfetto_trace(
    client: &mut Client,
    process_id: &str,
    query_range: TimeRange,
    span_types: SpanTypes,
) -> Result<Vec<u8>> {
    // Use the perfetto_trace_chunks table function to get binary chunks
    // Note: ORDER BY not needed since chunks are naturally produced in order (0, 1, 2, ...)
    let sql = format!(
        r#"
        SELECT chunk_id, chunk_data
        FROM perfetto_trace_chunks(
            '{}',
            '{}',
            TIMESTAMP '{}',
            TIMESTAMP '{}'
        )
        "#,
        process_id,
        span_types.as_str(),
        query_range.begin.to_rfc3339(),
        query_range.end.to_rfc3339()
    );

    let batches = client.query(sql, Some(query_range)).await?;

    // Collect all chunks and reassemble them in order
    let mut trace_data = Vec::new();
    for batch in batches {
        let chunk_data: &BinaryArray = typed_column_by_name(&batch, "chunk_data")?;

        // Chunks are already in order from server-side generation
        for i in 0..batch.num_rows() {
            let chunk = chunk_data.value(i);
            trace_data.extend_from_slice(chunk);
        }
    }

    if trace_data.is_empty() {
        anyhow::bail!("No trace data generated for process {}", process_id);
    }

    Ok(trace_data)
}

/// Writes a Perfetto trace to a file with configurable span types.
///
/// This function generates traces with thread spans, async spans, or both.
///
/// # Arguments
/// * `span_types` - Types of spans to include: Thread, Async, or Both
pub async fn write_perfetto_trace(
    client: &mut Client,
    process_id: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    out_filename: &str,
    span_types: SpanTypes,
) -> Result<()> {
    let buf =
        format_perfetto_trace(client, process_id, TimeRange::new(begin, end), span_types).await?;
    let mut file = File::create(out_filename).await?;
    file.write_all(&buf).await?;
    Ok(())
}
