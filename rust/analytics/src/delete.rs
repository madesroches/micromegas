use anyhow::{Context, Result};
use chrono::{DateTime, Days, Utc};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::{Row, query};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use crate::lakehouse::write_partition::retire_expired_partitions;

/// Upper bound for a retention horizon; keeps the date arithmetic in range.
pub const MAX_RETENTION_DAYS: u32 = 36_500;

/// How long the raw payloads of a stream are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetentionHorizon {
    Days(u32),
    Forever,
}

/// Payload retention policy: a default horizon and per-stream-tag overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadRetention {
    pub default_days: u32,
    pub by_stream_tag: BTreeMap<String, RetentionHorizon>,
}

/// One bounded block-deletion pass.
#[derive(Debug, PartialEq, Eq)]
pub struct BlockDeletePass {
    pub cutoff: DateTime<Utc>,
    /// `None`: every stream (the default pass). `Some(tags)`: only streams carrying one of them.
    pub include_tags: Option<Vec<String>>,
    /// Streams carrying any of these are left alone by this pass.
    pub exclude_tags: Vec<String>,
}

fn cutoff_for(now: DateTime<Utc>, days: u32) -> Result<DateTime<Utc>> {
    now.checked_sub_days(Days::new(days as u64))
        .with_context(|| "computing expiration date/time")
}

impl PayloadRetention {
    /// Cutoff for everything that follows the default horizon (partitions, empty streams and processes).
    pub fn default_cutoff(&self, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
        cutoff_for(now, self.default_days)
    }

    /// Plans the block-deletion passes. A stream's horizon is the longest among its overridden
    /// tags, because deletion can't be undone.
    pub fn block_delete_passes(&self, now: DateTime<Utc>) -> Result<Vec<BlockDeletePass>> {
        let mut passes = vec![BlockDeletePass {
            cutoff: self.default_cutoff(now)?,
            include_tags: None,
            exclude_tags: self.by_stream_tag.keys().cloned().collect(),
        }];
        let finite: BTreeSet<u32> = self
            .by_stream_tag
            .values()
            .filter_map(|h| match h {
                RetentionHorizon::Days(d) => Some(*d),
                RetentionHorizon::Forever => None,
            })
            .collect();
        for days in finite {
            let horizon = RetentionHorizon::Days(days);
            let include_tags = self
                .by_stream_tag
                .iter()
                .filter(|(_, h)| **h == horizon)
                .map(|(t, _)| t.clone())
                .collect();
            let exclude_tags = self
                .by_stream_tag
                .iter()
                .filter(|(_, h)| **h > horizon)
                .map(|(t, _)| t.clone())
                .collect();
            passes.push(BlockDeletePass {
                cutoff: cutoff_for(now, days)?,
                include_tags: Some(include_tags),
                exclude_tags,
            });
        }
        Ok(passes)
    }
}

/// Deletes a batch of expired blocks from the data lake.
/// Returns `true` if there are more blocks to delete, `false` otherwise.
#[span_fn]
pub async fn delete_expired_blocks_batch(
    lake: &DataLakeConnection,
    pass: &BlockDeletePass,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    let mut transaction = lake.db_pool.begin().await?;
    let rows = match &pass.include_tags {
        None => {
            query(
                "DELETE FROM blocks
                 WHERE block_id IN (
                     SELECT b.block_id FROM blocks b
                     WHERE  b.insert_time <= $1
                     AND    NOT EXISTS (SELECT 1 FROM streams s WHERE s.stream_id = b.stream_id AND s.tags && $3)
                     LIMIT  $2
                 )
                 RETURNING process_id, stream_id, block_id;",
            )
            .bind(pass.cutoff)
            .bind(batch_size)
            .bind(&pass.exclude_tags)
            .fetch_all(&mut *transaction)
            .await?
        }
        Some(include_tags) => {
            query(
                "DELETE FROM blocks
                 WHERE block_id IN (
                     SELECT b.block_id FROM streams s JOIN blocks b ON b.stream_id = s.stream_id
                     WHERE  s.tags && $3 AND NOT (s.tags && $4) AND b.insert_time <= $1
                     LIMIT  $2
                 )
                 RETURNING process_id, stream_id, block_id;",
            )
            .bind(pass.cutoff)
            .bind(batch_size)
            .bind(include_tags)
            .bind(&pass.exclude_tags)
            .fetch_all(&mut *transaction)
            .await?
        }
    };
    if rows.is_empty() {
        return Ok(false);
    }
    let mut paths = vec![];
    for r in &rows {
        let process_id: Uuid = r.try_get("process_id")?;
        let stream_id: Uuid = r.try_get("stream_id")?;
        let block_id: Uuid = r.try_get("block_id")?;
        let path = format!("blobs/{process_id}/{stream_id}/{block_id}");
        paths.push(path);
    }
    lake.blob_storage.delete_batch(&paths).await?;
    transaction.commit().await.with_context(|| "commit")?;
    for path in &paths {
        debug!("deleted blob {path}");
    }
    info!("deleted {} blocks", paths.len());
    Ok(paths.len() == batch_size as usize)
}

