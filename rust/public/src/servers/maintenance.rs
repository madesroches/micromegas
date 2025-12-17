use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, DurationRound};
use chrono::{TimeDelta, Utc};
use micromegas_analytics::delete::delete_old_data;
use micromegas_analytics::lakehouse::batch_update::materialize_partition_range;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::PartitionCache;
use micromegas_analytics::lakehouse::temp::delete_expired_temporary_files;
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::response_writer::ResponseWriter;
use micromegas_analytics::time::TimeRange;
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use tokio::task::JoinSet;

use super::cron_task::{CronTask, TaskCallback};

type Views = Arc<Vec<Arc<dyn View>>>;

/// Materializes all views within a given time range.
///
/// This function iterates through the provided views, materializing partitions
/// for each view within the specified `insert_range` and `partition_time_delta`.
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
    for view in &*views {
        if view.get_update_group() != last_group {
            // views in the same group should have no inter-dependencies
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
        materialize_partition_range(
            partitions_all_views.clone(),
            lakehouse.clone(),
            view.clone(),
            insert_range,
            partition_time_delta,
            null_response_writer.clone(),
        )
        .await?;
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

/// task running once an hour to materialize recent partitions
pub struct EveryHourTask {
    pub lakehouse: Arc<LakehouseContext>,
    pub views: Views,
}

#[async_trait]
impl TaskCallback for EveryHourTask {
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
        delete_old_data(self.lakehouse.lake(), 90).await?;
        delete_expired_temporary_files(self.lakehouse.lake().clone()).await?;

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

/// Runs a collection of `CronTask`s indefinitely.
///
/// This function continuously checks for tasks that are due to run, spawns them,
/// and manages their execution, ensuring that `max_parallelism` is not exceeded.
pub async fn run_tasks_forever(mut tasks: Vec<CronTask>, max_parallelism: usize) {
    let mut task_set = JoinSet::new();
    loop {
        let mut next_task_run = Utc::now() + TimeDelta::days(2);
        for task in &mut tasks {
            if task.get_next_run() < Utc::now() {
                task_set.spawn(task.spawn().await);
                if task_set.len() >= max_parallelism
                    && let Some(res) = task_set.join_next().await
                {
                    match res {
                        Ok(res) => match res {
                            Ok(res) => match res {
                                Ok(()) => {}
                                Err(e) => error!("{e:?}"),
                            },
                            Err(e) => error!("{e:?}"),
                        },
                        Err(e) => error!("{e:?}"),
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
                Ok(wait) => tokio::time::sleep(wait).await,
                Err(e) => warn!("{e:?}"),
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
/// and second-based maintenance operations, such as data materialization and cleanup.
///
/// # Arguments
///
/// * `lakehouse` - The lakehouse context with shared metadata cache.
/// * `views_to_update` - A vector of views that need to be updated by the daemon.
pub async fn daemon(
    lakehouse: Arc<LakehouseContext>,
    mut views_to_update: Vec<Arc<dyn View>>,
) -> Result<()> {
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
    let every_second = CronTask::new(
        String::from("every second"),
        TimeDelta::seconds(1),
        TimeDelta::milliseconds(500),
        Arc::new(EverySecondTask { lakehouse, views }),
    )?;

    let mut runners = tokio::task::JoinSet::new();
    runners.spawn(async move { run_tasks_forever(vec![every_day], 1).await });
    runners.spawn(async move { run_tasks_forever(vec![every_hour], 1).await });
    runners.spawn(async move { run_tasks_forever(vec![every_minute], 5).await });
    runners.spawn(async move { run_tasks_forever(vec![every_second], 5).await });
    runners.join_all().await;
    Ok(())
}
