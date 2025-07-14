use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use tokio::task::JoinError;

/// Trait for a task that can be run periodically by the cron service.
#[async_trait]
pub trait TaskCallback: Send + Sync {
    /// Runs the task at the scheduled time.
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()>;
}

/// Represents a task scheduled to run periodically.
pub struct CronTask {
    name: String,
    period: TimeDelta,
    callback: Arc<dyn TaskCallback>,
    next_run: DateTime<Utc>,
}

impl CronTask {
    /// Creates a new `CronTask`.
    ///
    /// The `next_run` time is calculated based on the current time, period, and offset.
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

    /// Returns the next scheduled run time for the task.
    ///
    /// This value is updated after each successful `spawn` operation.
    pub fn get_next_run(&self) -> DateTime<Utc> {
        self.next_run
    }

    /// Spawns the task to run in the background.
    ///
    /// This function calculates the next scheduled run time, records metrics about task delay,
    /// and then spawns an asynchronous task to execute the `TaskCallback`.
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
