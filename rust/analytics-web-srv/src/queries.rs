use anyhow::Result;
use arrow_flight::decode::FlightRecordBatchStream;
use datafusion::arrow::array::RecordBatch;
use micromegas::client::flightsql_client::Client;

/// Query all processes ordered by last update time
pub async fn query_all_processes(client: &mut Client) -> Result<Vec<RecordBatch>> {
    let sql = "SELECT process_id, start_time, last_update_time, exe, computer, username, cpu_brand, distro, properties
               FROM processes
               ORDER BY last_update_time DESC";
    client.query(sql.to_owned(), None).await
}

/// Query log entries for a process with optional level filter
pub async fn query_log_entries(
    client: &mut Client,
    process_id: &str,
    level_filter: Option<&str>,
    limit: usize,
) -> Result<FlightRecordBatchStream> {
    let level_condition = match level_filter {
        Some("fatal") => "AND level = 1",
        Some("error") => "AND level = 2",
        Some("warn") => "AND level = 3",
        Some("info") => "AND level = 4",
        Some("debug") => "AND level = 5",
        Some("trace") => "AND level = 6",
        _ => "",
    };

    let sql = format!(
        "SELECT time, level, target, msg
         FROM log_entries
         WHERE process_id = '{}'
         {}
         ORDER BY time DESC
         LIMIT {}",
        process_id, level_condition, limit
    );

    client.query_stream(sql, None).await
}

/// Query process statistics (log entries, measures, trace events, thread count)
pub async fn query_process_statistics(
    client: &mut Client,
    process_id: &str,
) -> Result<Vec<RecordBatch>> {
    let sql = format!(
        "SELECT
            SUM(CASE WHEN array_has(\"streams.tags\", 'log') THEN nb_objects ELSE 0 END) as log_entries,
            SUM(CASE WHEN array_has(\"streams.tags\", 'metrics') THEN nb_objects ELSE 0 END) as measures,
            SUM(CASE WHEN array_has(\"streams.tags\", 'cpu') THEN nb_objects ELSE 0 END) as trace_events,
            COUNT(DISTINCT CASE WHEN array_has(\"streams.tags\", 'cpu') THEN stream_id ELSE NULL END) as thread_count
         FROM blocks
         WHERE process_id = '{}'",
        process_id
    );

    client.query(sql, None).await
}

/// Query actual number of trace events for a specific process
pub async fn query_nb_trace_events(
    client: &mut Client,
    process_id: &str,
) -> Result<Vec<RecordBatch>> {
    let sql = format!(
        "SELECT
            SUM(CASE WHEN array_has(\"streams.tags\", 'cpu') THEN nb_objects ELSE 0 END) as trace_events
         FROM blocks
         WHERE process_id = '{}'",
        process_id
    );

    client.query(sql, None).await
}
