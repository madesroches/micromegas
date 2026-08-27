use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, DurationRound};
use chrono::{TimeDelta, Utc};
use micromegas_analytics::audience::audience_column_mismatch;
use micromegas_analytics::delete::delete_old_data;
use micromegas_analytics::lakehouse::batch_update::materialize_partition_range;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::PartitionCache;
use micromegas_analytics::lakehouse::temp::delete_expired_temporary_files;
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::response_writer::ResponseWriter;
use micromegas_analytics::time::TimeRange;
use micromegas_tracing::intern_string::intern_string;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set::{Property, PropertySet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::{JoinError, JoinSet};

use super::cron_task::{CronTask, TaskCallback};
use super::pg_stats::PgStatsTask;

type Views = Arc<Vec<Arc<dyn View>>>;

/// Materializes all views within a given time range.
///
/// This function iterates through the provided views, materializing partitions
/// for each view within the specified `insert_range` and `partition_time_delta`.
///
/// Per-view failures are isolated: a view whose `materialize_partition_range`
/// call errors is logged, counted (`materialize_view_failure`), and skipped —
/// every other view, in the same update group or a later one, still gets its
/// own materialization attempt this pass. This is a deliberate cross-group
/// policy ("continue anyway"): materialization is idempotent and re-attempted
/// every tick, so letting later groups proceed even after an earlier-group
/// failure can't corrupt anything, and it avoids reintroducing the
/// group-granularity starvation this isolation is meant to fix. If any view
/// failed, the pass still returns a single aggregated `Err` listing every
/// failed view, so `CronTask`/`log_task_result` continues to record the pass
/// as failed.
#[span_fn]
pub async fn materialize_all_views(
    lakehouse: Arc<LakehouseContext>,
    views: Views,
    insert_range: TimeRange,
    partition_time_delta: TimeDelta,
) -> Result<()> {
    let mut last_group = views.first().unwrap().get_update_group();
    let mut partitions_all_views = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lakehouse.lake().db_pool, insert_range)
            .await?,
    );
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    let mut failures = Vec::new();
    for view in &*views {
        if view.get_update_group() != last_group {
            // Views in the same update group have no inter-dependencies: a
            // SqlBatchView whose count_src_query/extract_query reads another
            // registered view must put that view in an *earlier* group (see
            // SqlBatchView::new's view_factory), so same-group views can
            // always be materialized independently, in any order, with one's
            // failure isolated from the other's.
            last_group = view.get_update_group();
            partitions_all_views = Arc::new(
                PartitionCache::fetch_overlapping_insert_range(
                    // we are fetching more partitions than we need, could be optimized
                    &lakehouse.lake().db_pool,
                    insert_range,
                )
                .await?,
            );
        }
        let view_set_name = view.get_view_set_name();
        let view_instance_id = view.get_view_instance_id();
        if let Err(e) = materialize_partition_range(
            partitions_all_views.clone(),
            lakehouse.clone(),
            view.clone(),
            insert_range,
            partition_time_delta,
            null_response_writer.clone(),
        )
        .await
        {
            error!("materialize_all_views: {view_set_name} {view_instance_id} failed: {e:?}");
            let tags = PropertySet::find_or_create(vec![
                Property::new("view_set_name", intern_string(view_set_name.as_str())),
                Property::new("view_instance_id", intern_string(view_instance_id.as_str())),
            ]);
            imetric!("materialize_view_failure", "count", tags, 1);
            failures.push(format!("{view_set_name} {view_instance_id}"));
        }
    }
    if !failures.is_empty() {
        anyhow::bail!(
            "materialize_all_views: {} view(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
    Ok(())
}

/// task running once a day to materialize older partitions
pub struct EveryDayTask {
    pub lakehouse: Arc<LakehouseContext>,
    pub views: Views,
}

#[async_trait]
impl TaskCallback for EveryDayTask {
    #[span_fn]
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
        let partition_time_delta = TimeDelta::days(1);
        let trunc_task_time = task_scheduled_time.duration_trunc(partition_time_delta)?;
        let begin_range = trunc_task_time - (partition_time_delta * 2);
        let end_range = trunc_task_time;
        materialize_all_views(
            self.lakehouse.clone(),
            self.views.clone(),
            TimeRange::new(begin_range, end_range),
            partition_time_delta,
        )
        .await
    }
}

