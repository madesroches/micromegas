use anyhow::Result;
use chrono::{DateTime, Utc};
use datafusion::prelude::*;
use datafusion::{
    arrow::array::TimestampNanosecondArray,
    functions_aggregate::min_max::{max, min},
};

use super::typed_column::typed_column;

/// Computes the minimum and maximum timestamps from a DataFrame.
pub async fn min_max_time_dataframe(
    df: DataFrame,
    min_time_column_name: &str,
    max_time_column_name: &str,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let df = df.aggregate(
        vec![],
        vec![
            min(col(min_time_column_name)),
            max(col(max_time_column_name)),
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
    Ok((
        DateTime::from_timestamp_nanos(min_column.value(0)),
        DateTime::from_timestamp_nanos(max_column.value(0)),
    ))
}
