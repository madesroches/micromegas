use super::flightsql_client::Client;
use crate::perfetto::writer::Writer;
use anyhow::Result;
use chrono::{DateTime, Utc};
use datafusion::arrow::array::{StringArray, TimestampNanosecondArray, UInt32Array};
use micromegas_analytics::dfext::typed_column::typed_column_by_name;
use micromegas_analytics::time::TimeRange;
use prost::Message;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

async fn get_process_exe(
    process_id: &str,
    client: &mut Client,
    query_range: TimeRange,
) -> Result<String> {
    let sql_process = format!(
        r#"
        SELECT "processes.exe" as exe
        FROM blocks
        WHERE process_id = '{process_id}'
        LIMIT 1
        "#
    );
    let batches = client.query(sql_process, Some(query_range)).await?;
    if batches.len() != 1 || batches[0].num_rows() != 1 {
        anyhow::bail!("process not found");
    }
    let exes: &StringArray = typed_column_by_name(&batches[0], "exe")?;
    Ok(exes.value(0).to_owned())
}

/// Formats a Perfetto trace from the telemetry data.
///
/// This function queries the FlightSQL server for process, thread, and span information
/// and formats it into a Perfetto trace protobuf.
pub async fn format_perfetto_trace(
    client: &mut Client,
    process_id: &str,
    query_range: TimeRange,
) -> Result<Vec<u8>> {
    let mut writer = Writer::new(process_id);
    let exe = get_process_exe(process_id, client, query_range).await?;
    writer.append_process_descriptor(&exe);

    let sql_streams = format!(
        r#"
        SELECT stream_id,
               property_get("streams.properties", 'thread-name') as thread_name,
               property_get("streams.properties", 'thread-id') as thread_id
        FROM blocks
        WHERE process_id = '{process_id}'
        AND array_has("streams.tags", 'cpu')
        GROUP BY stream_id, thread_name, thread_id
    "#
    );

    let batches = client.query(sql_streams, Some(query_range)).await?;
    for b in batches {
        let stream_id_column: &StringArray = typed_column_by_name(&b, "stream_id")?;
        let thread_name_column: &StringArray = typed_column_by_name(&b, "thread_name")?;
        let thread_id_column: &StringArray = typed_column_by_name(&b, "thread_id")?;
        for row in 0..b.num_rows() {
            let stream_id = stream_id_column.value(row);
            let thread_name = thread_name_column.value(row);
            let thread_id_str = thread_id_column.value(row);
            // Thread IDs from the database might be too large for i32
            // Use a hash or truncate to fit in i32 range
            let thread_id: i32 = if let Ok(id) = thread_id_str.parse::<i32>() {
                id
            } else {
                // If parsing fails, use a hash of the thread_id string
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                thread_id_str.hash(&mut hasher);
                hasher.finish() as i32
            };
            writer.append_thread_descriptor(stream_id, thread_id, thread_name);

            let sql_spans = format!(
                r#"
                SELECT id, parent, depth, hash, begin, end, duration, name, target, filename, line
                FROM view_instance('thread_spans', '{stream_id}');
            "#
            );
            let span_batches = client.query(sql_spans, Some(query_range)).await?;
            for b in span_batches {
                let begins: &TimestampNanosecondArray = typed_column_by_name(&b, "begin")?;
                let ends: &TimestampNanosecondArray = typed_column_by_name(&b, "end")?;
                let names: &StringArray = typed_column_by_name(&b, "name")?;
                let targets: &StringArray = typed_column_by_name(&b, "target")?;
                let filenames: &StringArray = typed_column_by_name(&b, "filename")?;
                let lines: &UInt32Array = typed_column_by_name(&b, "line")?;
                for row in 0..b.num_rows() {
                    let begin_ns = begins.value(row);
                    let end_ns = ends.value(row);
                    
                    // These timestamps are absolute UTC nanosecond timestamps as i64
                    // Convert to u64 by treating negative values as errors
                    let begin_u64: u64 = if begin_ns < 0 {
                        anyhow::bail!("Negative begin timestamp: {} - this indicates a data issue", begin_ns);
                    } else {
                        begin_ns as u64
                    };
                    
                    let end_u64: u64 = if end_ns < 0 {
                        anyhow::bail!("Negative end timestamp: {} - this indicates a data issue", end_ns);
                    } else {
                        end_ns as u64
                    };
                    
                    writer.append_span(
                        begin_u64,
                        end_u64,
                        names.value(row),
                        targets.value(row),
                        filenames.value(row),
                        lines.value(row),
                    );
                }
            }
        }
    }

    Ok(writer.into_trace().encode_to_vec())
}

/// Writes a Perfetto trace to a file.
///
/// This function calls `format_perfetto_trace` to generate the trace data
/// and then writes it to the specified output file.
pub async fn write_perfetto_trace(
    client: &mut Client,
    process_id: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    out_filename: &str,
) -> Result<()> {
    let buf = format_perfetto_trace(client, process_id, TimeRange::new(begin, end)).await?;
    let mut file = File::create(out_filename).await?;
    file.write_all(&buf).await?;
    Ok(())
}