/// Counts `blocks` rows whose `process_id` disagrees with their stream's own `process_id`, over
/// the last hour (AbAC Stage 5b, #1518, §5). No longer security-critical under per-row stamping
/// -- a block's own `audience` column governs its label regardless of what `process_id` it
/// claims -- so this is a plain data-integrity check, run from the maintenance role rather than
/// the hot ingestion path, exactly so it costs nothing there. Reported as an
/// `imetric!("block_stream_process_id_mismatch", "count", n)` and a `warn!` when non-zero: the
/// healthy baseline is a flat zero, and any non-zero reading is a bug or an attack.
async fn measure_block_stream_process_id_mismatch(
    pool: &sqlx::PgPool,
    since: DateTime<Utc>,
) -> Result<()> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM blocks b
         JOIN streams s ON s.stream_id = b.stream_id
         WHERE b.process_id <> s.process_id
         AND b.insert_time >= $1",
    )
    .bind(since)
    .fetch_one(pool)
    .await
    .with_context(|| "counting block_stream_process_id_mismatch")?;
    if count > 0 {
        warn!(
            "block_stream_process_id_mismatch: {count} block(s) in the last hour disagree with their stream's process_id"
        );
    }
    imetric!("block_stream_process_id_mismatch", "count", count as u64);
    Ok(())
}

/// Counts `blocks` rows whose own `audience` disagrees with their stream's or process's
/// `audience`, over the last hour (AbAC Stage 5b, #1518, §5) -- the live-Postgres counterpart to
/// the per-partition `block_audience_mismatch_excluded` metric `MetadataPartitionSpec::write`
/// emits. Built from [`audience_column_mismatch`], the same NULL-tolerant comparison
/// `blocks_view.rs`'s materialization-time exclusion predicate uses, so the two can never drift
/// apart. Unlike the `process_id` counter above, a non-zero reading here is not necessarily a
/// bug -- it may be the expected result of a re-pointed ingestion credential -- but it always
/// means telemetry was silently dropped from `blocks` (and so from `log_entries`/`measures`/
/// `log_stats`) by that predicate, so it is worth a `warn!` regardless. Reported as
/// `imetric!("block_audience_mismatch_rows", "count", n)`.
async fn measure_block_audience_mismatch_rows(
    pool: &sqlx::PgPool,
    since: DateTime<Utc>,
) -> Result<()> {
    let predicate = format!(
        "({} OR {})",
        audience_column_mismatch("b", "s"),
        audience_column_mismatch("b", "p"),
    );
    let sql = format!(
        "SELECT count(*) FROM blocks b
         JOIN streams s ON s.stream_id = b.stream_id
         JOIN processes p ON p.process_id = b.process_id
         WHERE {predicate}
         AND b.insert_time >= $1"
    );
    let count: i64 = sqlx::query_scalar(&sql)
        .bind(since)
        .fetch_one(pool)
        .await
        .with_context(|| "counting block_audience_mismatch_rows")?;
    if count > 0 {
        warn!(
            "block_audience_mismatch_rows: {count} block(s) in the last hour disagree with their \
             stream's or process's audience -- silently dropped from blocks/log_entries/measures/log_stats \
             by the audience-mismatch predicate"
        );
    }
    imetric!("block_audience_mismatch_rows", "count", count as u64);
    Ok(())
}

/// task running once an hour to materialize recent partitions
pub struct EveryHourTask {
    pub lakehouse: Arc<LakehouseContext>,
    pub views: Views,
    pub retention_days: i32,
}

#[async_trait]
impl TaskCallback for EveryHourTask {
    #[span_fn]
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
        delete_old_data(self.lakehouse.lake(), self.retention_days).await?;
        delete_expired_temporary_files(self.lakehouse.lake().clone()).await?;

        let since = task_scheduled_time - TimeDelta::hours(1);
        measure_block_stream_process_id_mismatch(&self.lakehouse.lake().db_pool, since).await?;
        measure_block_audience_mismatch_rows(&self.lakehouse.lake().db_pool, since).await?;

        let partition_time_delta = TimeDelta::hours(1);
        let trunc_task_time = task_scheduled_time.duration_trunc(partition_time_delta)?;
        let begin_range = trunc_task_time - (partition_time_delta * 2);
        let end_range = trunc_task_time;
        materialize_all_views(
            self.lakehouse.clone(),
            self.views.clone(),
            TimeRange::new(begin_range, end_range),
            partition_time_delta,
        )
        .await
    }
}

