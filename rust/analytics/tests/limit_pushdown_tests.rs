//! Tests for limit pushdown in custom TableProviders.
//!
//! Note: DataFusion's `MemorySourceConfig::with_limit()` works correctly in local execution
//! (via `df.collect()`), but we observed issues when used via FlightSQL streaming.
//! As a defensive measure, we use manual RecordBatch slicing to ensure limits are
//! always enforced, regardless of the execution path.
//!
//! This test suite verifies both approaches work correctly in local execution.

use anyhow::Result;
use datafusion::arrow::array::{ArrayRef, Int64Array, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::{Session, TableProvider};
use datafusion::datasource::TableType;
use datafusion::datasource::memory::{DataSourceExec, MemorySourceConfig};
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::*;
use std::any::Any;
use std::sync::Arc;

/// Generate test data with the given number of rows
fn generate_test_data(schema: &SchemaRef, total_rows: usize) -> RecordBatch {
    let ids: Vec<i64> = (0..total_rows as i64).collect();
    let names: Vec<String> = (0..total_rows).map(|i| format!("row_{i}")).collect();

    let id_array: ArrayRef = Arc::new(Int64Array::from(ids));
    let name_array: ArrayRef = Arc::new(StringArray::from(names));

    RecordBatch::try_new(schema.clone(), vec![id_array, name_array])
        .expect("Failed to create record batch")
}

fn test_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, false),
    ]))
}

// ============================================================================
// Provider using with_limit() - works in local execution, but had issues in FlightSQL
// ============================================================================

/// TableProvider that uses MemorySourceConfig::with_limit()
/// This works correctly in local execution but showed issues in FlightSQL streaming.
#[derive(Debug)]
struct WithLimitTableProvider {
    schema: SchemaRef,
    total_rows: usize,
}

impl WithLimitTableProvider {
    fn new(total_rows: usize) -> Self {
        Self {
            schema: test_schema(),
            total_rows,
        }
    }
}

#[async_trait::async_trait]
impl TableProvider for WithLimitTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let rb = generate_test_data(&self.schema, self.total_rows);

        // with_limit() works in local execution but showed issues in FlightSQL
        let source = MemorySourceConfig::try_new(
            &[vec![rb]],
            self.schema.clone(),
            projection.map(|v| v.to_owned()),
        )?
        .with_limit(limit);

        Ok(DataSourceExec::from_data_source(source))
    }
}

// ============================================================================
// Provider using slice workaround - WORKING APPROACH
// ============================================================================

/// TableProvider that manually slices the RecordBatch to apply limit.
/// This is the workaround for the DataFusion bug.
#[derive(Debug)]
struct SliceWorkaroundTableProvider {
    schema: SchemaRef,
    total_rows: usize,
}

impl SliceWorkaroundTableProvider {
    fn new(total_rows: usize) -> Self {
        Self {
            schema: test_schema(),
            total_rows,
        }
    }
}

#[async_trait::async_trait]
impl TableProvider for SliceWorkaroundTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let rb = generate_test_data(&self.schema, self.total_rows);

        // WORKAROUND: Manually slice the RecordBatch before creating MemorySourceConfig
        let limited_rb = if let Some(n) = limit {
            rb.slice(0, n.min(rb.num_rows()))
        } else {
            rb
        };

        let source = MemorySourceConfig::try_new(
            &[vec![limited_rb]],
            self.schema.clone(),
            projection.map(|v| v.to_owned()),
        )?;

        Ok(DataSourceExec::from_data_source(source))
    }
}

// ============================================================================
// Tests for with_limit() approach (works in local execution)
// ============================================================================

/// Verify with_limit() works for single column projection in local execution
#[tokio::test]
async fn test_with_limit_single_column() -> Result<()> {
    let provider = Arc::new(WithLimitTableProvider::new(100));
    let ctx = SessionContext::new();
    ctx.register_table("test_table", provider)?;

    let df = ctx.sql("SELECT id FROM test_table LIMIT 5").await?;
    let batches = df.collect().await?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(
        total_rows, 5,
        "with_limit() should work for single column projection"
    );

    Ok(())
}

/// Verify with_limit() works for SELECT * in local execution
#[tokio::test]
async fn test_with_limit_select_star() -> Result<()> {
    let provider = Arc::new(WithLimitTableProvider::new(100));
    let ctx = SessionContext::new();
    ctx.register_table("test_table", provider)?;

    let df = ctx.sql("SELECT * FROM test_table LIMIT 5").await?;
    let batches = df.collect().await?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(
        total_rows, 5,
        "with_limit() should work for SELECT * in local execution"
    );

    Ok(())
}

