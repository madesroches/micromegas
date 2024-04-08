use crate::data_lake_connection::DataLakeConnection;
use anyhow::Context;
use anyhow::Result;
use bytes::Buf;
use micromegas_telemetry::block_wire_format;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::prelude::*;

#[derive(Clone)]
pub struct WebIngestionService {
    lake: DataLakeConnection,
}

impl WebIngestionService {
    pub fn new(lake: DataLakeConnection) -> Self {
        Self { lake }
    }

    #[span_fn]
    pub async fn insert_block(&self, body: bytes::Bytes) -> Result<()> {
        let block: block_wire_format::Block = ciborium::from_reader(body.reader())
            .with_context(|| "parsing block_wire_format::Block")?;
        let encoded_payload = encode_cbor(&block.payload)?;
        let payload_size = encoded_payload.len();

        let process_id = "process_id"; //todo
        let stream_id = "stream_id"; //todo
        let block_id = &block.block_id;
        let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");

        self.lake
            .blob_storage
            .put(&obj_path, encoded_payload.into())
            .await
            .with_context(|| "Error writing block to blob storage")?;

        sqlx::query("INSERT INTO blocks VALUES($1,$2,$3,$4,$5,$6,$7,$8);")
            .bind(block.block_id)
            .bind(block.stream_id)
            .bind(block.begin_time)
            .bind(block.begin_ticks)
            .bind(block.end_time)
            .bind(block.end_ticks)
            .bind(block.nb_objects)
            .bind(payload_size as i64)
            .execute(&self.lake.db_pool)
            .await
            .with_context(|| "inserting into blocks")?;

        Ok(())
    }

    #[span_fn]
    pub async fn insert_stream(&self, stream_info: StreamInfo) -> Result<()> {
        info!(
            "new stream {:?} {}",
            &stream_info.tags, stream_info.stream_id
        );
        sqlx::query("INSERT INTO streams VALUES($1,$2,$3,$4,$5);")
            .bind(stream_info.stream_id)
            .bind(stream_info.process_id)
            .bind(encode_cbor(&stream_info.dependencies_metadata)?)
            .bind(encode_cbor(&stream_info.objects_metadata)?)
            .bind(serde_json::to_string(&stream_info.tags)?)
            .bind(serde_json::to_string(&stream_info.properties)?)
            .execute(&self.lake.db_pool)
            .await
            .with_context(|| "inserting into streams")?;
        Ok(())
    }

    #[span_fn]
    pub async fn insert_process(&self, body: serde_json::value::Value) -> Result<()> {
        let insert_time: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
        info!("insert_process: {body:?}");
        let tsc_frequency = body["tsc_frequency"]
            .as_str()
            .with_context(|| "reading field tsc_frequency")?
            .parse::<i64>()
            .with_context(|| "parsing tsc_frequency")?;

        let start_ticks = body["start_ticks"]
            .as_str()
            .with_context(|| "reading field start_ticks")?
            .parse::<i64>()
            .with_context(|| "parsing start_ticks")?;

        sqlx::query("INSERT INTO processes VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12);")
            .bind(
                body["process_id"]
                    .as_str()
                    .with_context(|| "reading field process_id")?,
            )
            .bind(body["exe"].as_str().with_context(|| "reading field exe")?)
            .bind(
                body["username"]
                    .as_str()
                    .with_context(|| "reading field username")?,
            )
            .bind(
                body["realname"]
                    .as_str()
                    .with_context(|| "reading field realname")?,
            )
            .bind(
                body["computer"]
                    .as_str()
                    .with_context(|| "reading field computer")?,
            )
            .bind(
                body["distro"]
                    .as_str()
                    .with_context(|| "reading field distro")?,
            )
            .bind(
                body["cpu_brand"]
                    .as_str()
                    .with_context(|| "reading field cpu_brand")?,
            )
            .bind(tsc_frequency)
            .bind(
                body["start_time"]
                    .as_str()
                    .with_context(|| "reading field start_time")?,
            )
            .bind(start_ticks)
            .bind(insert_time.to_rfc3339())
            .bind(
                body["parent_process_id"]
                    .as_str()
                    .with_context(|| "reading field parent_process_id")?,
            )
            .execute(&self.lake.db_pool)
            .await
            .with_context(|| "executing sql insert into processes")?;
        Ok(())
    }
}
