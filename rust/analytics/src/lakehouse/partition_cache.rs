use crate::{arrow_utils::parse_parquet_metadata, time::TimeRange};

use super::{partition::Partition, view::ViewMetadata};
use anyhow::{Context, Result};
use async_trait::async_trait;
use micromegas_tracing::prelude::*;
use sqlx::{PgPool, Row};
use std::{fmt, sync::Arc};

/// A trait for providing queryable partitions.
#[async_trait]
pub trait QueryPartitionProvider: std::fmt::Display + Send + Sync + std::fmt::Debug {
    /// Fetches partitions based on the provided criteria.
    async fn fetch(
        &self,
        view_set_name: &str,
        view_instance_id: &str,
        query_range: Option<TimeRange>,
        file_schema_hash: Vec<u8>,
    ) -> Result<Vec<Partition>>;
}

/// PartitionCache allows to query partitions based on the insert_time range
#[derive(Debug)]
pub struct PartitionCache {
    pub partitions: Vec<Partition>,
    insert_range: TimeRange,
}

impl fmt::Display for PartitionCache {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl PartitionCache {
    pub fn len(&self) -> usize {
        self.partitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.partitions.is_empty()
    }

    /// fetches the partitions of all views matching the specified insert range
    //todo: this should be limited to global instances
    //todo: ask for a list of view sets (which would be provided by the views using a get_dependencies() api entry)
    #[span_fn]
    pub async fn fetch_overlapping_insert_range(
        pool: &sqlx::PgPool,
        insert_range: TimeRange,
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
                    file_metadata,
                    num_rows
             FROM lakehouse_partitions
             WHERE begin_insert_time < $1
             AND end_insert_time > $2
             AND file_metadata IS NOT NULL
             ORDER BY begin_insert_time, file_path
             ;",
        )
        .bind(insert_range.end)
        .bind(insert_range.begin)
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
                num_rows: r.try_get("num_rows")?,
                file_metadata,
            });
        }
        Ok(Self {
            partitions,
            insert_range,
        })
    }

    /// fetches the partitions of a single view instance matching the specified insert range
    #[span_fn]
    pub async fn fetch_overlapping_insert_range_for_view(
        pool: &sqlx::PgPool,
        view_set_name: Arc<String>,
        view_instance_id: Arc<String>,
        insert_range: TimeRange,
    ) -> Result<Self> {
        let rows = sqlx::query(
            "SELECT begin_insert_time,
                    end_insert_time,
                    min_event_time,
                    max_event_time,
                    updated,
                    file_path,
                    file_size,
                    file_schema_hash,
                    source_data_hash,
                    file_metadata,
                    num_rows
             FROM lakehouse_partitions
             WHERE begin_insert_time < $1
             AND end_insert_time > $2
             AND view_set_name = $3
             AND view_instance_id = $4
             AND file_metadata IS NOT NULL
             ORDER BY begin_insert_time, file_path
             ;",
        )
        .bind(insert_range.end)
        .bind(insert_range.begin)
        .bind(&*view_set_name)
        .bind(&*view_instance_id)
        .fetch_all(pool)
        .await
        .with_context(|| "fetching partitions")?;
        let mut partitions = vec![];
        for r in rows {
            let view_metadata = ViewMetadata {
                view_set_name: view_set_name.clone(),
                view_instance_id: view_instance_id.clone(),
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
                num_rows: r.try_get("num_rows")?,
                file_metadata,
            });
        }
        Ok(Self {
            partitions,
            insert_range,
        })
    }

    // overlap test for a specific view
    pub fn filter(
        &self,
        view_set_name: &str,
        view_instance_id: &str,
        file_schema_hash: &[u8],
        insert_range: TimeRange,
    ) -> Self {
        let mut partitions = vec![];
        for part in &self.partitions {
            if *part.view_metadata.view_set_name == view_set_name
                && *part.view_metadata.view_instance_id == view_instance_id
                && part.view_metadata.file_schema_hash == file_schema_hash
                && part.begin_insert_time < insert_range.end
                && part.end_insert_time > insert_range.begin
            {
                partitions.push(part.clone());
            }
        }
        Self {
            partitions,
            insert_range,
        }
    }

    // overlap test for a all views
    pub fn filter_insert_range(&self, insert_range: TimeRange) -> Self {
        let mut partitions = vec![];
        for part in &self.partitions {
            if part.begin_insert_time < insert_range.end
                && part.end_insert_time > insert_range.begin
            {
                partitions.push(part.clone());
            }
        }
        Self {
            partitions,
            insert_range,
        }
    }

    // single view that fits completely in the specified range
    pub fn filter_inside_range(
        &self,
        view_set_name: &str,
        view_instance_id: &str,
        insert_range: TimeRange,
    ) -> Self {
        let mut partitions = vec![];
        for part in &self.partitions {
            if *part.view_metadata.view_set_name == view_set_name
                && *part.view_metadata.view_instance_id == view_instance_id
                && part.begin_insert_time >= insert_range.begin
                && part.end_insert_time <= insert_range.end
            {
                partitions.push(part.clone());
            }
        }
        Self {
            partitions,
            insert_range,
        }
    }
}

#[async_trait]
impl QueryPartitionProvider for PartitionCache {
    /// unlike LivePartitionProvider, the query_range is tested against the insertion time, not the event time
    async fn fetch(
        &self,
        view_set_name: &str,
        view_instance_id: &str,
        query_range: Option<TimeRange>,
        file_schema_hash: Vec<u8>,
    ) -> Result<Vec<Partition>> {
        let mut partitions = vec![];
        if let Some(range) = query_range {
            if range.begin < self.insert_range.begin || range.end > self.insert_range.end {
                anyhow::bail!("filtering from a result set that's not large enough");
            }
            for part in &self.partitions {
                if *part.view_metadata.view_set_name == view_set_name
                    && *part.view_metadata.view_instance_id == view_instance_id
                    && part.begin_insert_time < range.end
                    && part.end_insert_time > range.begin
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

/// A `QueryPartitionProvider` that fetches partitions directly from the database.
#[derive(Debug)]
pub struct LivePartitionProvider {
    db_pool: PgPool,
}

impl fmt::Display for LivePartitionProvider {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl LivePartitionProvider {
    pub fn new(db_pool: PgPool) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl QueryPartitionProvider for LivePartitionProvider {
    #[span_fn]
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
                    file_metadata,
                    num_rows
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND min_event_time <= $3
             AND max_event_time >= $4
             AND file_schema_hash = $5
             AND file_metadata IS NOT NULL
             ORDER BY begin_insert_time, file_path
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
                    file_metadata,
                    num_rows
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND file_schema_hash = $3
             AND file_metadata IS NOT NULL
             ORDER BY begin_insert_time, file_path
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
                num_rows: r.try_get("num_rows")?,
                file_metadata,
            });
        }
        Ok(partitions)
    }
}

/// A `QueryPartitionProvider` that always returns an empty list of partitions.
#[derive(Debug)]
pub struct NullPartitionProvider {}

impl fmt::Display for NullPartitionProvider {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
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