/// task running once a minute to materialize recent partitions
pub struct EveryMinuteTask {
    pub lakehouse: Arc<LakehouseContext>,
    pub views: Views,
}

#[async_trait]
impl TaskCallback for EveryMinuteTask {
    #[span_fn]
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
        let partition_time_delta = TimeDelta::minutes(1);
        let trunc_task_time = task_scheduled_time.duration_trunc(partition_time_delta)?;
        let begin_range = trunc_task_time - (partition_time_delta * 2);
        // we only try to process a single partition per view
        let end_range = trunc_task_time - partition_time_delta;
        materialize_all_views(
            self.lakehouse.clone(),
            self.views.clone(),
            TimeRange::new(begin_range, end_range),
            partition_time_delta,
        )
        .await
    }
}

/// task running once a second to materialize newest partitions
pub struct EverySecondTask {
    pub lakehouse: Arc<LakehouseContext>,
    pub views: Views,
}

#[async_trait]
impl TaskCallback for EverySecondTask {
    #[span_fn]
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
        let delay = Utc::now() - task_scheduled_time;
        if delay > TimeDelta::seconds(10) {
            // we don't want to accumulate too much delay - the minutes task will fill the missing data
            warn!("skipping `seconds` task, delay={delay}");
            return Ok(());
        }
        let partition_time_delta = TimeDelta::seconds(1);
        let trunc_task_time = task_scheduled_time.duration_trunc(partition_time_delta)?;
        let begin_range = trunc_task_time - (partition_time_delta * 2);
        // we only try to process a single partition per view
        let end_range = trunc_task_time - partition_time_delta;
        materialize_all_views(
            self.lakehouse.clone(),
            self.views.clone(),
            TimeRange::new(begin_range, end_range),
            partition_time_delta,
        )
        .await
    }
}

/// Logs the outcome of a completed cron task.
///
/// The result is triply nested: the outer `JoinError` reports a panicked or
/// cancelled task, the inner `JoinError` comes from the spawned future, and the
/// innermost `Result` is the task callback's own outcome. Any error at any layer
/// is logged; a fully successful run is a no-op.
fn log_task_result(res: Result<Result<Result<()>, JoinError>, JoinError>) {
    match res {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => error!("{e:?}"),
        Ok(Err(e)) => error!("{e:?}"),
        Err(e) => error!("{e:?}"),
    }
}

/// Awaits and logs every in-flight task, returning once the set is empty.
///
/// Used to drain currently running tasks before the loop exits on shutdown, so
/// their work completes rather than being dropped.
async fn drain_task_set(task_set: &mut JoinSet<Result<Result<()>, JoinError>>) {
    while let Some(res) = task_set.join_next().await {
        log_task_result(res);
    }
}

/// Runs a collection of `CronTask`s until `shutdown` fires.
///
/// When `shutdown` completes, the loop stops scheduling new tasks and drains
/// any currently running tasks before returning.
pub async fn run_tasks_forever<F>(mut tasks: Vec<CronTask>, max_parallelism: usize, shutdown: F)
where
    F: Future<Output = ()>,
{
    tokio::pin!(shutdown);
    let mut task_set = JoinSet::new();
    loop {
        let mut next_task_run = Utc::now() + TimeDelta::days(2);
        for task in &mut tasks {
            if task.get_next_run() < Utc::now() {
                task_set.spawn(task.spawn().await);
                if task_set.len() >= max_parallelism {
                    tokio::select! {
                        res = task_set.join_next() => {
                            if let Some(res) = res {
                                log_task_result(res);
                            }
                        }
                        _ = &mut shutdown => {
                            drain_task_set(&mut task_set).await;
                            return;
                        }
                    }
                }
            }
            let task_next_run = task.get_next_run();
            if task_next_run < next_task_run {
                next_task_run = task_next_run;
            }
        }
        let time_until_next_task = next_task_run - Utc::now();
        if time_until_next_task > TimeDelta::zero() {
            match time_until_next_task
                .to_std()
                .with_context(|| "delay.to_std")
            {
                Ok(wait) => {
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = &mut shutdown => {
                            drain_task_set(&mut task_set).await;
                            return;
                        }
                    }
                }
                Err(e) => warn!("{e:?}"),
            }
        } else {
            // No sleep needed, but still poll the shutdown future so the loop
            // can exit even when tasks run longer than their period.
            tokio::select! {
                biased;
                _ = &mut shutdown => {
                    drain_task_set(&mut task_set).await;
                    return;
                }
                _ = tokio::task::yield_now() => {}
            }
        }
    }
}

