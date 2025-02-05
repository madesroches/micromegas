use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, DurationRound};
use chrono::{TimeDelta, Utc};
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

use super::cron_task::{CronTask, TaskCallback};

type Views = Arc<Vec<Arc<dyn View>>>;

pub async fn materialize_all_views(
    lake: Arc<DataLakeConnection>,
    views: Views,
    task_scheduled_time: DateTime<Utc>,
    partition_time_delta: TimeDelta,
) -> Result<()> {
    let trunc_task_time = task_scheduled_time.duration_trunc(partition_time_delta)?;
    let begin_range = trunc_task_time - (partition_time_delta * 2);
    let end_range = trunc_task_time;
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
                    &lake.db_pool,
                    begin_range,
                    end_range,
                )
                .await?,
            );
        }
        let partition_time_delta = partition_time_delta.min(view.get_max_partition_time_delta());
        materialize_partition_range(
            partitions.clone(),
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
    pub lake: Arc<DataLakeConnection>,
    pub views: Views,
}

#[async_trait]
impl TaskCallback for EveryDayTask {
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
        materialize_all_views(
            self.lake.clone(),
            self.views.clone(),
            task_scheduled_time,
            TimeDelta::days(1),
        )
        .await
    }
}

pub struct EveryHourTask {
    pub lake: Arc<DataLakeConnection>,
    pub views: Views,
}

#[async_trait]
impl TaskCallback for EveryHourTask {
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
        delete_old_data(&self.lake, 90).await?;
        delete_expired_temporary_files(self.lake.clone()).await?;
        materialize_all_views(
            self.lake.clone(),
            self.views.clone(),
            task_scheduled_time,
            TimeDelta::hours(1),
        )
        .await
    }
}

pub struct EveryMinuteTask {
    pub lake: Arc<DataLakeConnection>,
    pub views: Views,
}

#[async_trait]
impl TaskCallback for EveryMinuteTask {
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
        materialize_all_views(
            self.lake.clone(),
            self.views.clone(),
            task_scheduled_time,
            TimeDelta::minutes(1),
        )
        .await
    }
}

pub struct EverySecondTask {
    pub lake: Arc<DataLakeConnection>,
    pub views: Views,
}

#[async_trait]
impl TaskCallback for EverySecondTask {
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()> {
        materialize_all_views(
            self.lake.clone(),
            self.views.clone(),
            task_scheduled_time,
            TimeDelta::seconds(1),
        )
        .await
    }
}

pub async fn run_tasks_forever(mut tasks: Vec<CronTask>) {
    loop {
        let mut next_task_run = Utc::now() + TimeDelta::days(2);
        for task in &mut tasks {
            if task.next_run < Utc::now() {
                if let Err(e) = task.tick().await {
                    error!("{e:?}");
                }
            }

            if task.next_run < next_task_run {
                next_task_run = task.next_run;
            }
        }
        let delay = next_task_run - Utc::now();
        if delay > TimeDelta::zero() {
            match delay.to_std().with_context(|| "delay.to_std") {
                Ok(wait) => tokio::time::sleep(wait).await,
                Err(e) => warn!("{e:?}"),
            }
        }
    }
}

pub async fn daemon(lake: Arc<DataLakeConnection>, view_factory: Arc<ViewFactory>) -> Result<()> {
    let mut views_to_update: Vec<Arc<dyn View>> = view_factory
        .get_global_views()
        .iter()
        .filter(|v| v.get_update_group().is_some())
        .cloned()
        .collect();
    views_to_update.sort_by_key(|v| v.get_update_group().unwrap_or(i32::MAX));
    let views = Arc::new(views_to_update);

    let lf_tasks = vec![
        CronTask::start(
            String::from("every_day"),
            TimeDelta::days(1),
            TimeDelta::hours(4),
            Arc::new(EveryDayTask {
                lake: lake.clone(),
                views: views.clone(),
            }),
        )
        .await?,
        CronTask::start(
            String::from("every_hour"),
            TimeDelta::hours(1),
            TimeDelta::minutes(10),
            Arc::new(EveryHourTask {
                lake: lake.clone(),
                views: views.clone(),
            }),
        )
        .await?,
    ];
    let hf_tasks = vec![
        CronTask::start(
            String::from("every minute"),
            TimeDelta::minutes(1),
            TimeDelta::seconds(10),
            Arc::new(EveryMinuteTask {
                lake: lake.clone(),
                views: views.clone(),
            }),
        )
        .await?,
        CronTask::start(
            String::from("every second"),
            TimeDelta::seconds(1),
            TimeDelta::milliseconds(500),
            Arc::new(EverySecondTask { lake, views }),
        )
        .await?,
    ];

    let mut runners = tokio::task::JoinSet::new();
    runners.spawn(async move { run_tasks_forever(lf_tasks).await });
    runners.spawn(async move { run_tasks_forever(hf_tasks).await });
    runners.join_all().await;
    Ok(())
}
