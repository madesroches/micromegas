use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::Parser;
use datafusion::arrow::array::{Int64Array, StringArray, TimestampNanosecondArray, UInt32Array};
use micromegas::client::flightsql_client::Client;
use micromegas::micromegas_main;
use micromegas::tracing::prelude::*;
use micromegas_analytics::dfext::typed_column::typed_column_by_name;
use micromegas_analytics::time::TimeRange;
use micromegas_perfetto::StreamingPerfettoWriter;
use std::collections::HashMap;
use std::io::Write;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tonic::transport::Channel;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Process ID to generate trace for
    #[arg(short, long)]
    process_id: String,

    /// Output Perfetto trace file path
    #[arg(short, long, default_value = "trace.perfetto")]
    output: String,

    /// FlightSQL server URL
    #[arg(long, default_value = "http://127.0.0.1:50051")]
    flightsql_url: String,

    /// Start time for trace (RFC 3339 format, optional)
    #[arg(long)]
    start_time: Option<DateTime<Utc>>,

    /// End time for trace (RFC 3339 format, optional)
    #[arg(long)]
    end_time: Option<DateTime<Utc>>,
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Args::parse();

    println!(
        "Connecting to FlightSQL server at {}...",
        args.flightsql_url
    );
    let channel = Channel::from_shared(args.flightsql_url)?.connect().await?;
    let mut client = Client::new(channel);

    // Determine time range
    let time_range = match (args.start_time, args.end_time) {
        (Some(start), Some(end)) => TimeRange::new(start, end),
        _ => {
            println!("No time range specified, using process lifetime...");
            get_process_time_range(&args.process_id, &mut client).await?
        }
    };

    println!(
        "Generating trace for process {} in time range {} to {}",
        args.process_id, time_range.begin, time_range.end
    );

    // Create output file
    let mut output_file = File::create(&args.output).await?;
    let mut buffer = Vec::new();

    {
        let mut writer = StreamingPerfettoWriter::new(&mut buffer, &args.process_id);

        // Generate the trace
        generate_trace(&mut writer, &args.process_id, &mut client, time_range).await?;

        writer.flush()?;
    }

    // Write to file
    output_file.write_all(&buffer).await?;
    output_file.flush().await?;

    println!(
        "Trace generated successfully: {} ({} bytes)",
        args.output,
        buffer.len()
    );

    Ok(())
}

async fn get_process_time_range(process_id: &str, client: &mut Client) -> Result<TimeRange> {
    let sql = format!(
        r#"
        SELECT MIN(begin_time) as min_time, MAX(end_time) as max_time
        FROM blocks
        WHERE process_id = '{}'
        "#,
        process_id
    );

    let batches = client.query(sql, None).await?;
    if batches.is_empty() || batches[0].num_rows() == 0 {
        anyhow::bail!("Process {} not found", process_id);
    }

    let min_times: &TimestampNanosecondArray = typed_column_by_name(&batches[0], "min_time")?;
    let max_times: &TimestampNanosecondArray = typed_column_by_name(&batches[0], "max_time")?;

    let min_time = DateTime::from_timestamp_nanos(min_times.value(0));
    let max_time = DateTime::from_timestamp_nanos(max_times.value(0));

    Ok(TimeRange::new(min_time, max_time))
}

async fn get_process_exe(
    process_id: &str,
    client: &mut Client,
    time_range: TimeRange,
) -> Result<String> {
    let sql = format!(
        r#"
        SELECT "processes.exe" as exe
        FROM blocks
        WHERE process_id = '{}'
        LIMIT 1
        "#,
        process_id
    );

    let batches = client.query(sql, Some(time_range)).await?;
    if batches.is_empty() || batches[0].num_rows() == 0 {
        anyhow::bail!("Process {} not found", process_id);
    }

    let exes: &StringArray = typed_column_by_name(&batches[0], "exe")?;
    Ok(exes.value(0).to_owned())
}

async fn generate_trace<W: Write>(
    writer: &mut StreamingPerfettoWriter<W>,
    process_id: &str,
    client: &mut Client,
    time_range: TimeRange,
) -> Result<()> {
    // Get process info and emit process descriptor
    let exe = get_process_exe(process_id, client, time_range).await?;
    writer.emit_process_descriptor(&exe)?;

    // Get thread information and emit thread descriptors
    let threads = get_thread_info(process_id, client, time_range).await?;
    for (stream_id, (thread_id, thread_name)) in &threads {
        writer.emit_thread_descriptor(stream_id, *thread_id, thread_name)?;
    }

    // Emit async track descriptor for async spans
    writer.emit_async_track_descriptor()?;

    // Generate thread spans
    generate_thread_spans(writer, process_id, client, time_range, &threads).await?;

    // Generate async spans
    generate_async_spans(writer, process_id, client, time_range).await?;

    Ok(())
}

