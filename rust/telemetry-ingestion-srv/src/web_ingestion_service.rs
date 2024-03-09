use crate::data_lake_connection::DataLakeConnection;
use anyhow::Context;
use anyhow::Result;
use bytes::Buf;
use lgn_telemetry_proto::telemetry::{ContainerMetadata, UdtMember, UserDefinedType};
use prost::Message;
use telemetry_sink::block_wire_format::{self, encode_cbor};
use tracing::prelude::*;

fn parse_json_udt_member(json_udt_member: &serde_json::value::Value) -> Result<UdtMember> {
    let name = json_udt_member["name"]
        .as_str()
        .with_context(|| "reading member name")?;
    let type_name = json_udt_member["type_name"]
        .as_str()
        .with_context(|| "reading member type_name")?;
    let offset = json_udt_member["offset"]
        .as_u64()
        .with_context(|| "reading member offset")? as u32;
    let size = json_udt_member["size"]
        .as_u64()
        .with_context(|| "reading member size")? as u32;
    let is_reference = json_udt_member["is_reference"]
        .as_bool()
        .with_context(|| "reading member is_reference")?;
    Ok(UdtMember {
        name: name.to_owned(),
        type_name: type_name.to_owned(),
        offset,
        size,
        is_reference,
    })
}

fn parse_json_udt(json_udt: &serde_json::value::Value) -> Result<UserDefinedType> {
    let name = json_udt["name"]
        .as_str()
        .with_context(|| "reading udt name")?;
    let size = json_udt["size"]
        .as_u64()
        .with_context(|| "reading udt size")? as u32;
    let is_reference = json_udt["is_reference"]
        .as_bool()
        .with_context(|| "reading udt is_reference")?;

    let mut members = vec![];
    for json_member in json_udt["members"]
        .as_array()
        .with_context(|| "reading udt members")?
    {
        members.push(parse_json_udt_member(json_member)?);
    }

    Ok(UserDefinedType {
        name: name.to_owned(),
        size,
        members,
        is_reference,
    })
}

fn parse_json_container_metadata(
    json_udts: &[serde_json::value::Value],
) -> Result<ContainerMetadata> {
    let mut udts = vec![];
    for json_udt in json_udts {
        udts.push(parse_json_udt(json_udt)?);
    }
    Ok(ContainerMetadata { types: udts })
}

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
        let block: block_wire_format::Block = ciborium::from_reader(body.reader())?;
        let mut connection = self.lake.db_pool.acquire().await?;
        let encoded_payload = encode_cbor(&block.payload)?;
        let payload_size = encoded_payload.len();
        self.lake
            .blob_storage
            .write_blob(&block.block_id, &encoded_payload)
            .await
            .with_context(|| "Error writing block to blob storage")?;

        sqlx::query("INSERT INTO blocks VALUES(?,?,?,?,?,?,?,?);")
            .bind(block.block_id)
            .bind(block.stream_id)
            .bind(block.begin_time)
            .bind(block.begin_ticks)
            .bind(block.end_time)
            .bind(block.end_ticks)
            .bind(block.nb_objects)
            .bind(payload_size as i64)
            .execute(&mut connection)
            .await
            .with_context(|| "inserting into blocks")?;

        Ok(())
    }

    #[span_fn]
    pub async fn insert_stream(&self, body: serde_json::value::Value) -> Result<()> {
        let mut connection = self.lake.db_pool.acquire().await?;
        let stream_id = body["stream_id"]
            .as_str()
            .with_context(|| "reading stream_id")?;
        let process_id = body["process_id"]
            .as_str()
            .with_context(|| "reading process_id")?;
        let tags = body["tags"].to_string();
        let properties = body["properties"].to_string();
        let dependencies_metadata = parse_json_container_metadata(
            body["dependencies_metadata"]
                .as_array()
                .with_context(|| "reading dependencies_metadata")?,
        )?
        .encode_to_vec();
        let objects_metadata = parse_json_container_metadata(
            body["objects_metadata"]
                .as_array()
                .with_context(|| "reading objects_metadata")?,
        )?
        .encode_to_vec();
        info!("new stream [{}] {}", tags, stream_id);
        sqlx::query("INSERT INTO streams VALUES(?,?,?,?,?,?);")
            .bind(stream_id)
            .bind(process_id)
            .bind(dependencies_metadata)
            .bind(objects_metadata)
            .bind(tags)
            .bind(properties)
            .execute(&mut connection)
            .await
            .with_context(|| "inserting into streams")?;
        Ok(())
    }

    #[span_fn]
    pub async fn insert_process(&self, body: serde_json::value::Value) -> Result<()> {
        let mut connection = self.lake.db_pool.acquire().await?;
        let current_date: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
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

        sqlx::query("INSERT INTO processes VALUES(?,?,?,?,?,?,?,?,?,?,?,?);")
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
            .bind(current_date.format("%Y-%m-%d").to_string())
            .bind(
                body["parent_process_id"]
                    .as_str()
                    .with_context(|| "reading field parent_process_id")?,
            )
            .execute(&mut connection)
            .await
            .with_context(|| "executing sql insert into processes")?;
        Ok(())
    }
}
