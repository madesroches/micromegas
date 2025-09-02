use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::Parser;
use datafusion::arrow::array::{BinaryArray, Int32Array};
use futures::stream::StreamExt;
use micromegas::client::flightsql_client::Client;
use micromegas::client::query_processes::ProcessQueryBuilder;
use micromegas::micromegas_main;
use micromegas_analytics::dfext::typed_column::typed_column_by_name;
use micromegas_analytics::time::TimeRange;
use std::fs::File;
use std::io::Write;
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

    /// Types of spans to include: 'thread', 'async', or 'both'
    #[arg(long, default_value = "both")]
    span_types: String,
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Validate span_types argument
    if !["thread", "async", "both"].contains(&args.span_types.as_str()) {
        anyhow::bail!(
            "Invalid span_types '{}'. Must be 'thread', 'async', or 'both'",
            args.span_types
        );
    }

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
            let batches = ProcessQueryBuilder::new()
                .with_process_id(&args.process_id)
                .query(&mut client)
                .await?;
            if batches.is_empty() || batches[0].num_rows() == 0 {
                anyhow::bail!("Process {} not found", args.process_id);
            }
            let begin_times: &datafusion::arrow::array::TimestampNanosecondArray =
                typed_column_by_name(&batches[0], "begin")?;
            let end_times: &datafusion::arrow::array::TimestampNanosecondArray =
                typed_column_by_name(&batches[0], "end")?;
            let min_time = DateTime::from_timestamp_nanos(begin_times.value(0));
            let max_time = DateTime::from_timestamp_nanos(end_times.value(0));
            TimeRange::new(min_time, max_time)
        }
    };

    println!(
        "Generating {} spans for process {} in time range {} to {}",
        args.span_types, args.process_id, time_range.begin, time_range.end
    );

    // Generate trace using perfetto_trace_chunks table function
    generate_trace_from_chunks(
        &mut client,
        &args.process_id,
        &args.span_types,
        time_range,
        &args.output,
    )
    .await?;

    println!("Trace generated successfully: {}", args.output);

    Ok(())
}

/// Generate trace using the perfetto_trace_chunks table function with streaming API
async fn generate_trace_from_chunks(
    client: &mut Client,
    process_id: &str,
    span_types: &str,
    time_range: TimeRange,
    output_path: &str,
) -> Result<()> {
    // Build SQL query to get trace chunks using the table function
    // Note: ORDER BY chunk_id is not needed since chunks are naturally produced in order
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
        span_types,
        time_range.begin.to_rfc3339(),
        time_range.end.to_rfc3339()
    );

    println!("Streaming perfetto trace chunks...");

    // Create output file
    let mut file = File::create(output_path)?;

    let mut total_chunks = 0;
    let mut total_bytes = 0;
    let mut expected_chunk_id = 0;
    let mut has_chunks = false;

    // Use streaming interface to process chunks as they arrive
    let mut stream = client.query_stream(sql, Some(time_range)).await?;

    // Process each record batch as it arrives
    while let Some(record_batch_result) = stream.next().await {
        let record_batch = record_batch_result?;
        has_chunks = true;
        let chunk_ids: &Int32Array = typed_column_by_name(&record_batch, "chunk_id")?;
        let chunk_data: &BinaryArray = typed_column_by_name(&record_batch, "chunk_data")?;

        // Process each chunk in the batch
        for i in 0..record_batch.num_rows() {
            let chunk_id = chunk_ids.value(i);
            let data = chunk_data.value(i);

            // Verify chunks are in order
            if chunk_id != expected_chunk_id {
                anyhow::bail!(
                    "Chunk {} received, expected {}. Chunks may be out of order or missing!",
                    chunk_id,
                    expected_chunk_id
                );
            }

            // Write chunk directly to file
            file.write_all(data)?;

            total_chunks += 1;
            total_bytes += data.len();
            expected_chunk_id += 1;

            // Progress reporting
            if total_chunks == 1 {
                println!("  Writing chunks to {}...", output_path);
            } else if total_chunks % 10 == 0 {
                println!(
                    "  Processed {} chunks ({} bytes)...",
                    total_chunks, total_bytes
                );
            }
        }
    }

    if !has_chunks {
        anyhow::bail!("No trace chunks found for process {}", process_id);
    }

    // Flush and close file
    file.flush()?;

    println!(
        "Generated trace with {} chunks ({} bytes)",
        total_chunks, total_bytes
    );

    Ok(())
}