/// Verify with_limit() works for multi-column projection in local execution
#[tokio::test]
async fn test_with_limit_multi_column() -> Result<()> {
    let provider = Arc::new(WithLimitTableProvider::new(100));
    let ctx = SessionContext::new();
    ctx.register_table("test_table", provider)?;

    let df = ctx.sql("SELECT id, name FROM test_table LIMIT 5").await?;
    let batches = df.collect().await?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(
        total_rows, 5,
        "with_limit() should work for multi-column in local execution"
    );

    Ok(())
}

// ============================================================================
// Tests demonstrating the workaround works
// ============================================================================

/// Verify the slice workaround works for SELECT *
#[tokio::test]
async fn test_slice_workaround_select_star() -> Result<()> {
    let provider = Arc::new(SliceWorkaroundTableProvider::new(100));
    let ctx = SessionContext::new();
    ctx.register_table("test_table", provider)?;

    let df = ctx.sql("SELECT * FROM test_table LIMIT 5").await?;
    let batches = df.collect().await?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(
        total_rows, 5,
        "Slice workaround should return exactly 5 rows for SELECT *"
    );

    Ok(())
}

/// Verify the slice workaround works for multi-column projection
#[tokio::test]
async fn test_slice_workaround_multi_column() -> Result<()> {
    let provider = Arc::new(SliceWorkaroundTableProvider::new(100));
    let ctx = SessionContext::new();
    ctx.register_table("test_table", provider)?;

    let df = ctx.sql("SELECT id, name FROM test_table LIMIT 5").await?;
    let batches = df.collect().await?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(
        total_rows, 5,
        "Slice workaround should return exactly 5 rows for multi-column"
    );

    Ok(())
}

/// Verify the slice workaround works for single column projection
#[tokio::test]
async fn test_slice_workaround_single_column() -> Result<()> {
    let provider = Arc::new(SliceWorkaroundTableProvider::new(100));
    let ctx = SessionContext::new();
    ctx.register_table("test_table", provider)?;

    let df = ctx.sql("SELECT id FROM test_table LIMIT 5").await?;
    let batches = df.collect().await?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(
        total_rows, 5,
        "Slice workaround should return exactly 5 rows for single column"
    );

    Ok(())
}

/// Verify no limit returns all rows
#[tokio::test]
async fn test_slice_workaround_no_limit() -> Result<()> {
    let provider = Arc::new(SliceWorkaroundTableProvider::new(50));
    let ctx = SessionContext::new();
    ctx.register_table("test_table", provider)?;

    let df = ctx.sql("SELECT * FROM test_table").await?;
    let batches = df.collect().await?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(total_rows, 50, "No limit should return all 50 rows");

    Ok(())
}

/// Verify LIMIT 0 returns no rows
#[tokio::test]
async fn test_slice_workaround_limit_zero() -> Result<()> {
    let provider = Arc::new(SliceWorkaroundTableProvider::new(100));
    let ctx = SessionContext::new();
    ctx.register_table("test_table", provider)?;

    let df = ctx.sql("SELECT * FROM test_table LIMIT 0").await?;
    let batches = df.collect().await?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(total_rows, 0, "LIMIT 0 should return 0 rows");

    Ok(())
}

/// Verify limit larger than data returns all rows
#[tokio::test]
async fn test_slice_workaround_limit_exceeds_data() -> Result<()> {
    let provider = Arc::new(SliceWorkaroundTableProvider::new(10));
    let ctx = SessionContext::new();
    ctx.register_table("test_table", provider)?;

    let df = ctx.sql("SELECT * FROM test_table LIMIT 100").await?;
    let batches = df.collect().await?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(
        total_rows, 10,
        "LIMIT larger than data should return all rows"
    );

    Ok(())
}

/// Verify ORDER BY with limit works (limit applied before ordering at source level)
#[tokio::test]
async fn test_slice_workaround_with_order_by() -> Result<()> {
    let provider = Arc::new(SliceWorkaroundTableProvider::new(100));
    let ctx = SessionContext::new();
    ctx.register_table("test_table", provider)?;

    let df = ctx
        .sql("SELECT id FROM test_table ORDER BY id DESC LIMIT 5")
        .await?;
    let batches = df.collect().await?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(
        total_rows, 5,
        "LIMIT with ORDER BY should return exactly 5 rows"
    );

    Ok(())
}