async fn get_thread_info(
    process_id: &str,
    client: &mut Client,
    time_range: TimeRange,
) -> Result<HashMap<String, (i32, String)>> {
    let sql = format!(
        r#"
        SELECT DISTINCT stream_id
        FROM streams
        WHERE process_id = '{}'
        AND array_has(tags, 'cpu')
        ORDER BY stream_id
        "#,
        process_id
    );

    let batches = client.query(sql, Some(time_range)).await?;
    let mut threads = HashMap::new();

    for batch in batches {
        let stream_ids: &StringArray = typed_column_by_name(&batch, "stream_id")?;

        for i in 0..batch.num_rows() {
            let stream_id = stream_ids.value(i).to_owned();
            // Use a hash of the stream_id as the thread ID and a default name
            let tid = (stream_id
                .as_bytes()
                .iter()
                .fold(0u32, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u32))
                % 65536) as i32;
            let name = format!("Thread-{}", stream_id);
            threads.insert(stream_id, (tid, name));
        }
    }

    Ok(threads)
}

async fn generate_thread_spans<W: Write>(
    writer: &mut StreamingPerfettoWriter<W>,
    _process_id: &str,
    client: &mut Client,
    time_range: TimeRange,
    threads: &HashMap<String, (i32, String)>,
) -> Result<()> {
    for stream_id in threads.keys() {
        // Set the current thread for this stream (needed before emitting spans)
        writer.emit_thread_descriptor(stream_id, threads[stream_id].0, &threads[stream_id].1)?;

        let sql = format!(
            r#"
            SELECT begin, end, name, filename, target, line
            FROM view_instance('thread_spans', '{}')
            WHERE begin >= '{}' AND end <= '{}'
            ORDER BY begin
            "#,
            stream_id,
            time_range.begin.to_rfc3339(),
            time_range.end.to_rfc3339()
        );

        let batches = client.query(sql, Some(time_range)).await?;

        for batch in batches {
            let begin_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "begin")?;
            let end_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "end")?;
            let names: &StringArray = typed_column_by_name(&batch, "name")?;
            let filenames: &StringArray = typed_column_by_name(&batch, "filename")?;
            let targets: &StringArray = typed_column_by_name(&batch, "target")?;
            let lines: &UInt32Array = typed_column_by_name(&batch, "line")?;

            for i in 0..batch.num_rows() {
                let begin_ns = begin_times.value(i) as u64;
                let end_ns = end_times.value(i) as u64;
                let name = names.value(i);
                let filename = filenames.value(i);
                let target = targets.value(i);
                let line = lines.value(i);

                writer.emit_span(begin_ns, end_ns, name, target, filename, line)?;
            }
        }
    }

    Ok(())
}

async fn generate_async_spans<W: Write>(
    writer: &mut StreamingPerfettoWriter<W>,
    process_id: &str,
    client: &mut Client,
    time_range: TimeRange,
) -> Result<()> {
    let sql = format!(
        r#"
        WITH begin_events AS (
            SELECT span_id, time as begin_time, name, filename, target, line
            FROM view_instance('async_events', '{}')
            WHERE time >= '{}' AND time <= '{}'
              AND event_type = 'begin'
        ),
        end_events AS (
            SELECT span_id, time as end_time
            FROM view_instance('async_events', '{}')
            WHERE time >= '{}' AND time <= '{}'
              AND event_type = 'end'
        )
        SELECT 
            b.span_id,
            b.begin_time,
            e.end_time,
            b.name,
            b.filename,
            b.target,
            b.line
        FROM begin_events b
        INNER JOIN end_events e ON b.span_id = e.span_id
        ORDER BY b.begin_time
        "#,
        process_id,
        time_range.begin.to_rfc3339(),
        time_range.end.to_rfc3339(),
        process_id,
        time_range.begin.to_rfc3339(),
        time_range.end.to_rfc3339()
    );

    let batches = client.query(sql, Some(time_range)).await?;

    // Process spans directly - each row represents a complete span with begin/end times
    for batch in batches {
        let _span_ids: &Int64Array = typed_column_by_name(&batch, "span_id")?;
        let begin_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "begin_time")?;
        let end_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "end_time")?;
        let names: &StringArray = typed_column_by_name(&batch, "name")?;
        let filenames: &StringArray = typed_column_by_name(&batch, "filename")?;
        let targets: &StringArray = typed_column_by_name(&batch, "target")?;
        let lines: &UInt32Array = typed_column_by_name(&batch, "line")?;

        for i in 0..batch.num_rows() {
            let begin_ns = begin_times.value(i) as u64;
            let end_ns = end_times.value(i) as u64;
            let name = names.value(i);
            let filename = filenames.value(i);
            let target = targets.value(i);
            let line = lines.value(i);

            if end_ns >= begin_ns {
                // Emit begin and end events consecutively for each span
                writer.emit_async_span_begin(begin_ns, name, target, filename, line)?;
                writer.emit_async_span_end(end_ns, name, target, filename, line)?;
            } else {
                let negative_duration_ns = begin_ns - end_ns;
                let negative_duration_ms = negative_duration_ns as f64 / 1_000_000.0;
                warn!("invalid span duration '{name}': {negative_duration_ms:.3}ms");
            }
        }
    }

    Ok(())
}