/// Retrieves a list of global views that have an associated update group.
///
/// This function filters the global views provided by the `view_factory`,
/// returning only those that are part of an update group.
pub fn get_global_views_with_update_group(view_factory: &ViewFactory) -> Vec<Arc<dyn View>> {
    view_factory
        .get_global_views()
        .iter()
        .filter(|v| v.get_update_group().is_some())
        .cloned()
        .collect()
}

/// Starts the maintenance daemon, which runs various scheduled tasks.
///
/// This function initializes and spawns several `CronTask`s for daily, hourly, minute,
/// and second-based maintenance operations, such as data materialization and cleanup,
/// plus a once-a-minute collector that samples the metadata Postgres's `pg_stat_*`
/// views for self-observability. All runner loops react to `shutdown`: they stop
/// scheduling and drain in-flight tasks. A deadline arm forces return after `grace`
/// even if tasks haven't drained.
///
/// # Arguments
///
/// * `lakehouse` - The lakehouse context with shared metadata cache.
/// * `views_to_update` - A vector of views that need to be updated by the daemon.
/// * `retention_days` - Delete lake data older than this many days (retention horizon).
/// * `shutdown` - Future that completes when the process should begin shutting down.
/// * `grace` - Maximum time to wait for in-flight tasks after the shutdown signal.
pub async fn daemon<F>(
    lakehouse: Arc<LakehouseContext>,
    mut views_to_update: Vec<Arc<dyn View>>,
    retention_days: i32,
    shutdown: F,
    grace: Duration,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    use super::shutdown::ShutdownFanout;

    views_to_update.sort_by_key(|v| v.get_update_group().unwrap_or(i32::MAX));
    let views = Arc::new(views_to_update);

    let every_day = CronTask::new(
        String::from("every_day"),
        TimeDelta::days(1),
        TimeDelta::hours(4),
        Arc::new(EveryDayTask {
            lakehouse: lakehouse.clone(),
            views: views.clone(),
        }),
    )?;
    let every_hour = CronTask::new(
        String::from("every_hour"),
        TimeDelta::hours(1),
        TimeDelta::minutes(10),
        Arc::new(EveryHourTask {
            lakehouse: lakehouse.clone(),
            views: views.clone(),
            retention_days,
        }),
    )?;
    let every_minute = CronTask::new(
        String::from("every minute"),
        TimeDelta::minutes(1),
        TimeDelta::seconds(30),
        Arc::new(EveryMinuteTask {
            lakehouse: lakehouse.clone(),
            views: views.clone(),
        }),
    )?;
    let pg_stats = CronTask::new(
        String::from("pg_stats"),
        TimeDelta::minutes(1),
        TimeDelta::seconds(15), // staggered from the materialization tasks' 30s offset
        Arc::new(PgStatsTask {
            lakehouse: lakehouse.clone(),
        }),
    )?;
    let every_second = CronTask::new(
        String::from("every second"),
        TimeDelta::seconds(1),
        TimeDelta::milliseconds(500),
        Arc::new(EverySecondTask { lakehouse, views }),
    )?;

    let fanout = ShutdownFanout::new(shutdown);
    let grace_secs = grace.as_secs();

    let mut runners = tokio::task::JoinSet::new();
    runners.spawn(run_tasks_forever(vec![every_day], 1, fanout.subscribe()));
    runners.spawn(run_tasks_forever(vec![every_hour], 1, fanout.subscribe()));
    runners.spawn(run_tasks_forever(vec![every_minute], 5, fanout.subscribe()));
    runners.spawn(run_tasks_forever(vec![every_second], 5, fanout.subscribe()));
    runners.spawn(run_tasks_forever(vec![pg_stats], 1, fanout.subscribe()));

    let deadline = {
        let d = fanout.subscribe();
        async move {
            d.await;
            tokio::time::sleep(grace).await;
        }
    };

    tokio::select! {
        _ = runners.join_all() => {
            info!("daemon drain completed");
        }
        _ = deadline => {
            warn!("daemon grace period of {grace_secs}s elapsed with work still in flight");
        }
    }
    Ok(())
}
