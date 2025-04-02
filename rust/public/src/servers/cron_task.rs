use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

#[async_trait]
pub trait TaskCallback: Send + Sync {
    async fn run(&self, task_scheduled_time: DateTime<Utc>) -> Result<()>;
}

pub struct CronTask {
    pub name: String,
    pub period: TimeDelta,
    pub offset: TimeDelta,
    pub callback: Arc<dyn TaskCallback>,
    pub next_run: DateTime<Utc>,
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
        self.callback
            .run(task_time)
            .await
            .with_context(|| "TaskDef::tick")?;
        Ok(())
    }
}
