use super::{sql_batch_view::SqlBatchView, view_factory::ViewFactory};
use anyhow::Result;
use chrono::TimeDelta;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

pub async fn make_streams_view(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
) -> Result<SqlBatchView> {
    let count_src_query = Arc::new(String::from(
        r#"
        SELECT count(*) as count
        FROM  blocks
        WHERE insert_time >= '{begin}'
        AND   insert_time < '{end}'
        ;"#,
    ));
    let transform_query = Arc::new(String::from(
        r#"
SELECT stream_id,
       first_value("process_id") as process_id,
       first_value("streams.dependencies_metadata") as dependencies_metadata,
       first_value("streams.objects_metadata") as objects_metadata,
       first_value("streams.tags") as tags,
       first_value("streams.properties") as properties,
       first_value("streams.insert_time") as insert_time,
       max(insert_time) as last_update_time
FROM blocks
GROUP BY stream_id
        ;"#,
    ));
    let merge_query = Arc::new(String::from(
        r#"
SELECT stream_id,
       first_value(process_id) as process_id,
       first_value(dependencies_metadata) as dependencies_metadata,
       first_value(objects_metadata) as objects_metadata,
       first_value(tags) as tags,
       first_value(properties) as properties,
       first_value(insert_time) as insert_time,
       max(last_update_time) as last_update_time
FROM {source}
GROUP BY stream_id
        ;"#,
    ));
    let min_time_column = Arc::new(String::from("insert_time"));
    let max_time_column = Arc::new(String::from("last_update_time"));
    SqlBatchView::new(
        runtime,
        Arc::new("streams".to_owned()),
        min_time_column,
        max_time_column,
        count_src_query,
        transform_query,
        merge_query,
        lake,
        view_factory,
        Some(2000),
        TimeDelta::hours(1), // from source
        TimeDelta::hours(1), // when merging
        None,
    )
    .await
}
