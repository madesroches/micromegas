//! Offline tests for the DataFusion config overrides: parsing rules, and that every session
//! built from a `LakehouseContext` applies them while keeping the options micromegas forces.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::config::ConfigOptions;
use micromegas_analytics::lakehouse::datafusion_config::{
    DataFusionConfigOverrides, DataFusionConfigSource, PINNED_KEYS,
};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::PartitionCache;
use micromegas_analytics::lakehouse::query::{
    make_session_context, query_partitions, query_partitions_context,
};
use micromegas_analytics::lakehouse::read_scope::CallerContext;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use object_store::ObjectStore;
use serde_json::json;
use std::sync::Arc;

fn strict(value: serde_json::Value) -> Result<DataFusionConfigOverrides> {
    DataFusionConfigOverrides::parse_strict(&value)
}

#[test]
fn strict_stringifies_scalars() {
    let o = strict(json!({
        "datafusion.execution.target_partitions": 2,
        "datafusion.execution.batch_size": "4096",
        "datafusion.optimizer.repartition_joins": false,
    }))
    .unwrap();
    assert_eq!(
        o.to_string(),
        "datafusion.execution.batch_size=4096, \
         datafusion.execution.target_partitions=2, \
         datafusion.optimizer.repartition_joins=false"
    );
    assert!(!o.is_empty());
}

#[test]
fn strict_rejects_bad_input() {
    assert!(strict(json!([1])).is_err());
    assert!(strict(json!("x")).is_err());
    for bad in [json!(null), json!([1]), json!({"a": 1})] {
        assert!(
            strict(json!({"datafusion.execution.target_partitions": bad})).is_err(),
            "non-scalar value must be rejected"
        );
    }
    let err = strict(json!({"datafusion.execution.no_such_option": 1})).unwrap_err();
    assert!(format!("{err:#}").contains("no_such_option"), "{err:#}");
    assert!(strict(json!({"datafusion.execution.target_partitions": "abc"})).is_err());
    assert!(strict(json!({"datafusion.runtime.memory_limit": "1G"})).is_err());
    for key in PINNED_KEYS {
        assert!(strict(json!({ *key: true })).is_err(), "{key} is pinned");
    }
}

#[test]
fn strict_accepts_merge_pinned_keys() {
    strict(json!({"datafusion.optimizer.repartition_file_scans": true})).unwrap();
}

#[test]
fn strict_accepts_the_documented_candidate_keys() {
    strict(json!({
        "datafusion.execution.target_partitions": 2,
        "datafusion.execution.planning_concurrency": 2,
        "datafusion.execution.batch_size": 8192,
        "datafusion.optimizer.repartition_file_min_size": 1048576,
    }))
    .unwrap();
}

#[test]
fn lenient_skips_bad_entries_and_keeps_good_ones() {
    let o = DataFusionConfigOverrides::parse_lenient(&json!({
        "datafusion.execution.target_partitions": 1,
        "datafusion.execution.no_such_option": 1,
        "datafusion.sql_parser.collect_spans": false,
        "datafusion.execution.batch_size": "abc",
        "datafusion.runtime.memory_limit": "1G",
        "datafusion.execution.coalesce_batches": null,
    }))
    .unwrap();
    assert_eq!(o.to_string(), "datafusion.execution.target_partitions=1");
    assert!(DataFusionConfigOverrides::parse_lenient(&json!(3)).is_err());
}

#[test]
fn apply_sets_values() {
    let o = strict(json!({
        "datafusion.execution.target_partitions": 3,
        "datafusion.execution.batch_size": 1024,
    }))
    .unwrap();
    let mut options = ConfigOptions::new();
    o.apply(&mut options).unwrap();
    assert_eq!(options.execution.target_partitions, 3);
    assert_eq!(options.execution.batch_size.get(), 1024);
}

#[derive(Debug)]
struct FixedSource(Result<Arc<DataFusionConfigOverrides>, String>);

#[async_trait]
impl DataFusionConfigSource for FixedSource {
    async fn overrides(&self) -> Result<Arc<DataFusionConfigOverrides>> {
        self.0.clone().map_err(|e| anyhow!(e))
    }
}

fn source(value: serde_json::Value) -> Arc<FixedSource> {
    Arc::new(FixedSource(Ok(Arc::new(strict(value).unwrap()))))
}

fn offline_lakehouse() -> Arc<LakehouseContext> {
    let db_pool = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
        .expect("connect_lazy should not touch the network");
    let object_store: Arc<dyn ObjectStore> = Arc::new(object_store::memory::InMemory::new());
    let blob_storage = Arc::new(BlobStorage::new(
        object_store,
        object_store::path::Path::from("lakehouse"),
    ));
    let lake = Arc::new(DataLakeConnection::new(db_pool, blob_storage));
    let runtime = Arc::new(make_runtime_env().expect("make_runtime_env"));
    Arc::new(LakehouseContext::new(lake, runtime).expect("LakehouseContext::new"))
}

async fn session(lakehouse: Arc<LakehouseContext>) -> Result<datafusion::prelude::SessionContext> {
    let now = Utc::now();
    make_session_context(
        lakehouse,
        Arc::new(PartitionCache::empty(TimeRange::new(
            now,
            now + TimeDelta::hours(1),
        ))),
        None,
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        CallerContext::maintenance(),
    )
    .await
}

fn id_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]))
}

#[tokio::test]
async fn session_applies_overrides_and_keeps_forced_options() {
    let lakehouse = offline_lakehouse().with_datafusion_config(source(json!({
        "datafusion.execution.target_partitions": 1,
        "datafusion.execution.batch_size": 4096,
    })));
    let ctx = session(lakehouse.clone()).await.unwrap();
    let state = ctx.state();
    let options = state.config_options();
    assert_eq!(options.execution.target_partitions, 1);
    assert_eq!(options.execution.batch_size.get(), 4096);
    assert!(!options.execution.parquet.enable_page_index);
    assert!(options.sql_parser.collect_spans);
    assert!(options.catalog.information_schema);

    let copy = lakehouse.with_runtime(lakehouse.runtime().clone());
    let ctx = session(copy).await.unwrap();
    assert_eq!(
        ctx.state().config_options().execution.target_partitions,
        1,
        "a with_runtime copy keeps the overrides"
    );
}

#[tokio::test]
async fn default_context_has_no_overrides() {
    let ctx = session(offline_lakehouse()).await.unwrap();
    assert_eq!(
        ctx.state().config_options().execution.target_partitions,
        ConfigOptions::new().execution.target_partitions
    );
}

#[tokio::test]
async fn query_partitions_applies_overrides() {
    let lakehouse = offline_lakehouse().with_datafusion_config(source(json!({
        "datafusion.execution.target_partitions": 1,
    })));
    let ctx = query_partitions_context(&lakehouse, id_schema(), Arc::new(vec![]))
        .await
        .unwrap();
    assert_eq!(ctx.state().config_options().execution.target_partitions, 1);
    query_partitions(
        &lakehouse,
        id_schema(),
        Arc::new(vec![]),
        "SELECT * FROM source",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn failing_source_fails_the_session() {
    let lakehouse = offline_lakehouse()
        .with_datafusion_config(Arc::new(FixedSource(Err("settings unreadable".into()))));
    let err = session(lakehouse.clone()).await.err().expect("must fail");
    assert!(format!("{err:#}").contains("settings unreadable"));
    assert!(
        query_partitions_context(&lakehouse, id_schema(), Arc::new(vec![]))
            .await
            .is_err()
    );
}
