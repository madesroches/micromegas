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
type Callback = Box<dyn Fn(Arc<DataLakeConnection>, Arc<dyn View>, Views) -> FutureTask>;

pub struct TaskDef {
    lake: Arc<DataLakeConnection>,
    blocks_view: Arc<dyn View>,
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
        blocks_view: Arc<dyn View>,
        views: Views,
        name: String,
        period: TimeDelta,
        offset: TimeDelta,
        callback: Callback,
    ) -> Result<Self> {
        let now = Utc::now();
        info!("running scheduled task name={name}");
        if let Err(e) = callback(lake.clone(), blocks_view.clone(), views.clone()).await {
            error!("{e:?}");
        }
        let next_run = now.duration_trunc(period)? + period + offset;
        Ok(Self {
            lake,
            blocks_view,
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
        (self.callback)(
            self.lake.clone(),
            self.blocks_view.clone(),
            self.views.clone(),
        )
        .await
        .with_context(|| "TaskDef::tick")?;
        self.next_run = now.duration_trunc(self.period)? + self.period + self.offset;
        Ok(())
    }
}

pub async fn materialize_all_views(
    lake: Arc<DataLakeConnection>,
    blocks_view: Arc<dyn View>,
    views: Views,
    partition_time_delta: TimeDelta,
    nb_partitions: i32,
) -> Result<()> {
    let now = Utc::now();
    let end_range = now.duration_trunc(partition_time_delta)?;
    let begin_range = end_range - (partition_time_delta * nb_partitions);
    let mut partitions =
        PartitionCache::fetch_overlapping_insert_range(&lake.db_pool, begin_range, end_range)
            .await?;
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    materialize_partition_range(
        &partitions,
        lake.clone(),
        blocks_view,
        begin_range,
        end_range,
        partition_time_delta,
        null_response_writer.clone(),
    )
    .await?;
    partitions =
        PartitionCache::fetch_overlapping_insert_range(&lake.db_pool, begin_range, end_range)
            .await?;
    for view in &*views {
        materialize_partition_range(
            &partitions,
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
    lake: Arc<DataLakeConnection>,
    blocks_view: Arc<dyn View>,
    views: Views,
) -> Result<()> {
    materialize_all_views(lake, blocks_view, views, TimeDelta::days(1), 3).await
}

pub async fn every_hour(
    lake: Arc<DataLakeConnection>,
    blocks_view: Arc<dyn View>,
    views: Views,
) -> Result<()> {
    delete_old_data(&lake, 90).await?;
    delete_expired_temporary_files(lake.clone()).await?;
    materialize_all_views(lake, blocks_view, views, TimeDelta::hours(1), 3).await
}

pub async fn every_minute(
    lake: Arc<DataLakeConnection>,
    blocks_view: Arc<dyn View>,
    views: Views,
) -> Result<()> {
    materialize_all_views(lake, blocks_view, views, TimeDelta::minutes(1), 3).await
}

pub async fn every_second(
    lake: Arc<DataLakeConnection>,
    blocks_view: Arc<dyn View>,
    views: Views,
) -> Result<()> {
    materialize_all_views(lake, blocks_view, views, TimeDelta::seconds(1), 5).await
}

pub async fn daemon(lake: Arc<DataLakeConnection>, view_factory: Arc<ViewFactory>) -> Result<()> {
    let blocks_view = view_factory.make_view("blocks", "global")?;
    let views = Arc::new(vec![
        view_factory.make_view("processes", "global")?,
        view_factory.make_view("streams", "global")?,
        view_factory.make_view("log_entries", "global")?,
        view_factory.make_view("measures", "global")?,
    ]);
    let mut tasks = vec![
        TaskDef::start(
            lake.clone(),
            blocks_view.clone(),
            views.clone(),
            String::from("every_day"),
            TimeDelta::days(1),
            TimeDelta::minutes(5),
            Box::new(|lake, blocks_view, views| Box::pin(every_day(lake, blocks_view, views))),
        )
        .await?,
        TaskDef::start(
            lake.clone(),
            blocks_view.clone(),
            views.clone(),
            String::from("every_hour"),
            TimeDelta::hours(1),
            TimeDelta::minutes(2),
            Box::new(|lake, blocks_view, views| Box::pin(every_hour(lake, blocks_view, views))),
        )
        .await?,
        TaskDef::start(
            lake.clone(),
            blocks_view.clone(),
            views.clone(),
            String::from("every minute"),
            TimeDelta::minutes(1),
            TimeDelta::seconds(2),
            Box::new(|lake, blocks_view, views| Box::pin(every_minute(lake, blocks_view, views))),
        )
        .await?,
        TaskDef::start(
            lake.clone(),
            blocks_view.clone(),
            views.clone(),
            String::from("every second"),
            TimeDelta::seconds(1),
            TimeDelta::milliseconds(100),
            Box::new(|lake, blocks_view, views| Box::pin(every_second(lake, blocks_view, views))),
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
