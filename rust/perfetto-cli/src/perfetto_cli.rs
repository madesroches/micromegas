use anyhow::Result;
use micromegas::analytics::dfext::typed_column::typed_column_by_name;
use micromegas::chrono::{DateTime, Utc};
use micromegas::client::Client;
use micromegas::datafusion::arrow::array::{StringArray, TimestampNanosecondArray, UInt32Array};
use micromegas::perfetto::writer::Writer;
use micromegas::prost::Message;
use micromegas::tonic::transport::Channel;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

async fn get_process_exe(
    process_id: &str,
    client: &mut Client,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<String> {
    let sql_process = format!(
        r#"
        SELECT "processes.exe" as exe
        FROM blocks
        WHERE process_id = '{process_id}'
        LIMIT 1
        "#
    );
    let batches = client.query(sql_process, begin, end).await?;
    if batches.len() != 1 || batches[0].num_rows() != 1 {
        anyhow::bail!("process not found");
    }
    let exes: &StringArray = typed_column_by_name(&batches[0], "exe")?;
    Ok(exes.value(0).to_owned())
}

#[tokio::main]
async fn main() -> Result<()> {
    let channel = Channel::from_static("grpc://localhost:50051")
        .connect()
        .await?;
    let mut client = Client::new(channel);
    let begin = DateTime::parse_from_rfc3339("2025-01-16T14:36:15.00+00:00")?.into();
    let end = DateTime::parse_from_rfc3339("2025-01-16T14:36:16.00+00:00")?.into();

    let process_id = "941c19c3-4de2-5879-76be-e3af810930be";
    let mut writer = Writer::new(process_id);

    let exe = get_process_exe(process_id, &mut client, begin, end).await?;
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

    let batches = client.query(sql_streams, begin, end).await?;
    for b in batches {
        let stream_id_column: &StringArray = typed_column_by_name(&b, "stream_id")?;
        let thread_name_column: &StringArray = typed_column_by_name(&b, "thread_name")?;
        let thread_id_column: &StringArray = typed_column_by_name(&b, "thread_id")?;
        for row in 0..b.num_rows() {
            let stream_id = stream_id_column.value(row);
            let thread_name = thread_name_column.value(row);
            eprintln!("stream_id={stream_id} thread_name={thread_name}");
            let thread_id = thread_id_column.value(row);
            writer.append_thread_descriptor(stream_id, thread_id.parse::<i32>()?, thread_name);

            let sql_spans = format!(
                r#"
                SELECT id, parent, depth, hash, begin, end, duration, name, target, filename, line
                FROM view_instance('thread_spans', '{stream_id}');
            "#
            );
            let span_batches = client.query(sql_spans, begin, end).await?;
            for b in span_batches {
                let begins: &TimestampNanosecondArray = typed_column_by_name(&b, "begin")?;
                let ends: &TimestampNanosecondArray = typed_column_by_name(&b, "end")?;
                let names: &StringArray = typed_column_by_name(&b, "name")?;
                let targets: &StringArray = typed_column_by_name(&b, "target")?;
                let filenames: &StringArray = typed_column_by_name(&b, "filename")?;
                let lines: &UInt32Array = typed_column_by_name(&b, "line")?;
                for row in 0..b.num_rows() {
                    writer.append_span(
                        begins.value(row) as u64,
                        ends.value(row) as u64,
                        names.value(row),
                        targets.value(row),
                        filenames.value(row),
                        lines.value(row),
                    );
                }
            }
        }
    }

    let mut file = File::create("f:/temp/trace.pb").await?;
    let buf = writer.into_trace().encode_to_vec();
    file.write_all(&buf).await?;

    Ok(())
}
