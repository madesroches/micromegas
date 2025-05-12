use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use tokio::task::JoinError;

#[async_trait]
pub trait TaskCallback: Send + Sync {
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()>;
}

pub struct CronTask {
    name: String,
    period: TimeDelta,
    callback: Arc<dyn TaskCallback>,
    next_run: DateTime<Utc>,
}

impl CronTask {
    pub fn new(
        name: String,
        period: TimeDelta,
        offset: TimeDelta,
        callback: Arc<dyn TaskCallback>,
    ) -> Result<Self> {
        let now = Utc::now();
        let next_run = now.duration_trunc(period)? + period + offset;
        Ok(Self {
            name,
            period,
            callback,
            next_run,
        })
    }

    pub fn get_next_run(&self) -> DateTime<Utc> {
        self.next_run
    }

    pub async fn spawn(&mut self) -> BoxFuture<'static, Result<Result<()>, JoinError>> {
        let now = Utc::now();
        info!("running scheduled task name={}", &self.name);
        let task_time: DateTime<Utc> = self.next_run;
        self.next_run += self.period;
        imetric!(
            "task_tick_delay",
            "ns",
            (now - task_time)
                .num_nanoseconds()
                .with_context(|| "get tick delay as ns")
                .unwrap() as u64
        );
        let callback = self.callback.clone();
        Box::pin(tokio::spawn(async move {
            let res = callback
                .run(task_time)
                .await
                .with_context(|| "TaskDef::tick");
            imetric!(
                "task_tick_latency",
                "ns",
                (Utc::now() - task_time)
                    .num_nanoseconds()
                    .with_context(|| "get tick delay as ns")
                    .unwrap() as u64
            );
            res
        }))
    }
}
