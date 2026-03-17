use anyhow::Result;
use datafusion::execution::context::SessionContext;
use micromegas_analytics::dfext::csv_table_provider::csv_table_provider;
use std::io::Write;
use tempfile::NamedTempFile;

#[tokio::test]
async fn test_csv_table_provider() -> Result<()> {
    let mut temp_file = NamedTempFile::with_suffix(".csv")?;
    temp_file.write_all(
        b"id,name,age\n\
          1,Alice,30\n\
          2,Bob,25\n\
          3,Charlie,35\n",
    )?;
    temp_file.flush()?;

    let csv_url = format!("file://{}", temp_file.path().display());
    let ctx = SessionContext::new();
    let table_provider = csv_table_provider(&ctx, &csv_url).await?;
    ctx.register_table("people", table_provider)?;

    let df = ctx.sql("SELECT * FROM people ORDER BY id").await?;
    let results = df.collect().await?;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].num_rows(), 3);

    drop(temp_file);
    Ok(())
}

#[tokio::test]
async fn test_csv_table_provider_query_values() -> Result<()> {
    let mut temp_file = NamedTempFile::with_suffix(".csv")?;
    temp_file.write_all(
        b"product,price,quantity\n\
          Widget,10.5,100\n\
          Gadget,25.0,50\n",
    )?;
    temp_file.flush()?;

    let csv_url = format!("file://{}", temp_file.path().display());
    let ctx = SessionContext::new();
    let table_provider = csv_table_provider(&ctx, &csv_url).await?;
    ctx.register_table("products", table_provider)?;

    let df = ctx
        .sql("SELECT product, price * quantity as total FROM products ORDER BY product")
        .await?;
    let results = df.collect().await?;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].num_rows(), 2);

    drop(temp_file);
    Ok(())
}

#[tokio::test]
async fn test_csv_table_provider_nonexistent_file() -> Result<()> {
    let nonexistent_url = "file:///this/path/does/not/exist/data.csv";
    let ctx = SessionContext::new();
    let result = csv_table_provider(&ctx, nonexistent_url).await;

    assert!(result.is_err(), "Expected error when file doesn't exist");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("No files found"),
        "Error message should mention no files found, got: {error_msg}",
    );

    Ok(())
}
