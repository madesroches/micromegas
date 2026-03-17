use anyhow::Result;
use datafusion::execution::context::SessionContext;
use micromegas_analytics::lakehouse::session_configurator::SessionConfigurator;
use micromegas_analytics::lakehouse::static_tables_configurator::StaticTablesConfigurator;
use std::io::Write;
use tempfile::TempDir;

fn write_file(dir: &TempDir, name: &str, contents: &[u8]) -> Result<()> {
    let path = dir.path().join(name);
    let mut f = std::fs::File::create(path)?;
    f.write_all(contents)?;
    Ok(())
}

#[tokio::test]
async fn test_auto_discovery_mixed_files() -> Result<()> {
    let dir = TempDir::new()?;

    write_file(
        &dir,
        "users.json",
        br#"{"id": 1, "name": "Alice"}
{"id": 2, "name": "Bob"}
"#,
    )?;

    write_file(
        &dir,
        "products.csv",
        b"id,name,price\n1,Widget,10.5\n2,Gadget,25.0\n",
    )?;

    // This file should be skipped (unsupported extension)
    write_file(&dir, "readme.txt", b"this is not a table")?;

    let url = format!("file://{}/", dir.path().display());
    let ctx = SessionContext::new();
    let configurator = StaticTablesConfigurator::new(&ctx, &url).await?;

    // Apply to a fresh context
    let query_ctx = SessionContext::new();
    configurator.configure(&query_ctx).await?;

    // Verify JSON table is registered and queryable
    let df = query_ctx.sql("SELECT * FROM users ORDER BY id").await?;
    let results = df.collect().await?;
    assert_eq!(results[0].num_rows(), 2);

    // Verify CSV table is registered and queryable
    let df = query_ctx.sql("SELECT * FROM products ORDER BY id").await?;
    let results = df.collect().await?;
    assert_eq!(results[0].num_rows(), 2);

    // Verify .txt file was not registered
    let result = query_ctx.sql("SELECT * FROM readme").await;
    assert!(result.is_err());

    drop(dir);
    Ok(())
}

#[tokio::test]
async fn test_table_names_from_filename_stems() -> Result<()> {
    let dir = TempDir::new()?;

    write_file(
        &dir,
        "event_schemas.json",
        br#"{"event": "click", "version": 1}
"#,
    )?;

    let url = format!("file://{}/", dir.path().display());
    let ctx = SessionContext::new();
    let configurator = StaticTablesConfigurator::new(&ctx, &url).await?;

    let query_ctx = SessionContext::new();
    configurator.configure(&query_ctx).await?;

    // Table name should be the filename stem
    let df = query_ctx.sql("SELECT * FROM event_schemas").await?;
    let results = df.collect().await?;
    assert_eq!(results[0].num_rows(), 1);

    drop(dir);
    Ok(())
}

#[tokio::test]
async fn test_empty_directory() -> Result<()> {
    let dir = TempDir::new()?;

    let url = format!("file://{}/", dir.path().display());
    let ctx = SessionContext::new();
    let configurator = StaticTablesConfigurator::new(&ctx, &url).await?;

    // Should succeed with zero tables
    let query_ctx = SessionContext::new();
    configurator.configure(&query_ctx).await?;

    drop(dir);
    Ok(())
}

#[tokio::test]
async fn test_resilience_to_bad_files() -> Result<()> {
    let dir = TempDir::new()?;

    // Valid CSV
    write_file(&dir, "good.csv", b"id,name\n1,Alice\n2,Bob\n")?;

    // Malformed JSON (should fail to infer schema but not block other files)
    write_file(&dir, "bad.json", b"this is not json\n")?;

    let url = format!("file://{}/", dir.path().display());
    let ctx = SessionContext::new();
    let configurator = StaticTablesConfigurator::new(&ctx, &url).await?;

    let query_ctx = SessionContext::new();
    configurator.configure(&query_ctx).await?;

    // The good CSV should still be queryable
    let df = query_ctx.sql("SELECT * FROM good ORDER BY id").await?;
    let results = df.collect().await?;
    assert_eq!(results[0].num_rows(), 2);

    drop(dir);
    Ok(())
}