/// Deletes all expired blocks selected by a pass.
#[span_fn]
pub async fn delete_expired_blocks(
    lake: &DataLakeConnection,
    pass: &BlockDeletePass,
) -> Result<()> {
    while delete_expired_blocks_batch(lake, pass).await? {}
    Ok(())
}

/// Deletes a batch of empty streams from the data lake.
/// Returns `true` if there are more streams to delete, `false` otherwise.
#[span_fn]
pub async fn delete_empty_streams_batch(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    let rows = query(
        "WITH batch AS (
             SELECT stream_id FROM streams
             WHERE  insert_time <= $1
             AND    NOT EXISTS (SELECT 1 FROM blocks WHERE blocks.stream_id = streams.stream_id LIMIT 1)
             LIMIT  $2
         )
         DELETE FROM streams
         WHERE stream_id IN (SELECT stream_id FROM batch)
         RETURNING stream_id;",
    )
    .bind(expiration)
    .bind(batch_size)
    .fetch_all(&lake.db_pool)
    .await?;
    if rows.is_empty() {
        return Ok(false);
    }
    for r in &rows {
        let stream_id: Uuid = r.try_get("stream_id")?;
        debug!("deleted stream {stream_id}");
    }
    let count = rows.len();
    info!("deleted {count} empty streams");
    Ok(count == batch_size as usize)
}

/// Deletes all empty streams from the data lake.
#[span_fn]
pub async fn delete_empty_streams(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    while delete_empty_streams_batch(lake, expiration).await? {}
    Ok(())
}

/// Deletes a batch of empty processes from the data lake.
/// Returns `true` if there are more processes to delete, `false` otherwise.
#[span_fn]
pub async fn delete_empty_processes_batch(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<bool> {
    let batch_size: i32 = 1000;
    let rows = query(
        "WITH batch AS (
             SELECT process_id FROM processes
             WHERE  insert_time <= $1
             AND    NOT EXISTS (SELECT 1 FROM streams WHERE streams.process_id = processes.process_id LIMIT 1)
             LIMIT  $2
         )
         DELETE FROM processes
         WHERE process_id IN (SELECT process_id FROM batch)
         RETURNING process_id;",
    )
    .bind(expiration)
    .bind(batch_size)
    .fetch_all(&lake.db_pool)
    .await?;
    if rows.is_empty() {
        return Ok(false);
    }
    for r in &rows {
        let process_id: Uuid = r.try_get("process_id")?;
        debug!("deleted process {process_id}");
    }
    let count = rows.len();
    info!("deleted {count} empty processes");
    Ok(count == batch_size as usize)
}

/// Deletes all empty processes from the data lake.
#[span_fn]
pub async fn delete_empty_processes(
    lake: &DataLakeConnection,
    expiration: DateTime<Utc>,
) -> Result<()> {
    while delete_empty_processes_batch(lake, expiration).await? {}
    Ok(())
}

/// Deletes the data that fell out of its retention horizon from the data lake.
#[span_fn]
pub async fn delete_old_data(
    lake: &DataLakeConnection,
    retention: &PayloadRetention,
) -> Result<()> {
    let now = Utc::now();
    let expiration = retention.default_cutoff(now)?;
    for pass in retention.block_delete_passes(now)? {
        delete_expired_blocks(lake, &pass)
            .await
            .with_context(|| "delete_expired_blocks")?;
    }
    delete_empty_streams(lake, expiration)
        .await
        .with_context(|| "delete_empty_streams")?;
    delete_empty_processes(lake, expiration)
        .await
        .with_context(|| "delete_empty_processes")?;
    retire_expired_partitions(lake, expiration)
        .await
        .with_context(|| "retire_expired_partitions")?;
    Ok(())
}
