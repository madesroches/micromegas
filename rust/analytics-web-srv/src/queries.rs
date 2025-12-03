use anyhow::Result;
use datafusion::arrow::array::RecordBatch;
use micromegas::client::flightsql_client::Client;

/// Query all processes ordered by last update time
pub async fn query_all_processes(client: &mut Client) -> Result<Vec<RecordBatch>> {
    let sql = "SELECT process_id, start_time, last_update_time, exe, computer, username, cpu_brand, distro, properties
               FROM processes
               ORDER BY last_update_time DESC";
    client.query(sql.to_owned(), None).await
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
