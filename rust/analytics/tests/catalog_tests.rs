use anyhow::Result;
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::catalog::list_view_sets;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::log_stats_view::log_stats_view_definition;
use micromegas_analytics::lakehouse::log_view::LogViewMaker;
use micromegas_analytics::lakehouse::metrics_view::MetricsViewMaker;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view_definition::build_sql_batch_view;
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, ViewMaker, default_view_factory};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

#[tokio::test]
async fn test_list_view_sets_catalog() -> Result<()> {
    // Create minimal test setup without database
    let _runtime = Arc::new(make_runtime_env()?);

    // Create a view factory with some test view sets
    let mut view_factory = ViewFactory::new(vec![]);
    view_factory.add_view_set(String::from("log_entries"), Arc::new(LogViewMaker {}));
    view_factory.add_view_set(String::from("measures"), Arc::new(MetricsViewMaker {}));
    let view_factory = Arc::new(view_factory);

    // Test the catalog function directly
    let view_sets = list_view_sets(&view_factory)?;

    // Verify we get results
    assert!(
        !view_sets.is_empty(),
        "list_view_sets() should return view sets"
    );

    // Verify structure of returned data
    for view_set in &view_sets {
        assert!(
            !view_set.view_set_name.is_empty(),
            "view_set_name should not be empty"
        );
        assert!(
            !view_set.current_schema_hash.is_empty(),
            "schema hash should not be empty"
        );
        assert!(!view_set.schema.is_empty(), "schema should not be empty");
        assert!(
            view_set.schema.contains("Field"),
            "schema should contain 'Field' (Arrow schema format)"
        );
    }

    // Check for expected view sets based on the codebase
    let view_set_names: Vec<&str> = view_sets
        .iter()
        .map(|vs| vs.view_set_name.as_str())
        .collect();

    assert!(
        view_set_names.contains(&"log_entries"),
        "Should have log_entries view set"
    );
    assert!(
        view_set_names.contains(&"measures"),
        "Should have measures view set"
    );

    println!("Found {} view sets:", view_sets.len());
    for view_set in &view_sets {
        println!(
            "  - {}: global={}, has_view_maker={}",
            view_set.view_set_name, view_set.global_instance_available, view_set.has_view_maker
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_view_sets_schema_hash_consistency() -> Result<()> {
    // Create minimal test setup
    let mut view_factory = ViewFactory::new(vec![]);
    view_factory.add_view_set(String::from("log_entries"), Arc::new(LogViewMaker {}));
    view_factory.add_view_set(String::from("measures"), Arc::new(MetricsViewMaker {}));
    let view_factory = Arc::new(view_factory);

    // Test the catalog function
    let view_sets = list_view_sets(&view_factory)?;

    // Verify schema versions are consistent
    for view_set in &view_sets {
        // Schema versions should be non-empty (they are version numbers, not hashes)
        assert!(
            !view_set.current_schema_hash.is_empty(),
            "Schema version for {} should not be empty",
            view_set.view_set_name
        );

        // Schemas should be valid Arrow schema strings
        assert!(
            !view_set.schema.is_empty(),
            "Schema for {} should not be empty",
            view_set.view_set_name
        );
        assert!(
            view_set.schema.contains("Field"),
            "Schema for {} should contain 'Field' (Arrow schema format)",
            view_set.view_set_name
        );

        println!(
            "View set {}: schema version = {:?}",
            view_set.view_set_name, view_set.current_schema_hash
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_view_sets_properties() -> Result<()> {
    // Create minimal test setup
    let mut view_factory = ViewFactory::new(vec![]);
    view_factory.add_view_set(String::from("log_entries"), Arc::new(LogViewMaker {}));
    view_factory.add_view_set(String::from("measures"), Arc::new(MetricsViewMaker {}));
    let view_factory = Arc::new(view_factory);

    // Test the catalog function
    let view_sets = list_view_sets(&view_factory)?;

    // Test properties of view sets
    let global_view_sets: Vec<_> = view_sets
        .iter()
        .filter(|vs| vs.global_instance_available)
        .collect();

    let view_maker_sets: Vec<_> = view_sets.iter().filter(|vs| vs.has_view_maker).collect();

    println!("Global view sets ({}):", global_view_sets.len());
    for vs in &global_view_sets {
        println!("  - {}", vs.view_set_name);
    }

    println!("View maker sets ({}):", view_maker_sets.len());
    for vs in &view_maker_sets {
        println!("  - {}", vs.view_set_name);
    }

    // All view sets should have at least one of these properties
    for view_set in &view_sets {
        assert!(
            view_set.global_instance_available || view_set.has_view_maker,
            "View set {} should have either global instance or view maker",
            view_set.view_set_name
        );
    }

    Ok(())
}

async fn make_offline_lakehouse_context() -> Arc<LakehouseContext> {
    let db_pool = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
        .expect("connect_lazy should not touch the network");
    let object_store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let blob_storage = Arc::new(BlobStorage::new(
        object_store,
        object_store::path::Path::from("lakehouse"),
    ));
    let lake = Arc::new(DataLakeConnection::new(db_pool, blob_storage));
    let runtime = Arc::new(make_runtime_env().expect("make_runtime_env"));
    Arc::new(LakehouseContext::new(lake, runtime).expect("LakehouseContext::new"))
}

fn base_factory(lakehouse: &LakehouseContext) -> ViewFactory {
    let blocks_view =
        Arc::new(BlocksView::new(lakehouse.default_audience()).expect("BlocksView::new"));
    let log_entries_view = LogViewMaker {}
        .make_view("global")
        .expect("log_entries global view");
    let mut factory = ViewFactory::new(vec![log_entries_view, blocks_view]);
    factory.add_view_set("log_entries".to_string(), Arc::new(LogViewMaker {}));
    factory
}

#[tokio::test]
async fn test_list_view_sets_description_and_columns() -> Result<()> {
    let mut view_factory = ViewFactory::new(vec![]);
    view_factory.add_view_set(String::from("log_entries"), Arc::new(LogViewMaker {}));
    view_factory.add_view_set(String::from("measures"), Arc::new(MetricsViewMaker {}));
    view_factory.set_description("measures", Arc::from("# measures"));

    let view_sets = list_view_sets(&view_factory)?;
    let find = |name: &str| view_sets.iter().find(|vs| vs.view_set_name == name);
    let measures = find("measures").expect("measures");
    assert_eq!(measures.description.as_deref(), Some("# measures"));
    assert!(
        find("log_entries")
            .expect("log_entries")
            .description
            .is_none()
    );

    let lines: Vec<&str> = measures.columns.lines().collect();
    assert!(
        lines.contains(&"name: Dictionary(Int32, Utf8)"),
        "{lines:?}"
    );
    assert!(lines.contains(&"value: Float64"), "{lines:?}");
    assert!(
        lines.iter().all(|l| l.split_once(": ").is_some()),
        "every line is `name: type`: {lines:?}"
    );
    assert!(
        measures.sort_order.is_none(),
        "maker-only set has no sort order"
    );
    Ok(())
}

#[tokio::test]
async fn test_list_view_sets_sort_order_of_global_views() -> Result<()> {
    let lakehouse = make_offline_lakehouse_context().await;
    let mut factory = base_factory(&lakehouse);

    let ordered_def = log_stats_view_definition();
    let mut unordered_def = log_stats_view_definition();
    unordered_def.view_set_name = "unordered_stats".to_string();
    unordered_def.options.merge_sort_order = None;

    for definition in [ordered_def, unordered_def] {
        let view = build_sql_batch_view(
            &definition,
            lakehouse.runtime().clone(),
            lakehouse.lake().clone(),
            Arc::new(factory.clone()),
            Arc::new(NoOpSessionConfigurator),
        )
        .await?;
        factory.add_global_view(Arc::new(view));
    }

    let view_sets = list_view_sets(&factory)?;
    let find = |name: &str| view_sets.iter().find(|vs| vs.view_set_name == name);
    assert_eq!(
        find("log_stats").expect("log_stats").sort_order,
        Some(vec![
            "time_bin".to_string(),
            "process_id".to_string(),
            "level".to_string(),
            "target".to_string()
        ])
    );
    assert!(
        find("unordered_stats")
            .expect("unordered_stats")
            .sort_order
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn every_default_view_set_has_a_description() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = default_view_factory(
        lakehouse.runtime().clone(),
        lakehouse.lake().clone(),
        lakehouse.default_audience(),
    )
    .await
    .expect("default_view_factory");
    let names = factory
        .get_global_views()
        .iter()
        .map(|v| v.get_view_set_name().to_string())
        .chain(factory.get_view_sets().keys().cloned());
    for name in names {
        let description = factory.get_description(&name);
        assert!(
            description.is_some_and(|d| !d.trim().is_empty()),
            "built-in view set {name} ships without a description"
        );
    }
}
