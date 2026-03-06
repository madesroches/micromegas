use crate::dfext::string_column_accessor::string_column_by_name;

/// Get thread information from the streams table for a given process.
/// Returns (stream_id, thread_id_numeric, display_name) where display_name is "name-id" or just "id".
pub async fn get_process_thread_list(
    process_id: &str,
    ctx: &datafusion::execution::context::SessionContext,
) -> anyhow::Result<Vec<(String, i32, String)>> {
    let sql = format!(
        r#"
        SELECT b.stream_id,
               property_get("streams.properties", 'thread-name') as thread_name,
               property_get("streams.properties", 'thread-id') as thread_id
        FROM blocks b
        WHERE b.process_id = '{process_id}'
        AND array_has(b."streams.tags", 'cpu')
        GROUP BY stream_id, thread_name, thread_id
        ORDER BY stream_id
        "#,
    );

    let df = ctx.sql(&sql).await?;
    let batches = df.collect().await?;
    let mut threads = Vec::new();

    for batch in batches {
        let stream_ids = string_column_by_name(&batch, "stream_id")?;
        let thread_names = string_column_by_name(&batch, "thread_name")?;
        let thread_ids = string_column_by_name(&batch, "thread_id")?;

        for i in 0..batch.num_rows() {
            let stream_id = stream_ids.value(i)?.to_owned();
            let thread_name = thread_names.value(i)?;
            let thread_id_str = thread_ids.value(i)?;

            // thread_id falls back to stream_id if not available
            let thread_id_for_display = if thread_id_str.is_empty() {
                &stream_id
            } else {
                thread_id_str
            };

            // Parse numeric thread_id for Perfetto (use 0 if not parseable)
            let thread_id_numeric = thread_id_str.parse::<i64>().unwrap_or(0) as i32;

            // Build display name: "name-id" if name exists, otherwise just "id"
            let display_name = if thread_name.is_empty() {
                thread_id_for_display.to_owned()
            } else {
                format!("{thread_name}-{thread_id_for_display}")
            };

            threads.push((stream_id, thread_id_numeric, display_name));
        }
    }

    Ok(threads)
}
