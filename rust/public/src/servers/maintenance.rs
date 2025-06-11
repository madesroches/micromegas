use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, DurationRound};
use chrono::{TimeDelta, Utc};
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_analytics::delete::delete_old_data;
use micromegas_analytics::lakehouse::batch_update::materialize_partition_range;
use micromegas_analytics::lakehouse::partition_cache::PartitionCache;
use micromegas_analytics::lakehouse::temp::delete_expired_temporary_files;
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::response_writer::ResponseWriter;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use tokio::task::JoinSet;

use super::cron_task::{CronTask, TaskCallback};

type Views = Arc<Vec<Arc<dyn View>>>;

pub async fn materialize_all_views(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    views: Views,
    begin_range: DateTime<Utc>,
    end_range: DateTime<Utc>,
    partition_time_delta: TimeDelta,
) -> Result<()> {
    let mut last_group = views.first().unwrap().get_update_group();
    let mut partitions = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lake.db_pool, begin_range, end_range)
            .await?,
    );
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    for view in &*views {
        if view.get_update_group() != last_group {
            // views in the same group should have no inter-dependencies
            last_group = view.get_update_group();
            partitions = Arc::new(
                PartitionCache::fetch_overlapping_insert_range(
                    // we are fetching more partitions than we need, could be optimized
                    &lake.db_pool,
                    begin_range,
                    end_range,
                )
                .await?,
            );
        }
        materialize_partition_range(
            partitions.clone(),
            runtime.clone(),
            lake.clone(),
            view.clone(),
            begin_range,
            end_range,
            partition_time_delta,
            null_response_writer.clone(),
        )
        .await?;
    }
    Ok(())
}

pub struct EveryDayTask {
    pub runtime: Arc<RuntimeEnv>,
    pub lake: Arc<DataLakeConnection>,
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
            self.runtime.clone(),
            self.lake.clone(),
            self.views.clone(),
            begin_range,
            end_range,
            partition_time_delta,
        )
        .await
    }
}

pub struct EveryHourTask {
    pub runtime: Arc<RuntimeEnv>,
    pub lake: Arc<DataLakeConnection>,
    pub views: Views,
}

#[async_trait]
impl TaskCallback for EveryHourTask {
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
        delete_old_data(&self.lake, 90).await?;
        delete_expired_temporary_files(self.lake.clone()).await?;

        let partition_time_delta = TimeDelta::hours(1);
        let trunc_task_time = task_scheduled_time.duration_trunc(partition_time_delta)?;
        let begin_range = trunc_task_time - (partition_time_delta * 2);
        let end_range = trunc_task_time;
        materialize_all_views(
            self.runtime.clone(),
            self.lake.clone(),
            self.views.clone(),
            begin_range,
            end_range,
            partition_time_delta,
        )
        .await
    }
}

pub struct EveryMinuteTask {
    pub runtime: Arc<RuntimeEnv>,
    pub lake: Arc<DataLakeConnection>,
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
            self.runtime.clone(),
            self.lake.clone(),
            self.views.clone(),
            begin_range,
            end_range,
            partition_time_delta,
        )
        .await
    }
}

pub struct EverySecondTask {
    pub runtime: Arc<RuntimeEnv>,
    pub lake: Arc<DataLakeConnection>,
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
            self.runtime.clone(),
            self.lake.clone(),
            self.views.clone(),
            begin_range,
            end_range,
            partition_time_delta,
        )
        .await
    }
}

pub async fn run_tasks_forever(mut tasks: Vec<CronTask>, max_parallelism: usize) {
    let mut task_set = JoinSet::new();
    loop {
        let mut next_task_run = Utc::now() + TimeDelta::days(2);
        for task in &mut tasks {
            if task.get_next_run() < Utc::now() {
                task_set.spawn(task.spawn().await);
                if task_set.len() >= max_parallelism {
                    if let Some(res) = task_set.join_next().await {
                        match res {
                            Ok(res) => {
                                if let Err(e) = res {
                                    error!("{e:?}")
                                }
                            }
                            Err(e) => error!("{e:?}"),
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
                Ok(wait) => tokio::time::sleep(wait).await,
                Err(e) => warn!("{e:?}"),
            }
        }
    }
}

pub async fn daemon(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
) -> Result<()> {
    let mut views_to_update: Vec<Arc<dyn View>> = view_factory
        .get_global_views()
        .iter()
        .filter(|v| v.get_update_group().is_some())
        .cloned()
        .collect();
    views_to_update.sort_by_key(|v| v.get_update_group().unwrap_or(i32::MAX));
    let views = Arc::new(views_to_update);

    let every_day = CronTask::new(
        String::from("every_day"),
        TimeDelta::days(1),
        TimeDelta::hours(4),
        Arc::new(EveryDayTask {
            runtime: runtime.clone(),
            lake: lake.clone(),
            views: views.clone(),
        }),
    )?;
    let every_hour = CronTask::new(
        String::from("every_hour"),
        TimeDelta::hours(1),
        TimeDelta::minutes(10),
        Arc::new(EveryHourTask {
            runtime: runtime.clone(),
            lake: lake.clone(),
            views: views.clone(),
        }),
    )?;
    let every_minute = CronTask::new(
        String::from("every minute"),
        TimeDelta::minutes(1),
        TimeDelta::seconds(30),
        Arc::new(EveryMinuteTask {
            runtime: runtime.clone(),
            lake: lake.clone(),
            views: views.clone(),
        }),
    )?;
    let every_second = CronTask::new(
        String::from("every second"),
        TimeDelta::seconds(1),
        TimeDelta::milliseconds(500),
        Arc::new(EverySecondTask {
            runtime,
            lake,
            views,
        }),
    )?;

    let mut runners = tokio::task::JoinSet::new();
    runners.spawn(async move { run_tasks_forever(vec![every_day], 1).await });
    runners.spawn(async move { run_tasks_forever(vec![every_hour], 1).await });
    runners.spawn(async move { run_tasks_forever(vec![every_minute], 5).await });
    runners.spawn(async move { run_tasks_forever(vec![every_second], 5).await });
    runners.join_all().await;
    Ok(())
}
