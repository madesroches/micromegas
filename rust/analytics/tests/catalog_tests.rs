use anyhow::Result;
use micromegas_analytics::lakehouse::catalog::list_view_sets;
use micromegas_analytics::lakehouse::log_view::LogViewMaker;
use micromegas_analytics::lakehouse::metrics_view::MetricsViewMaker;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
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
        view_set_names.iter().any(|&name| name == "log_entries"),
        "Should have log_entries view set"
    );
    assert!(
        view_set_names.iter().any(|&name| name == "measures"),
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
