use super::{sql_batch_view::SqlBatchView, view_factory::ViewFactory};
use anyhow::Result;
use chrono::TimeDelta;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

/// Creates a new `SqlBatchView` for processes.
pub async fn make_processes_view(
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
SELECT process_id,
       first_value("processes.exe") as exe,
       first_value("processes.username") as username,
       first_value("processes.realname") as realname,
       first_value("processes.computer") as computer,
       first_value("processes.distro") as distro,
       first_value("processes.cpu_brand") as cpu_brand,
       first_value("processes.tsc_frequency") as tsc_frequency,
       first_value("processes.start_time") as start_time,
       first_value("processes.start_ticks") as start_ticks,
       first_value("processes.insert_time") as insert_time,
       first_value("processes.parent_process_id") as parent_process_id,
       first_value("processes.properties") as properties,
       max(insert_time) as last_update_time
FROM blocks
GROUP BY process_id
        ;"#,
    ));
    let merge_query = Arc::new(String::from(
        r#"
SELECT process_id,
       first_value("exe") as exe,
       first_value("username") as username,
       first_value("realname") as realname,
       first_value("computer") as computer,
       first_value("distro") as distro,
       first_value("cpu_brand") as cpu_brand,
       first_value("tsc_frequency") as tsc_frequency,
       first_value("start_time") as start_time,
       first_value("start_ticks") as start_ticks,
       first_value("insert_time") as insert_time,
       first_value("parent_process_id") as parent_process_id,
       first_value("properties") as properties,
       max(last_update_time) as last_update_time
FROM {source}
GROUP BY process_id
        ;"#,
    ));
    let min_time_column = Arc::new(String::from("insert_time"));
    let max_time_column = Arc::new(String::from("last_update_time"));
    SqlBatchView::new(
        runtime,
        Arc::new("processes".to_owned()),
        min_time_column,
        max_time_column,
        count_src_query,
        transform_query,
        merge_query,
        lake,
        view_factory,
        Some(2000),
        TimeDelta::days(1), // from source
        TimeDelta::days(1), // when merging
        None,
    )
    .await
}
