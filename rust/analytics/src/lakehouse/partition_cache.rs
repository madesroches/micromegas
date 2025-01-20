use crate::{arrow_utils::parse_parquet_metadata, time::TimeRange};

use super::{partition::Partition, view::ViewMetadata};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use std::{fmt, sync::Arc};

#[async_trait]
pub trait QueryPartitionProvider: std::fmt::Display + Send + Sync + std::fmt::Debug {
    async fn fetch(
        &self,
        view_set_name: &str,
        view_instance_id: &str,
        query_range: Option<TimeRange>,
        file_schema_hash: Vec<u8>,
    ) -> Result<Vec<Partition>>;
}

#[derive(Debug)]
pub struct PartitionCache {
    pub partitions: Vec<Partition>,
    begin_insert: DateTime<Utc>,
    end_insert: DateTime<Utc>,
}

impl fmt::Display for PartitionCache {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl PartitionCache {
    pub async fn fetch_overlapping_insert_range(
        pool: &sqlx::PgPool,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Self> {
        let rows = sqlx::query(
            "SELECT view_set_name,
                    view_instance_id,
                    begin_insert_time,
                    end_insert_time,
                    min_event_time,
                    max_event_time,
                    updated,
                    file_path,
                    file_size,
                    file_schema_hash,
                    source_data_hash,
                    file_metadata
             FROM lakehouse_partitions
             WHERE begin_insert_time < $1
             AND end_insert_time > $2
             AND file_metadata IS NOT NULL
             ;",
        )
        .bind(end_insert)
        .bind(begin_insert)
        .fetch_all(pool)
        .await
        .with_context(|| "fetching partitions")?;
        let mut partitions = vec![];
        for r in rows {
            let view_metadata = ViewMetadata {
                view_set_name: Arc::new(r.try_get("view_set_name")?),
                view_instance_id: Arc::new(r.try_get("view_instance_id")?),
                file_schema_hash: r.try_get("file_schema_hash")?,
            };
            let file_metadata_buffer: Vec<u8> = r.try_get("file_metadata")?;
            let file_metadata = Arc::new(parse_parquet_metadata(&file_metadata_buffer.into())?);
            partitions.push(Partition {
                view_metadata,
                begin_insert_time: r.try_get("begin_insert_time")?,
                end_insert_time: r.try_get("end_insert_time")?,
                min_event_time: r.try_get("min_event_time")?,
                max_event_time: r.try_get("max_event_time")?,
                updated: r.try_get("updated")?,
                file_path: r.try_get("file_path")?,
                file_size: r.try_get("file_size")?,
                source_data_hash: r.try_get("source_data_hash")?,
                file_metadata,
            });
        }
        Ok(Self {
            partitions,
            begin_insert,
            end_insert,
        })
    }

    pub fn filter(
        &self,
        view_set_name: &str,
        view_instance_id: &str,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Self> {
        if begin_insert < self.begin_insert || end_insert > self.end_insert {
            anyhow::bail!("filtering from a result set that's not large enough");
        }
        let mut partitions = vec![];
        for part in &self.partitions {
            if *part.view_metadata.view_set_name == view_set_name
                && *part.view_metadata.view_instance_id == view_instance_id
                && part.begin_insert_time < end_insert
                && part.end_insert_time > begin_insert
            {
                partitions.push(part.clone());
            }
        }
        Ok(Self {
            partitions,
            begin_insert,
            end_insert,
        })
    }
}

#[async_trait]
impl QueryPartitionProvider for PartitionCache {
    async fn fetch(
        &self,
        view_set_name: &str,
        view_instance_id: &str,
        query_range: Option<TimeRange>,
        file_schema_hash: Vec<u8>,
    ) -> Result<Vec<Partition>> {
        let mut partitions = vec![];
        if let Some(range) = query_range {
            for part in &self.partitions {
                if *part.view_metadata.view_set_name == view_set_name
                    && *part.view_metadata.view_instance_id == view_instance_id
                    && part.min_event_time < range.end
                    && part.max_event_time > range.begin
                    && part.view_metadata.file_schema_hash == file_schema_hash
                {
                    partitions.push(part.clone());
                }
            }
        } else {
            for part in &self.partitions {
                if *part.view_metadata.view_set_name == view_set_name
                    && *part.view_metadata.view_instance_id == view_instance_id
                    && part.view_metadata.file_schema_hash == file_schema_hash
                {
                    partitions.push(part.clone());
                }
            }
        }
        Ok(partitions)
    }
}

#[derive(Debug)]
pub struct LivePartitionProvider {
    db_pool: PgPool,
}

impl fmt::Display for LivePartitionProvider {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl LivePartitionProvider {
    pub fn new(db_pool: PgPool) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl QueryPartitionProvider for LivePartitionProvider {
    async fn fetch(
        &self,
        view_set_name: &str,
        view_instance_id: &str,
        query_range: Option<TimeRange>,
        file_schema_hash: Vec<u8>,
    ) -> Result<Vec<Partition>> {
        let mut partitions = vec![];
        let rows = if let Some(range) = query_range {
            sqlx::query(
                "SELECT view_set_name,
                    view_instance_id,
                    begin_insert_time,
                    end_insert_time,
                    min_event_time,
                    max_event_time,
                    updated,
                    file_path,
                    file_size,
                    file_schema_hash,
                    source_data_hash,
                    file_metadata
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND min_event_time <= $3
             AND max_event_time >= $4
             AND file_schema_hash = $5
             AND file_metadata IS NOT NULL
             ;",
            )
            .bind(view_set_name)
            .bind(view_instance_id)
            .bind(range.end)
            .bind(range.begin)
            .bind(file_schema_hash)
            .fetch_all(&self.db_pool)
            .await
            .with_context(|| "listing lakehouse partitions")?
        } else {
            sqlx::query(
                "SELECT view_set_name,
                    view_instance_id,
                    begin_insert_time,
                    end_insert_time,
                    min_event_time,
                    max_event_time,
                    updated,
                    file_path,
                    file_size,
                    file_schema_hash,
                    source_data_hash,
                    file_metadata
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND file_schema_hash = $3
             AND file_metadata IS NOT NULL
             ;",
            )
            .bind(view_set_name)
            .bind(view_instance_id)
            .bind(file_schema_hash)
            .fetch_all(&self.db_pool)
            .await
            .with_context(|| "listing lakehouse partitions")?
        };
        for r in rows {
            let view_metadata = ViewMetadata {
                view_set_name: Arc::new(r.try_get("view_set_name")?),
                view_instance_id: Arc::new(r.try_get("view_instance_id")?),
                file_schema_hash: r.try_get("file_schema_hash")?,
            };
            let file_metadata_buffer: Vec<u8> = r.try_get("file_metadata")?;
            let file_metadata = Arc::new(parse_parquet_metadata(&file_metadata_buffer.into())?);
            partitions.push(Partition {
                view_metadata,
                begin_insert_time: r.try_get("begin_insert_time")?,
                end_insert_time: r.try_get("end_insert_time")?,
                min_event_time: r.try_get("min_event_time")?,
                max_event_time: r.try_get("max_event_time")?,
                updated: r.try_get("updated")?,
                file_path: r.try_get("file_path")?,
                file_size: r.try_get("file_size")?,
                source_data_hash: r.try_get("source_data_hash")?,
                file_metadata,
            });
        }
        Ok(partitions)
    }
}

#[derive(Debug)]
pub struct NullPartitionProvider {}

impl fmt::Display for NullPartitionProvider {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[async_trait]
impl QueryPartitionProvider for NullPartitionProvider {
    async fn fetch(
        &self,
        _view_set_name: &str,
        _view_instance_id: &str,
        _query_range: Option<TimeRange>,
        _file_schema_hash: Vec<u8>,
    ) -> Result<Vec<Partition>> {
        Ok(vec![])
    }
}
