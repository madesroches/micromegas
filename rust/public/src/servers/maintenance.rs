use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound};
use chrono::{TimeDelta, Utc};
use micromegas_analytics::delete::delete_old_data;
use micromegas_analytics::lakehouse::batch_update::materialize_recent_partitions;
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
type Callback = Box<dyn Fn(Arc<DataLakeConnection>, Views) -> FutureTask>;

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
        callback(lake.clone(), views.clone()).await?;
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
        (self.callback)(self.lake.clone(), self.views.clone())
            .await
            .with_context(|| "TaskDef::tick")?;
        self.next_run = now.duration_trunc(self.period)? + self.period + self.offset;
        Ok(())
    }
}

pub async fn every_day(lake: Arc<DataLakeConnection>, views: Views) -> Result<()> {
    let delta = TimeDelta::days(1);
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    for view in &*views {
        materialize_recent_partitions(
            lake.clone(),
            view.clone(),
            delta,
            3,
            null_response_writer.clone(),
        )
        .await?;
    }
    Ok(())
}

pub async fn every_hour(lake: Arc<DataLakeConnection>, views: Views) -> Result<()> {
    delete_old_data(&lake, 90).await?;
    delete_expired_temporary_files(lake.clone()).await?;
    let delta = TimeDelta::hours(1);
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    for view in &*views {
        materialize_recent_partitions(
            lake.clone(),
            view.clone(),
            delta,
            3,
            null_response_writer.clone(),
        )
        .await?;
    }
    Ok(())
}

pub async fn every_minute(lake: Arc<DataLakeConnection>, views: Views) -> Result<()> {
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    for view in &*views {
        materialize_recent_partitions(
            lake.clone(),
            view.clone(),
            TimeDelta::minutes(1),
            3,
            null_response_writer.clone(),
        )
        .await?;
    }
    Ok(())
}

pub async fn daemon(lake: Arc<DataLakeConnection>, view_factory: Arc<ViewFactory>) -> Result<()> {
    let views = Arc::new(vec![
        view_factory.make_view("log_entries", "global")?,
        view_factory.make_view("measures", "global")?,
    ]);
    let mut tasks = vec![
        TaskDef::start(
            lake.clone(),
            views.clone(),
            String::from("every_day"),
            TimeDelta::days(1),
            TimeDelta::minutes(5),
            Box::new(|lake, views| Box::pin(every_day(lake, views))),
        )
        .await?,
        TaskDef::start(
            lake.clone(),
            views.clone(),
            String::from("every_hour"),
            TimeDelta::hours(1),
            TimeDelta::minutes(2),
            Box::new(|lake, views| Box::pin(every_hour(lake, views))),
        )
        .await?,
        TaskDef::start(
            lake.clone(),
            views.clone(),
            String::from("every minute"),
            TimeDelta::minutes(1),
            TimeDelta::seconds(2),
            Box::new(|lake, views| Box::pin(every_minute(lake, views))),
        )
        .await?,
        // TaskDef::start(
        //     String::from("every second"),
        //     TimeDelta::seconds(1),
        //     TimeDelta::milliseconds(100),
        //     Box::new(|| Box::pin(every_second())),
        // )
        // .await?,
    ];

    loop {
        let mut next_task_run = Utc::now() + TimeDelta::days(2);
        for task in &mut tasks {
            if task.next_run < Utc::now() {
                task.tick().await?;
            }

            if task.next_run < next_task_run {
                next_task_run = task.next_run;
            }
        }
        tokio::time::sleep((next_task_run - Utc::now()).to_std()?).await;
    }
}
