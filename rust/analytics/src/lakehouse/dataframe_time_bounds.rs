use crate::dfext::typed_column::typed_column;
use crate::time::TimeRange;
use anyhow::Result;
use async_trait::async_trait;
use chrono::DateTime;
use datafusion::prelude::*;
use datafusion::{
    arrow::array::TimestampNanosecondArray,
    functions_aggregate::min_max::{max, min},
};
use std::sync::Arc;

#[async_trait]
pub trait DataFrameTimeBounds: Send + Sync {
    async fn get_time_bounds(&self, df: DataFrame) -> Result<TimeRange>;
}

pub struct NamedColumnsTimeBounds {
    min_column_name: Arc<String>,
    max_column_name: Arc<String>,
}

impl NamedColumnsTimeBounds {
    pub fn new(min_column_name: Arc<String>, max_column_name: Arc<String>) -> Self {
        Self {
            min_column_name,
            max_column_name,
        }
    }
}

#[async_trait]
impl DataFrameTimeBounds for NamedColumnsTimeBounds {
    async fn get_time_bounds(&self, df: DataFrame) -> Result<TimeRange> {
        let df = df.aggregate(
            vec![],
            vec![
                min(col(&*self.min_column_name)),
                max(col(&*self.max_column_name)),
            ],
        )?;
        let minmax = df.collect().await?;
        if minmax.len() != 1 {
            anyhow::bail!("expected minmax to be size 1");
        }
        let minmax = &minmax[0];
        let min_column: &TimestampNanosecondArray = typed_column(minmax, 0)?;
        let max_column: &TimestampNanosecondArray = typed_column(minmax, 1)?;
        if min_column.is_empty() || max_column.is_empty() {
            anyhow::bail!("expected minmax to be size 1");
        }
        Ok(TimeRange::new(
            DateTime::from_timestamp_nanos(min_column.value(0)),
            DateTime::from_timestamp_nanos(max_column.value(0)),
        ))
    }
}
