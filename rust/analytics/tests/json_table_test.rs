use anyhow::Result;
use datafusion::execution::context::SessionContext;
use micromegas_analytics::dfext::json_table_provider::json_table_provider;
use micromegas_analytics::lakehouse::session_configurator::{
    NoOpSessionConfigurator, SessionConfigurator,
};
use std::io::Write;
use std::sync::Arc;
use tempfile::NamedTempFile;

/// Example SessionConfigurator that registers a JSON file as a table
#[derive(Debug)]
struct JsonTableConfigurator {
    table_provider: Arc<dyn datafusion::catalog::TableProvider>,
}

impl JsonTableConfigurator {
    async fn new(json_url: &str) -> Result<Self> {
        let ctx = SessionContext::new();
        let table_provider = json_table_provider(&ctx, json_url).await?;
        Ok(Self { table_provider })
    }
}

#[async_trait::async_trait]
impl SessionConfigurator for JsonTableConfigurator {
    async fn configure(&self, ctx: &SessionContext) -> Result<()> {
        ctx.register_table("example_data", self.table_provider.clone())?;
        Ok(())
    }
}

#[tokio::test]
async fn test_noop_configurator() -> Result<()> {
    let ctx = SessionContext::new();
    let configurator = NoOpSessionConfigurator;
    configurator.configure(&ctx).await?;
    Ok(())
}

#[tokio::test]
async fn test_json_table_provider() -> Result<()> {
    // Create a temporary JSONL file with test data
    let mut temp_file = NamedTempFile::with_suffix(".json")?;

    // Write test data
    temp_file.write_all(
        br#"{"id": 1, "name": "Alice", "age": 30}
{"id": 2, "name": "Bob", "age": 25}
{"id": 3, "name": "Charlie", "age": 35}
"#,
    )?;
    temp_file.flush()?;

    let json_url = format!("file://{}", temp_file.path().display());

    // Create session context
    let ctx = SessionContext::new();

    // Create table provider using the helper function
    let table_provider = json_table_provider(&ctx, &json_url).await?;

    // Register it in the session context
    ctx.register_table("people", table_provider)?;

    // Query the table
    let df = ctx.sql("SELECT * FROM people ORDER BY id").await?;
    let results = df.collect().await?;

    // Verify results
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].num_rows(), 3);

    // Keep temp_file alive until the end
    drop(temp_file);

    Ok(())
}

#[tokio::test]
async fn test_session_configurator_with_json() -> Result<()> {
    // Create a temporary JSONL file with test data
    let mut temp_file = NamedTempFile::with_suffix(".json")?;

    temp_file.write_all(
        br#"{"product": "Widget", "price": 10.5, "quantity": 100}
{"product": "Gadget", "price": 25.0, "quantity": 50}
{"product": "Doohickey", "price": 5.25, "quantity": 200}
"#,
    )?;
    temp_file.flush()?;

    let json_url = format!("file://{}", temp_file.path().display());

    // Create configurator
    let configurator = JsonTableConfigurator::new(&json_url).await?;

    // Create session context and apply configuration
    let ctx = SessionContext::new();
    configurator.configure(&ctx).await?;

    // Query the configured table
    let df = ctx
        .sql("SELECT product, price * quantity as total_value FROM example_data ORDER BY product")
        .await?;
    let results = df.collect().await?;

    // Verify we got results
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].num_rows(), 3);

    // Keep temp_file alive until the end
    drop(temp_file);

    Ok(())
}

#[tokio::test]
async fn test_multiple_json_tables() -> Result<()> {
    // Create two temporary JSONL files
    let mut temp_file1 = NamedTempFile::with_suffix(".json")?;
    temp_file1.write_all(
        br#"{"id": 1, "category": "A"}
{"id": 2, "category": "B"}
"#,
    )?;
    temp_file1.flush()?;
    let json_url1 = format!("file://{}", temp_file1.path().display());

    let mut temp_file2 = NamedTempFile::with_suffix(".json")?;
    temp_file2.write_all(
        br#"{"id": 1, "value": 100}
{"id": 2, "value": 200}
"#,
    )?;
    temp_file2.flush()?;
    let json_url2 = format!("file://{}", temp_file2.path().display());

    // Create session context
    let ctx = SessionContext::new();

    // Create table providers
    let table1 = json_table_provider(&ctx, &json_url1).await?;
    let table2 = json_table_provider(&ctx, &json_url2).await?;

    // Create a custom configurator that registers both tables
    #[derive(Debug)]
    struct MultiTableConfigurator {
        table1: Arc<dyn datafusion::catalog::TableProvider>,
        table2: Arc<dyn datafusion::catalog::TableProvider>,
    }

    #[async_trait::async_trait]
    impl SessionConfigurator for MultiTableConfigurator {
        async fn configure(&self, ctx: &SessionContext) -> Result<()> {
            ctx.register_table("categories", self.table1.clone())?;
            ctx.register_table("values", self.table2.clone())?;
            Ok(())
        }
    }

    let configurator = MultiTableConfigurator { table1, table2 };

    // Apply configuration
    configurator.configure(&ctx).await?;

    // Join the two tables
    let df = ctx
        .sql("SELECT c.id, c.category, v.value FROM categories c JOIN values v ON c.id = v.id ORDER BY c.id")
        .await?;
    let results = df.collect().await?;

    // Verify results
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].num_rows(), 2);

    // Keep temp files alive until the end
    drop(temp_file1);
    drop(temp_file2);

    Ok(())
}

#[tokio::test]
async fn test_json_table_provider_nonexistent_file() -> Result<()> {
    let nonexistent_url = "file:///this/path/does/not/exist/data.json";

    // This should fail when the file doesn't exist
    let ctx = SessionContext::new();
    let result = json_table_provider(&ctx, nonexistent_url).await;

    assert!(result.is_err(), "Expected error when file doesn't exist");

    // Verify the error message is informative
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("No files found"),
        "Error message should mention no files found, got: {}",
        error_msg
    );

    Ok(())
}
