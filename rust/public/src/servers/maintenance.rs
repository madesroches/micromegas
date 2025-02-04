use anyhow::{Context, Result};
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
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type Views = Arc<Vec<Arc<dyn View>>>;
type FutureTask = Pin<Box<dyn Future<Output = Result<()>>>>;
type Callback = Box<dyn Fn(DateTime<Utc>, Arc<DataLakeConnection>, Views) -> FutureTask>;

pub struct TaskDef {
    lake: Arc<DataLakeConnection>,
    views: Views,
    pub name: String,
    pub period: TimeDelta,
    pub offset: TimeDelta,
    pub callback: Callback,
    pub next_run: DateTime<Utc>,
}

impl TaskDef {
    pub async fn start(
        lake: Arc<DataLakeConnection>,
        views: Views,
        name: String,
        period: TimeDelta,
        offset: TimeDelta,
        callback: Callback,
    ) -> Result<Self> {
        let now = Utc::now();
        info!("running scheduled task name={name}");
        if let Err(e) = callback(now, lake.clone(), views.clone()).await {
            error!("{e:?}");
        }
        let next_run = now.duration_trunc(period)? + period + offset;
        Ok(Self {
            lake,
            views,
            name,
            period,
            offset,
            callback,
            next_run,
        })
    }

    pub async fn tick(&mut self) -> Result<()> {
        let now = Utc::now();
        info!("running scheduled task name={}", &self.name);
        imetric!(
            "task_tick_delay",
            "ns",
            (now - self.next_run)
                .num_nanoseconds()
                .with_context(|| "get tick delay as ns")? as u64
        );
        let task_time = self.next_run;
        self.next_run = now.duration_trunc(self.period)? + self.period + self.offset;
        (self.callback)(task_time, self.lake.clone(), self.views.clone())
            .await
            .with_context(|| "TaskDef::tick")?;
        Ok(())
    }
}

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

pub async fn every_day(
    task_scheduled_time: DateTime<Utc>,
    lake: Arc<DataLakeConnection>,
    views: Views,
) -> Result<()> {
    materialize_all_views(lake, views, task_scheduled_time, TimeDelta::days(1)).await
}

pub async fn every_hour(
    task_scheduled_time: DateTime<Utc>,
    lake: Arc<DataLakeConnection>,
    views: Views,
) -> Result<()> {
    delete_old_data(&lake, 90).await?;
    delete_expired_temporary_files(lake.clone()).await?;
    materialize_all_views(lake, views, task_scheduled_time, TimeDelta::hours(1)).await
}

pub async fn every_minute(
    task_scheduled_time: DateTime<Utc>,
    lake: Arc<DataLakeConnection>,
    views: Views,
) -> Result<()> {
    materialize_all_views(lake, views, task_scheduled_time, TimeDelta::minutes(1)).await
}

pub async fn every_second(
    task_scheduled_time: DateTime<Utc>,
    lake: Arc<DataLakeConnection>,
    views: Views,
) -> Result<()> {
    materialize_all_views(lake, views, task_scheduled_time, TimeDelta::seconds(1)).await
}

pub async fn daemon(lake: Arc<DataLakeConnection>, view_factory: Arc<ViewFactory>) -> Result<()> {
    //todo: spawn two tasks - one for high-frequency small tasks, the other for tasks that can take longer to complete
    let mut views_to_update: Vec<Arc<dyn View>> = view_factory
        .get_global_views()
        .iter()
        .filter(|v| v.get_update_group().is_some())
        .cloned()
        .collect();
    views_to_update.sort_by_key(|v| v.get_update_group().unwrap_or(i32::MAX));
    let views = Arc::new(views_to_update);
    let mut tasks = vec![
        TaskDef::start(
            lake.clone(),
            views.clone(),
            String::from("every_day"),
            TimeDelta::days(1),
            TimeDelta::hours(4),
            Box::new(|task_scheduled_time, lake, views| {
                Box::pin(every_day(task_scheduled_time, lake, views))
            }),
        )
        .await?,
        TaskDef::start(
            lake.clone(),
            views.clone(),
            String::from("every_hour"),
            TimeDelta::hours(1),
            TimeDelta::minutes(10),
            Box::new(|task_scheduled_time, lake, views| {
                Box::pin(every_hour(task_scheduled_time, lake, views))
            }),
        )
        .await?,
        TaskDef::start(
            lake.clone(),
            views.clone(),
            String::from("every minute"),
            TimeDelta::minutes(1),
            TimeDelta::seconds(10),
            Box::new(|task_scheduled_time, lake, views| {
                Box::pin(every_minute(task_scheduled_time, lake, views))
            }),
        )
        .await?,
        TaskDef::start(
            lake.clone(),
            views.clone(),
            String::from("every second"),
            TimeDelta::seconds(1),
            TimeDelta::milliseconds(500),
            Box::new(|task_scheduled_time, lake, views| {
                Box::pin(every_second(task_scheduled_time, lake, views))
            }),
        )
        .await?,
    ];

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
